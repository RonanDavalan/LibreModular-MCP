//! Un Hub construit dans des répertoires temporaires privés, et un client MCP pour le piloter.
//!
//! Pourquoi ce module existe
//! Le serveur doit être exercé par le protocole que le modèle emploie, avec de fausses dépendances
//! passées à sa fabrique, jamais par un interrupteur dans le code de production et jamais sur la
//! vraie configuration de l'utilisateur.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use libremodular_mcp::audit::AuditLog;
use libremodular_mcp::config::Config;
use libremodular_mcp::modules::Module;
use libremodular_mcp::registry::Registry;
use libremodular_mcp::security::Token;
use libremodular_mcp::server::{HubContext, HubServer, build_server};
use rmcp::ServiceExt;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

use super::FAKE_TOKEN;

/// L'heure de départ fixe de l'horloge injectée.
pub fn epoch() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000)
}

/// Tout ce qu'un test doit regarder après avoir construit un Hub.
pub struct TestHub {
    pub server: HubServer,
    pub audit_path: PathBuf,
    pub volatile: PathBuf,
    pub now: Arc<Mutex<SystemTime>>,
}

impl TestHub {
    pub fn captures(&self) -> PathBuf {
        self.volatile.join("captures")
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.now.lock().expect("clock");
        *now += by;
    }

    pub fn audit_text(&self) -> String {
        std::fs::read_to_string(&self.audit_path).unwrap_or_default()
    }

    pub fn audit_rows(&self) -> Vec<Value> {
        self.audit_text()
            .lines()
            .map(|line| serde_json::from_str(line).expect("journal line"))
            .collect()
    }
}

pub fn config(text: &str) -> Config {
    Config::from_bytes(text.as_bytes()).expect("test configuration")
}

/// Une configuration qui active `app` sur `port`, plus du TOML `extra`.
pub fn config_for(app: &str, port: u16, extra: &str) -> Config {
    config(&format!(
        "{extra}\n[apps.{app}]\nenabled = true\nport = {port}\n"
    ))
}

/// Construit un Hub sous `base`, avec des modules injectés s'il y en a.
pub fn build(base: &Path, config: Config, injected: Vec<(String, Arc<dyn Module>)>) -> TestHub {
    build_with_token(
        base,
        config,
        injected,
        Token::new(FAKE_TOKEN).expect("token"),
    )
}

/// Comme [`build`], pour un Hub qui doit tenir le jeton qu'un vrai pont lit dans son fichier.
pub fn build_with_token(
    base: &Path,
    config: Config,
    injected: Vec<(String, Arc<dyn Module>)>,
    token: Token,
) -> TestHub {
    let audit_path = base.join("state").join("audit.jsonl");
    let volatile = base.join("run").join("libremodular-mcp");
    let now = Arc::new(Mutex::new(epoch()));
    let clock_source = Arc::clone(&now);
    let audit = AuditLog::new(&audit_path, token.clone(), *config.limits())
        .with_script_text(config.security().audit_script_text);
    let registry = Registry::with_modules(&config, &token, injected);
    let server = build_server(HubContext {
        config,
        registry,
        audit,
        token,
        clock: Arc::new(move || *clock_source.lock().expect("clock")),
        volatile_dir: volatile.clone(),
        home_dir: Some(PathBuf::from("/home/alice")),
    });
    TestHub {
        server,
        audit_path,
        volatile,
        now,
    }
}

/// Un client MCP qui parle en lignes JSON-RPC à un serveur en mémoire.
pub struct Session {
    runtime: tokio::runtime::Runtime,
    writer: WriteHalf<DuplexStream>,
    reader: Lines<BufReader<ReadHalf<DuplexStream>>>,
    next_id: u64,
}

impl Session {
    pub fn start(server: HubServer) -> Session {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (client, server_side) = tokio::io::duplex(1 << 22);
        runtime.spawn(async move {
            if let Ok(running) = server.serve(server_side).await {
                let _ = running.waiting().await;
            }
        });
        let (read, write) = tokio::io::split(client);
        let mut session = Session {
            runtime,
            writer: write,
            reader: BufReader::new(read).lines(),
            next_id: 1,
        };
        let answer = session.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test-client", "version": "0"},
            }),
        );
        assert!(
            answer.get("result").is_some(),
            "initialize failed: {answer}"
        );
        session.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        session
    }

    fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_vec(message).expect("encode");
        line.push(b'\n');
        let writer = &mut self.writer;
        self.runtime.block_on(async {
            writer.write_all(&line).await.expect("write");
            writer.flush().await.expect("flush");
        });
    }

    /// Envoie une requête et rend tout l'objet de réponse.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let reader = &mut self.reader;
        self.runtime.block_on(async {
            loop {
                let line = reader
                    .next_line()
                    .await
                    .expect("read")
                    .expect("the server closed the session");
                let message: Value = serde_json::from_str(&line).expect("JSON-RPC line");
                if message.get("id") == Some(&json!(id)) {
                    return message;
                }
            }
        })
    }

    pub fn tool_names(&mut self) -> Vec<String> {
        let answer = self.request("tools/list", json!({}));
        answer["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name").to_owned())
            .collect()
    }

    /// Appelle un outil et rend le texte brut de sa réponse.
    pub fn call_text(&mut self, tool: &str, arguments: Value) -> String {
        let answer = self.request("tools/call", json!({"name": tool, "arguments": arguments}));
        answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text in {answer}"))
            .to_owned()
    }

    /// Appelle un outil et analyse sa réponse comme du JSON.
    pub fn call(&mut self, tool: &str, arguments: Value) -> Value {
        serde_json::from_str(&self.call_text(tool, arguments)).expect("JSON answer")
    }

    pub fn execute(&mut self, app: &str, script: &str) -> Value {
        self.call(
            "execute_script",
            json!({"target_app": app, "script_content": script}),
        )
    }

    pub fn state(&mut self, app: &str) -> Value {
        self.call("get_state", json!({"target_app": app}))
    }
}
