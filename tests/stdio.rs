//! The `libremodular-mcp` binary, run as an MCP client would run it.
//!
//! Why this module exists
//! The standard output belongs to the protocol: one stray line breaks the
//! client. And the server must refuse to start, without showing the token, when
//! a security condition cannot be verified. Both are only observable on the
//! real binary, started with private temporary directories.

#[macro_use]
mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::fake_bridge::FakeBridge;
use common::{FAKE_TOKEN, FILE_TOKEN, Verdict, must_fail, must_pass, write_file};
use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_libremodular-mcp");

struct Layout {
    config: PathBuf,
    state: PathBuf,
    runtime: PathBuf,
    home: PathBuf,
}

fn layout(base: &Path) -> Layout {
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
fn prepare(layout: &Layout, token: &str, config_text: &str) {
    let dir = layout.config.join("libremodular-mcp");
    fs::create_dir_all(&dir).expect("config dir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("mode");
    write_file(&dir.join("token"), 0o600, token);
    write_file(&dir.join("config.toml"), 0o600, config_text);
}

fn command(layout: &Layout, with_runtime: bool) -> Command {
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

fn rpc(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

#[test]
fn a_full_session_on_stdio_writes_nothing_but_the_protocol() {
    let base = common::private_tempdir();
    let layout = layout(base.path());
    let bridge = FakeBridge::echo(FILE_TOKEN);
    prepare(
        &layout,
        FILE_TOKEN,
        &format!("[apps.gimp]\nenabled = true\nport = {}\n", bridge.port()),
    );
    let mut child = command(&layout, true).spawn().expect("start the Hub");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut lines = Vec::new();
    let mut exchange = |message: String, expect_answer: bool| -> Option<Value> {
        writeln!(stdin, "{message}").expect("write");
        stdin.flush().expect("flush");
        if !expect_answer {
            return None;
        }
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read");
        lines.push(line.clone());
        Some(serde_json::from_str(&line).expect("a protocol line"))
    };
    let init = exchange(
        rpc(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "e2e", "version": "0"}}),
        ),
        true,
    )
    .expect("answer");
    assert_eq!(init["result"]["serverInfo"]["name"], "libremodular-mcp");
    exchange(
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        false,
    );
    let tools = exchange(rpc(2, "tools/list", json!({})), true).expect("answer");
    let mut names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["execute_script", "get_state"]);
    let call = exchange(
        rpc(
            3,
            "tools/call",
            json!({"name": "execute_script",
                   "arguments": {"target_app": "gimp", "script_content": "print('hi')"}}),
        ),
        true,
    )
    .expect("answer");
    let answer: Value =
        serde_json::from_str(call["result"]["content"][0]["text"].as_str().expect("text"))
            .expect("tool answer");
    assert_eq!(answer["status"], "ok", "{answer}");
    let hub = exchange(
        rpc(
            4,
            "tools/call",
            json!({"name": "get_state", "arguments": {"target_app": "hub"}}),
        ),
        true,
    )
    .expect("answer");
    assert!(
        hub["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("\"gimp\"")
    );
    drop(stdin);
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "{:?}", output.status);
    let mut rest = String::new();
    let _ = std::io::Read::read_to_string(&mut stdout, &mut rest);
    let all: Vec<String> = lines
        .into_iter()
        .chain(rest.lines().map(str::to_owned))
        .collect();
    for line in &all {
        let message: Value = serde_json::from_str(line).expect("only protocol on stdout");
        assert_eq!(message["jsonrpc"], "2.0", "{line}");
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(FILE_TOKEN));
    assert!(!all.iter().any(|line| line.contains(FILE_TOKEN)));
    let journal = fs::read_to_string(layout.state.join("libremodular-mcp").join("audit.jsonl"))
        .expect("journal");
    assert_eq!(journal.lines().count(), 2);
    assert_eq!(bridge.scripts(), vec!["print('hi')".to_owned()]);
}

// --- refusing to start -------------------------------------------------------------

/// The Hub must exit with code 1, print nothing on stdout, explain on stderr and
/// never show the token.
fn check_refuses_to_start(layout: &Layout, with_runtime: bool) -> Verdict {
    let mut child = command(layout, with_runtime)
        .spawn()
        .map_err(|e| e.to_string())?;
    // A client that starts a session: a Hub that refuses must not answer it.
    if let Some(mut stdin) = child.stdin.take() {
        let hello = rpc(
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "e2e", "version": "0"}}),
        );
        let _ = writeln!(stdin, "{hello}");
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.code() == Some(1),
        "exit status {:?}",
        output.status
    );
    ensure!(output.stdout.is_empty(), "something was written on stdout");
    ensure!(!stderr.trim().is_empty(), "no explanation on stderr");
    ensure!(!stderr.contains(FILE_TOKEN), "the token was shown");
    Ok(())
}

#[test]
fn the_hub_refuses_to_start_without_a_token() {
    let base = common::private_tempdir();
    let layout = layout(base.path());
    must_pass(check_refuses_to_start(&layout, true));
}

#[test]
fn the_hub_refuses_to_start_with_a_token_readable_by_others() {
    let base = common::private_tempdir();
    let layout = layout(base.path());
    prepare(&layout, FILE_TOKEN, "");
    let token = layout.config.join("libremodular-mcp").join("token");
    fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).expect("mode");
    must_pass(check_refuses_to_start(&layout, true));
}

#[test]
fn the_hub_refuses_to_start_with_a_volatile_directory_open_to_others() {
    let base = common::private_tempdir();
    let layout = layout(base.path());
    prepare(&layout, FILE_TOKEN, "");
    let volatile = layout.runtime.join("libremodular-mcp");
    fs::create_dir(&volatile).expect("volatile");
    fs::set_permissions(&volatile, fs::Permissions::from_mode(0o755)).expect("mode");
    must_pass(check_refuses_to_start(&layout, true));
}

#[test]
fn the_hub_refuses_to_start_without_xdg_runtime_dir_and_never_falls_back_to_tmp() {
    let base = common::private_tempdir();
    let layout = layout(base.path());
    prepare(&layout, FILE_TOKEN, "");
    must_pass(check_refuses_to_start(&layout, false));
}

#[test]
fn counter_proof_a_sound_setup_is_not_reported_as_a_refusal() {
    let base = common::private_tempdir();
    let layout = layout(base.path());
    prepare(&layout, FILE_TOKEN, "");
    must_fail(check_refuses_to_start(&layout, true));
}

#[test]
fn a_malformed_token_file_is_refused_without_quoting_it() {
    let base = common::private_tempdir();
    let layout = layout(base.path());
    prepare(&layout, &format!("{FAKE_TOKEN}{FILE_TOKEN}"), "");
    must_pass(check_refuses_to_start(&layout, true));
}
