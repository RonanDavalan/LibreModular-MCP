//! The real `libremodular-mcp` binary, started in private temporary directories.
//!
//! Why this module exists
//! The standard output belongs to the protocol and the Hub refuses to start when a
//! security condition cannot be verified: both are only observable on the binary
//! itself. Every test that starts it needs the same private layout, token, command
//! and JSON-RPC line, so they live here once.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};

use serde_json::{Value, json};

use super::write_file;

pub const BINARY: &str = env!("CARGO_BIN_EXE_libremodular-mcp");

pub struct Layout {
    pub config: PathBuf,
    pub state: PathBuf,
    pub runtime: PathBuf,
    pub home: PathBuf,
}

pub fn layout(base: &Path) -> Layout {
    let make = |name: &str| {
        let path = base.join(name);
        fs::create_dir_all(&path).expect("directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("mode");
        path
    };
    Layout {
        config: make("config"),
        state: make("state"),
        runtime: make("runtime"),
        home: make("home"),
    }
}

/// Write a private configuration directory with a token and `config_text`.
pub fn prepare(layout: &Layout, token: &str, config_text: &str) {
    let dir = layout.config.join("libremodular-mcp");
    fs::create_dir_all(&dir).expect("config dir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("mode");
    write_file(&dir.join("token"), 0o600, token);
    write_file(&dir.join("config.toml"), 0o600, config_text);
}

pub fn command(layout: &Layout, with_runtime: bool) -> Command {
    let mut command = Command::new(BINARY);
    command
        .env_clear()
        .env("HOME", &layout.home)
        .env("LIBREMODULAR_CONFIG_DIR", &layout.config)
        .env("LIBREMODULAR_STATE_DIR", &layout.state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if with_runtime {
        command.env("XDG_RUNTIME_DIR", &layout.runtime);
    }
    command
}

pub fn rpc(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// An MCP client driving a running `libremodular-mcp serve`, one request at a time.
pub struct McpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    lines: Vec<String>,
    next_id: u64,
}

impl McpProcess {
    /// Start the binary with `command` and open the session.
    pub fn start(mut command: Command) -> McpProcess {
        let mut child = command.spawn().expect("start the Hub");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut process = McpProcess {
            child,
            stdin,
            stdout,
            lines: Vec::new(),
            next_id: 1,
        };
        let init = process.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "e2e", "version": "0"}}),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "libremodular-mcp");
        writeln!(
            process.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .expect("write");
        process.stdin.flush().expect("flush");
        process
    }

    /// Send a request and return the whole response line, parsed.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.stdin, "{}", rpc(id, method, params)).expect("write");
        self.stdin.flush().expect("flush");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read");
        self.lines.push(line.clone());
        serde_json::from_str(&line).expect("a protocol line")
    }

    /// Call a tool and return the JSON object its answer carries as text.
    pub fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let response = self.request("tools/call", json!({"name": tool, "arguments": arguments}));
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text in {response}"));
        serde_json::from_str(text).expect("a JSON answer")
    }

    /// Close the session and wait for the process; returns what it wrote besides
    /// the lines already read.
    pub fn finish(self) -> (Output, Vec<String>) {
        let McpProcess {
            child,
            stdin,
            mut stdout,
            mut lines,
            ..
        } = self;
        drop(stdin);
        let output = child.wait_with_output().expect("wait");
        let mut rest = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut rest);
        lines.extend(rest.lines().map(str::to_owned));
        (output, lines)
    }
}
