//! La recette à froid sur le vrai binaire : `init`, puis une session de quatre appels.
//!
//! Pourquoi ce module existe
//! Chaque partie du Hub est testée seule ; seul le binaire compilé montre que ce qu'écrit `init` est ce
//! que lit `serve`, qu'une session laisse exactement une ligne de journal par appel et aucun texte de
//! script, et que le jeton ne quitte jamais son fichier. Le processus reçoit un environnement vidé
//! fait de `HOME` et des trois répertoires `LIBREMODULAR_*`, tous privés et temporaires, jamais ceux
//! de l'utilisateur.

#[macro_use]
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::binary::{BINARY, McpProcess, command, layout};
use common::fake_bridge::{FakeBridge, Handler, HelloMode, Reply, handler};
use common::{Verdict, mode_of, must_fail, must_pass, write_file};
use serde_json::{Value, json};

const SUCCESS: &str = "print('smoke-script-ok')\nresult = 1\n";
const FAILURE: &str = "print(smoke_undefined_name)\n";
const DELETION: &str = "import os\nos.remove('/home/alice/work/old.txt')\n";
const OTHER_APPLICATION: &str = "print('smoke-script-unknown-target')\n";
/// Morceaux des scripts qui ne doivent jamais atteindre le journal.
const SCRIPT_PIECES: [&str; 4] = [
    "smoke-script-ok",
    "smoke_undefined_name",
    "os.remove",
    "smoke-script-unknown-target",
];

/// Le pont : il exécute les deux premiers scripts comme le ferait une application, et n'est jamais
/// atteint par les deux autres.
fn application() -> Handler {
    handler(|request| {
        let script = request.get("script").and_then(Value::as_str).unwrap_or("");
        if script == SUCCESS {
            Reply::Json(json!({
                "ok": true, "stdout": "smoke-script-ok\n", "stderr": "",
                "result": 1, "error": null, "duration_ms": 1,
            }))
        } else {
            Reply::Json(json!({
                "ok": false, "stdout": "", "stderr": "", "result": null,
                "error": {
                    "type": "NameError",
                    "message": "name 'smoke_undefined_name' is not defined",
                    "traceback": "Traceback (most recent call last):\n  File \"<libremodular>\", line 1, in <module>\nNameError: name 'smoke_undefined_name' is not defined\n",
                },
                "duration_ms": 1,
            }))
        }
    })
}

/// Exécute `init` avec le vrai binaire ; rend ce qu'il a écrit sur ses deux flux.
fn run_init(layout: &common::binary::Layout) -> Result<(String, String), String> {
    let output = Command::new(BINARY)
        .arg("init")
        .env_clear()
        .env("HOME", &layout.home)
        .env("LIBREMODULAR_CONFIG_DIR", &layout.config)
        .env("LIBREMODULAR_STATE_DIR", &layout.state)
        .env("LIBREMODULAR_RUNTIME_DIR", &layout.runtime)
        .output()
        .map_err(|error| format!("step init: cannot start the binary: {error}"))?;
    let (stdout, stderr) = (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    );
    ensure!(
        output.status.success(),
        "step init: exit status {:?}: {stderr}",
        output.status
    );
    Ok((stdout, stderr))
}

/// Chaque fichier sous `base`, avec son chemin.
fn files_under(base: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, found);
            } else {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(base, &mut found);
    found
}

/// Un défaut mis exprès dans la mise en place, pour montrer que les contrôles de la recette peuvent
/// échouer : le Hub n'est pas défectueux, la mise en place lui fait faire ce que les contrôles interdisent.
#[derive(Clone, Copy, Default)]
struct Defect {
    /// L'opérateur a demandé au journal de garder le texte des scripts.
    journal_keeps_script_text: bool,
    /// Quelque chose a copié le jeton dans un fichier du répertoire d'état.
    token_copied_into_state: bool,
}

fn drive(base: &Path, defect: Defect) -> Verdict {
    let layout = layout(base);
    let config_dir = layout.config.join("libremodular-mcp");
    let token_file = config_dir.join("token");

    // Étape init : le vrai binaire crée la configuration et le jeton.
    let (init_out, init_err) = run_init(&layout)?;
    let token = fs::read_to_string(&token_file)
        .map_err(|error| format!("step init: no token file: {error}"))?;
    ensure!(
        !init_out.contains(token.trim()) && !init_err.contains(token.trim()),
        "step init: the token was printed"
    );
    ensure!(
        mode_of(&token_file) == 0o600,
        "step init: the token file is not mode 600"
    );

    // Étape configure : activer `gimp` sur le port du faux pont.
    let bridge = FakeBridge::start(token.trim(), HelloMode::Normal, application());
    let config_file = config_dir.join("config.toml");
    let example = fs::read_to_string(&config_file)
        .map_err(|error| format!("step configure: no configuration: {error}"))?;
    let enabled = example.replacen(
        "[apps.gimp]\nenabled = false\nmode = \"tcp\"\nport = 9301",
        &format!(
            "[apps.gimp]\nenabled = true\nmode = \"tcp\"\nport = {}",
            bridge.port()
        ),
        1,
    );
    ensure!(
        enabled != example,
        "step configure: the example configuration has no gimp entry to enable"
    );
    let enabled = if defect.journal_keeps_script_text {
        enabled.replacen("# audit_script_text = false", "audit_script_text = true", 1)
    } else {
        enabled
    };
    write_file(&config_file, 0o600, &enabled);
    if defect.token_copied_into_state {
        write_file(&layout.state.join("copy.txt"), 0o600, &token);
    }

    // Étape serve : une session, quatre appels.
    let mut serve = command(&layout, false);
    serve
        .arg("serve")
        .env("LIBREMODULAR_RUNTIME_DIR", &layout.runtime);
    let mut session = McpProcess::start(serve);
    let ran = session.call(
        "execute_script",
        json!({"target_app": "gimp", "script_content": SUCCESS}),
    );
    ensure!(
        ran["status"] == "ok" && ran["stdout"] == "smoke-script-ok\n" && ran["result"] == 1,
        "step script that succeeds: {ran}"
    );
    let failed = session.call(
        "execute_script",
        json!({"target_app": "gimp", "script_content": FAILURE}),
    );
    ensure!(
        failed["status"] == "error"
            && failed["error"]["type"] == "NameError"
            && failed["error"]["traceback"]
                .as_str()
                .is_some_and(|trace| trace.contains("File \"<libremodular>\", line 1")),
        "step script that raises: {failed}"
    );
    let refused = session.call(
        "execute_script",
        json!({"target_app": "gimp", "script_content": DELETION}),
    );
    ensure!(
        refused["status"] == "refused"
            && refused["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("G-1")),
        "step script refused by the guard: {refused}"
    );
    let unknown = session.call(
        "execute_script",
        json!({"target_app": "photoshop", "script_content": OTHER_APPLICATION}),
    );
    ensure!(
        unknown["status"] == "refused",
        "step unknown target: {unknown}"
    );
    let (output, lines) = session.finish();
    ensure!(
        output.status.success(),
        "step serve: exit status {:?}",
        output.status
    );
    for line in &lines {
        let message: Value = serde_json::from_str(line)
            .map_err(|_| "step serve: something other than the protocol on stdout".to_owned())?;
        ensure!(
            message["jsonrpc"] == "2.0",
            "step serve: not a JSON-RPC line"
        );
    }
    ensure!(
        bridge.scripts() == vec![SUCCESS.to_owned(), FAILURE.to_owned()],
        "step bridge: it received {} script(s) instead of the first two",
        bridge.scripts().len()
    );

    // Étape journal : quatre lignes, dans l'ordre, et aucun texte de script.
    let journal_file = layout.state.join("libremodular-mcp").join("audit.jsonl");
    let journal = fs::read_to_string(&journal_file)
        .map_err(|error| format!("step journal: cannot read it: {error}"))?;
    let rows: Vec<Value> = journal
        .lines()
        .map(|line| serde_json::from_str(line).map_err(|_| "step journal: a line is not JSON"))
        .collect::<Result<_, _>>()?;
    ensure!(rows.len() == 4, "step journal: {} lines, not 4", rows.len());
    let statuses: Vec<&str> = rows
        .iter()
        .filter_map(|row| row["status"].as_str())
        .collect();
    ensure!(
        statuses == ["ok", "error", "refused", "refused"],
        "step journal: statuses {statuses:?}"
    );
    let targets: Vec<&str> = rows
        .iter()
        .filter_map(|row| row["target_app"].as_str())
        .collect();
    ensure!(
        targets == ["gimp", "gimp", "gimp", "photoshop"],
        "step journal: targets {targets:?}"
    );
    ensure!(
        rows.iter().all(|row| row["tool"] == "execute_script"
            && row["script_sha256"]
                .as_str()
                .is_some_and(|hash| hash.len() == 64)),
        "step journal: a line lacks its tool or the hash of its script"
    );
    for piece in SCRIPT_PIECES {
        ensure!(
            !journal.contains(piece),
            "step journal: the text of a script is in the journal ({piece})"
        );
    }
    ensure!(
        mode_of(&journal_file) == 0o600,
        "step journal: the journal is not mode 600"
    );

    // Étape jeton : le jeton est dans son propre fichier et nulle part ailleurs, ni dans aucune sortie.
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure!(
        !stderr.contains(token.trim()),
        "step token: it reached the error stream"
    );
    ensure!(
        !lines.iter().any(|line| line.contains(token.trim())),
        "step token: it reached the standard output"
    );
    for path in files_under(base) {
        if path == token_file {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| format!("step token: {error}"))?;
        ensure!(
            !bytes
                .windows(token.trim().len())
                .any(|window| window == token.trim().as_bytes()),
            "step token: it is in {}",
            path.strip_prefix(base).unwrap_or(&path).display()
        );
    }
    Ok(())
}

#[test]
fn init_then_a_session_of_four_calls_leaves_four_journal_lines_and_the_token_nowhere_else() {
    let base = common::private_tempdir();
    must_pass(drive(base.path(), Defect::default()));
}

#[test]
fn counter_proof_a_journal_that_keeps_the_script_text_is_detected() {
    let base = common::private_tempdir();
    let verdict = drive(
        base.path(),
        Defect {
            journal_keeps_script_text: true,
            ..Defect::default()
        },
    );
    assert!(
        verdict
            .as_ref()
            .is_err_and(|message| message.contains("the text of a script is in the journal")),
        "{verdict:?}"
    );
    must_fail(verdict);
}

#[test]
fn counter_proof_a_token_copied_into_another_file_is_detected() {
    let base = common::private_tempdir();
    let verdict = drive(
        base.path(),
        Defect {
            token_copied_into_state: true,
            ..Defect::default()
        },
    );
    assert!(
        verdict
            .as_ref()
            .is_err_and(|message| message.contains("step token: it is in")),
        "{verdict:?}"
    );
    must_fail(verdict);
}
