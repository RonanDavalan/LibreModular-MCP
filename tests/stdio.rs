//! Le binaire `libremodular-mcp`, exécuté comme un client MCP l'exécuterait.
//!
//! Pourquoi ce module existe
//! La sortie standard appartient au protocole : une seule ligne parasite casse le client. Et le
//! serveur doit refuser de démarrer, sans montrer le jeton, quand une condition de sécurité ne peut
//! pas être vérifiée. Les deux ne s'observent que sur le vrai binaire, démarré avec des répertoires
//! temporaires privés.

#![cfg_attr(windows, allow(unused_imports, dead_code, unused_variables))]

#[macro_use]
mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant, SystemTime};

use common::binary::{Layout, command, layout, prepare, rpc};
use common::fake_bridge::FakeBridge;
use common::{FAKE_TOKEN, FILE_TOKEN, Verdict, must_fail, must_pass, write_file};
#[cfg(unix)]
use rustix::process::{Pid, Signal, kill_process};
use serde_json::{Value, json};

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
    // Ce que le client MCP lit des outils (descriptions et schémas) est en anglais : pas une lettre accentuée.
    let listed = tools["result"]["tools"].to_string();
    assert!(listed.is_ascii(), "tool descriptions must be English: {listed}");
    assert!(!listed.contains("Le script"), "{listed}");
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

// --- refuser de démarrer ------------------------------------------------------------

/// Le Hub doit sortir avec le code 1, ne rien afficher sur stdout, expliquer sur stderr et ne
/// jamais montrer le jeton.
fn check_refuses_to_start(layout: &Layout, with_runtime: bool) -> Verdict {
    let mut child = command(layout, with_runtime)
        .spawn()
        .map_err(|e| e.to_string())?;
    // Un client qui ouvre une session : un Hub qui refuse ne doit pas lui répondre.
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

#[cfg(unix)]
#[test]
fn the_hub_refuses_to_start_with_a_token_readable_by_others() {
    let base = common::private_tempdir();
    let layout = layout(base.path());
    prepare(&layout, FILE_TOKEN, "");
    let token = layout.config.join("libremodular-mcp").join("token");
    fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).expect("mode");
    must_pass(check_refuses_to_start(&layout, true));
}

#[cfg(unix)]
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

#[cfg(unix)]
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

// --- s'arrêter sur un signal --------------------------------------------------------

#[cfg(unix)]
/// Démarrer le Hub, attendre qu'il réponde, placer une capture assez ancienne pour être purgée,
/// envoyer `signal`, et exiger que le processus se termine à temps sans la capture et sans montrer
/// le jeton.
fn check_signal_purges_captures(signal: Signal) -> Verdict {
    let base = common::private_tempdir();
    let layout = layout(base.path());
    prepare(&layout, FILE_TOKEN, "");
    let mut child = command(&layout, true).spawn().map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
    let init = rpc(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {},
               "clientInfo": {"name": "e2e", "version": "0"}}),
    );
    writeln!(stdin, "{init}").map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    stdout.read_line(&mut line).map_err(|e| e.to_string())?;
    ensure!(line.contains("\"result\""), "the Hub did not start: {line}");
    // Placée une fois le Hub lancé, pour que seul l'arrêt puisse la retirer.
    let captures = layout.runtime.join("libremodular-mcp").join("captures");
    let capture = captures.join("gimp-old.png");
    write_file(&capture, 0o600, "not really a png");
    let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 3600);
    fs::File::options()
        .write(true)
        .open(&capture)
        .and_then(|file| file.set_modified(two_hours_ago))
        .map_err(|e| e.to_string())?;
    kill_process(Pid::from_child(&child), signal).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the Hub did not stop within 10 seconds".to_owned());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(stdin);
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stderr);
    }
    ensure!(!stderr.contains(FILE_TOKEN), "the token was shown");
    ensure!(
        fs::symlink_metadata(&capture).is_err(),
        "the capture survived the stop ({status:?})"
    );
    ensure!(status.success(), "exit status {status:?}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn sigterm_purges_the_captures_before_the_hub_exits() {
    must_pass(check_signal_purges_captures(Signal::TERM));
}

#[cfg(unix)]
#[test]
fn sigint_purges_the_captures_before_the_hub_exits() {
    must_pass(check_signal_purges_captures(Signal::INT));
}

#[cfg(unix)]
#[test]
fn counter_proof_a_stop_without_the_purge_is_detected() {
    // SIGKILL ne peut pas être traité : le processus se termine comme se terminerait un signal non
    // traité, et le contrôle doit remarquer la capture laissée.
    must_fail(check_signal_purges_captures(Signal::KILL));
}
