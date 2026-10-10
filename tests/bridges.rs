//! Les ponts Python vus du Hub : leur propre suite de tests, les ponts générés de GIMP, Krita et
//! Blender pilotés de bout en bout par le module `tcp` du Hub, et le pont LibreOffice piloté par les
//! trois cibles LibreOffice du Hub.
//!
//! Pourquoi ce module existe
//! `cargo test` est la seule commande qui prouve que le produit fonctionne. Elle doit exécuter les
//! tests Python des ponts, échouer quand Python manque au lieu d'ignorer, et montrer que le client du
//! Hub et un vrai fichier de pont se comprennent.

#[macro_use]
mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use common::{FILE_TOKEN, Verdict, must_fail, must_pass, write_file};
use libremodular_mcp::config::{Config, Limits};
use libremodular_mcp::modules::{Module, TcpModule};
use libremodular_mcp::protocol::BridgeConnector;
use libremodular_mcp::registry::Registry;
use libremodular_mcp::security::Token;
use serde_json::json;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const LISTENING: &str = "LMM bridge listening on 127.0.0.1:";

fn generated(name: &str) -> PathBuf {
    Path::new(ROOT).join("bridges").join("generated").join(name)
}

#[test]
fn the_python_tests_of_the_bridges_pass() {
    let output = Command::new("python3")
        .args(["-m", "unittest", "discover", "-s", "tests/bridges", "-v"])
        .current_dir(ROOT)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env_remove("LIBREMODULAR_BRIDGE_PORT")
        .env_remove("LIBREMODULAR_TOKEN_PATH")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("python3 is required to test the bridges: {error}"));
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the Python tests of the bridges failed:\n{report}"
    );
    let ran = report
        .lines()
        .find_map(|line| line.strip_prefix("Ran "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|count| count.parse::<u32>().ok())
        .unwrap_or(0);
    assert!(ran > 0, "no Python test was found:\n{report}");
    assert!(!report.contains(FILE_TOKEN));
}

/// Un pont qui tourne comme un programme Python ; il est tué quand il est abandonné, quelle que
/// soit la conclusion du test.
struct BridgeProcess {
    child: Child,
    port: u16,
    lines: Receiver<String>,
}

impl BridgeProcess {
    fn start(bridge_file: &Path, token_file: &Path) -> Result<BridgeProcess, String> {
        BridgeProcess::start_with(bridge_file, token_file, &[])
    }

    /// `flags` va à `python3` avant le fichier du pont. L'environnement est exactement les cinq variables
    /// posées plus bas : rien d'autre de la machine n'atteint le pont.
    fn start_with(
        bridge_file: &Path,
        token_file: &Path,
        flags: &[&str],
    ) -> Result<BridgeProcess, String> {
        let mut child = Command::new("python3")
            .args(flags)
            .arg(bridge_file)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("LIBREMODULAR_BRIDGE_PORT", "0")
            .env("LIBREMODULAR_TOKEN_PATH", token_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("python3 is required: {error}"))?;
        let stderr = child.stderr.take();
        let (sender, lines) = mpsc::channel();
        let mut process = BridgeProcess {
            child,
            port: 0,
            lines,
        };
        let stderr = stderr.ok_or("the bridge has no error stream")?;
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = process
                .lines
                .recv_timeout(remaining)
                .map_err(|_| "the bridge never said where it listens".to_owned())?;
            ensure!(!line.contains(FILE_TOKEN), "the bridge printed the token");
            if let Some(port) = line.strip_prefix(LISTENING) {
                process.port = port
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad line {line}"))?;
                return Ok(process);
            }
        }
    }

    /// Tout ce que le pont a écrit sur son flux d'erreur depuis la ligne d'écoute.
    fn said(&self) -> Vec<String> {
        self.lines.try_iter().collect()
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Le module propre du Hub doit atteindre le pont de `app`, exécuter un script, recevoir en retour
/// une trace d'appels Python, lire l'état et le sonder. Un `python3` ordinaire n'est pas l'application,
/// qu'elle soit installée ou non : le pont rapporte soit que son API manque, soit la lit directement sans
/// attendre une boucle principale. Chaque échec nomme son étape.
fn check_the_hub_drives_the_bridge(bridge_file: &Path, app: &'static str) -> Verdict {
    let base = common::private_tempdir();
    let token_file = base.path().join("token");
    write_file(&token_file, 0o600, FILE_TOKEN);
    let work_dir = common::fresh(base.path());
    let captures = common::fresh(base.path());
    let bridge = BridgeProcess::start(bridge_file, &token_file)?;
    let token = Token::new(FILE_TOKEN).map_err(|error| error.to_string())?;
    let connector = BridgeConnector::new(app, bridge.port, token, Limits::default());
    let module = TcpModule::new(connector.clone(), Duration::from_secs(10));
    let outcome = (|| -> Verdict {
        let health = module.health();
        if !health.connected {
            // Laisser au pont le temps de remarquer la connexion fermée et de libérer sa place unique, puis
            // demander à la poignée de main elle-même ce qui n'allait pas.
            std::thread::sleep(Duration::from_millis(300));
            let why = connector
                .connect()
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default();
            return Err(format!("health: the bridge is not reachable ({why})"));
        }
        ensure!(
            health.app_version.is_some() && health.bridge_version.as_deref() == Some("1"),
            "health: {health:?}"
        );
        let without_api = health.app_version.as_deref() == Some("unknown");
        let script = "print('hello from the bridge')\nresult = {'sum': 1 + 2, 'dir': LMM_WORK_DIR}";
        let reply = module
            .execute(script, 10, &work_dir)
            .map_err(|error| format!("execute: {error}"))?;
        ensure!(reply.ok, "the script failed: {reply:?}");
        ensure!(reply.stdout == "hello from the bridge\n", "{reply:?}");
        ensure!(
            reply.result == json!({"sum": 3, "dir": work_dir.to_string_lossy()}),
            "{reply:?}"
        );
        let reply = module
            .execute("x = 1\ny = undefined_name\n", 10, &work_dir)
            .map_err(|error| format!("execute with an error: {error}"))?;
        ensure!(!reply.ok, "a NameError was reported as success");
        ensure!(reply.error["type"] == "NameError", "{reply:?}");
        let traceback = reply.error["traceback"].as_str().unwrap_or_default();
        ensure!(
            traceback.contains("File \"<libremodular>\", line 2, in <module>"),
            "{traceback}"
        );
        let state = module
            .state(&captures.join(format!("{app}-e2e.png")), &work_dir)
            .map_err(|error| format!("state: {error}"))?;
        ensure!(state.state.is_object(), "state: not an object: {state:?}");
        ensure!(
            state.screenshot.is_none(),
            "state: unexpected capture {state:?}"
        );
        ensure!(
            state.app_version == health.app_version,
            "state: version {state:?}"
        );
        let missing_api = state
            .warnings
            .iter()
            .any(|warning| warning.contains("cannot be imported"));
        if without_api {
            ensure!(
                state.state == json!({}) && missing_api,
                "state without the API: {state:?}"
            );
        } else {
            ensure!(!missing_api, "state with the API: {state:?}");
        }
        Ok(())
    })();
    module.close();
    let said = bridge.said();
    ensure!(
        !said.iter().any(|line| line.contains(FILE_TOKEN)),
        "the bridge printed the token"
    );
    outcome
}

const BRIDGES: [(&str, &str); 3] = [
    ("gimp", "gimp_bridge.py"),
    ("krita", "krita_bridge.py"),
    ("blender", "blender_bridge.py"),
];

#[test]
fn the_hub_tcp_module_drives_the_generated_gimp_bridge() {
    must_pass(check_the_hub_drives_the_bridge(
        &generated("gimp_bridge.py"),
        "gimp",
    ));
}

#[test]
fn the_hub_tcp_module_drives_the_generated_krita_bridge() {
    must_pass(check_the_hub_drives_the_bridge(
        &generated("krita_bridge.py"),
        "krita",
    ));
}

#[test]
fn the_hub_tcp_module_drives_the_generated_blender_bridge() {
    must_pass(check_the_hub_drives_the_bridge(
        &generated("blender_bridge.py"),
        "blender",
    ));
}

#[test]
fn counter_proof_a_bridge_whose_hello_lacks_the_protocol_is_refused() {
    let base = common::private_tempdir();
    let line = "                \"protocol\": PROTOCOL_VERSION,\n";
    for (app, name) in BRIDGES {
        let text = fs::read_to_string(generated(name)).expect("generated bridge");
        assert_eq!(
            text.matches(line).count(),
            1,
            "the defect must change one place"
        );
        let defective = base
            .path()
            .join(format!("{app}_bridge_without_protocol.py"));
        fs::write(&defective, text.replacen(line, "", 1)).expect("write the defective bridge");
        must_fail(check_the_hub_drives_the_bridge(&defective, app));
    }
}

#[test]
fn counter_proof_a_bridge_on_the_port_of_another_application_is_refused() {
    must_fail(check_the_hub_drives_the_bridge(
        &generated("krita_bridge.py"),
        "blender",
    ));
}

// --- le pont LibreOffice, sans LibreOffice ------------------------------------------

const LIBREOFFICE_TARGETS: [&str; 3] = [
    "libreoffice-writer",
    "libreoffice-calc",
    "libreoffice-impress",
];

/// `python3 -S` démarre sans le module `site`, de sorte que les paquets que le système installe pour
/// lui, `uno` compris, restent introuvables, quelle que soit la machine. Le contrôle s'en assure d'abord,
/// puis pilote chacune des trois cibles du Hub contre le fichier du pont, en nommant l'étape qui a échoué.
fn check_the_hub_reaches_the_libreoffice_bridge_without_libreoffice(bridge_file: &Path) -> Verdict {
    let probe = Command::new("python3")
        .args(["-S", "-c", "import uno"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("python3 is required: {error}"))?;
    ensure!(
        !probe.success(),
        "setup: `python3 -S` can import uno, the environment is not the one described"
    );
    let base = common::private_tempdir();
    let token_file = base.path().join("token");
    write_file(&token_file, 0o600, FILE_TOKEN);
    let work_dir = common::fresh(base.path());
    let captures = common::fresh(base.path());
    let bridge = BridgeProcess::start_with(bridge_file, &token_file, &["-S"])?;
    let token = Token::new(FILE_TOKEN).map_err(|error| error.to_string())?;
    let configuration = Config::from_bytes(
        format!(
            "[apps.libreoffice-writer]\nenabled = true\nbridge_port = {port}\n\
             [apps.libreoffice-calc]\nenabled = true\nbridge_port = {port}\n\
             [apps.libreoffice-impress]\nenabled = true\nbridge_port = {port}\n",
            port = bridge.port
        )
        .as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    let registry = Registry::from_config(&configuration, &token);
    let outcome = (|| -> Verdict {
        for target in LIBREOFFICE_TARGETS {
            let module = registry
                .get(target)
                .ok_or_else(|| format!("{target}: not in the registry"))?;
            let health = module.health();
            ensure!(
                health.connected,
                "{target}: health: the bridge is not reachable"
            );
            ensure!(
                health.app_version.as_deref() == Some("unknown")
                    && health.bridge_version.as_deref() == Some("1"),
                "{target}: health: {health:?}"
            );
            let state = module
                .state(&captures.join(format!("{target}.png")), &work_dir)
                .map_err(|error| format!("{target}: state: {error}"))?;
            ensure!(
                state.state == json!({}),
                "{target}: state without LibreOffice: {state:?}"
            );
            ensure!(
                state
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("cannot be imported")),
                "{target}: state warnings: {:?}",
                state.warnings
            );
            ensure!(
                state.screenshot.is_none(),
                "{target}: unexpected capture {state:?}"
            );
            let reply = module
                .execute("result = 1", 10, &work_dir)
                .map_err(|error| format!("{target}: execute: {error}"))?;
            ensure!(
                !reply.ok,
                "{target}: a script ran without LibreOffice: {reply:?}"
            );
            ensure!(
                reply.error["type"] == "LibreOfficeUnavailable",
                "{target}: execute error: {reply:?}"
            );
        }
        Ok(())
    })();
    registry.close_all();
    let said = bridge.said();
    ensure!(
        !said.iter().any(|line| line.contains(FILE_TOKEN)),
        "the bridge printed the token"
    );
    outcome
}

#[test]
fn the_hub_drives_the_generated_libreoffice_bridge_for_its_three_targets_without_libreoffice() {
    must_pass(
        check_the_hub_reaches_the_libreoffice_bridge_without_libreoffice(&generated(
            "uno_bridge.py",
        )),
    );
}

#[test]
fn counter_proof_a_libreoffice_bridge_announcing_another_application_is_refused() {
    let base = common::private_tempdir();
    let text = fs::read_to_string(generated("uno_bridge.py")).expect("generated bridge");
    let line = "    app = \"libreoffice\"\n";
    assert_eq!(
        text.matches(line).count(),
        1,
        "the defect must change one place"
    );
    let defective = base.path().join("uno_bridge_as_gimp.py");
    fs::write(&defective, text.replacen(line, "    app = \"gimp\"\n", 1)).expect("write");
    must_fail(check_the_hub_reaches_the_libreoffice_bridge_without_libreoffice(&defective));
}
