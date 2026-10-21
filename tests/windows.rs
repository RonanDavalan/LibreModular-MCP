//! Ce que le Hub garantit sous Windows : fichiers privés à l'utilisateur, arbre de processus borné,
//! emplacements, lanceur, garde de motifs, et un Inkscape réel quand il est installé.
//!
//! Pourquoi ce fichier existe
//! Les essais de modes de fichier, de groupes de processus et de variables XDG sont ceux d'Unix et ne
//! s'y compilent pas ; chaque garantie qu'ils portent a ici son équivalent, avec sa contre-épreuve :
//! l'état sain passe, le même état élargi est refusé.

#![cfg(windows)]

mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use libremodular_mcp::config::{self, Paths};
use libremodular_mcp::launch::{self, Programs};
use libremodular_mcp::modules::inkscape::{ProcessSpec, RunError, run};
use libremodular_mcp::security::{
    GuardPolicy, SecurityError, check_trusted_file_as, create_token, ensure_private_dir_as,
    guard_script, harden_process, load_token_as, read_control_file_as, resolve_under,
};
use serde_json::json;

use common::current_uid;

/// Ajoute à `path` un droit pour tout le monde (`*S-1-1-0`, indépendant de la langue du système).
fn grant_everyone(path: &Path, right: &str) {
    let output = Command::new("icacls")
        .arg(path)
        .args(["/grant", &format!("*S-1-1-0:{right}")])
        .output()
        .expect("icacls");
    assert!(output.status.success(), "icacls: {output:?}");
}

fn token_path(base: &Path) -> std::path::PathBuf {
    base.join("config").join("token")
}

// --- le jeton et les fichiers de contrôle -----------------------------------------------------

#[test]
fn a_token_is_created_private_and_read_back() {
    harden_process().expect("hardening cannot fail");
    let base = common::private_tempdir();
    let path = token_path(base.path());
    create_token(&path, false).expect("create");
    let token = load_token_as(&path, current_uid()).expect("a token made by this process loads");
    assert_eq!(token.as_str().len(), 64);
    assert!(
        matches!(
            create_token(&path, false),
            Err(SecurityError::TokenExists { .. })
        ),
        "a second init does not overwrite the token"
    );
}

#[test]
fn a_token_that_every_account_can_read_is_refused() {
    let base = common::private_tempdir();
    let path = token_path(base.path());
    create_token(&path, false).expect("create");
    assert!(load_token_as(&path, current_uid()).is_ok());
    grant_everyone(&path, "(R)");
    let error = load_token_as(&path, current_uid()).expect_err("shared token");
    assert!(matches!(error, SecurityError::LooseAcl { .. }), "{error:?}");
    let message = error.to_string();
    assert!(
        message.contains("token") && message.contains("icacls") && !message.contains("0000"),
        "{message}"
    );
}

#[test]
fn a_file_of_another_owner_is_refused() {
    let base = common::private_tempdir();
    let path = token_path(base.path());
    create_token(&path, false).expect("create");
    let error = load_token_as(&path, current_uid() + 1).expect_err("foreign owner");
    assert!(
        matches!(error, SecurityError::ForeignOwner { .. }),
        "{error:?}"
    );
    let error = read_control_file_as(&path, 8, current_uid()).expect_err("too large");
    assert!(matches!(error, SecurityError::TooLarge { .. }), "{error:?}");
}

#[test]
fn a_private_directory_is_created_and_a_shared_one_is_refused() {
    let base = common::private_tempdir();
    let nested = base.path().join("a").join("b").join("c");
    ensure_private_dir_as(&nested, current_uid()).expect("created with its parents");
    ensure_private_dir_as(&nested, current_uid()).expect("checked again");
    grant_everyone(&nested, "(OI)(CI)(R)");
    let error = ensure_private_dir_as(&nested, current_uid()).expect_err("shared directory");
    assert!(matches!(error, SecurityError::LooseAcl { .. }), "{error:?}");
    assert!(matches!(
        ensure_private_dir_as(&nested, current_uid() + 1),
        Err(SecurityError::ForeignOwner { .. }) | Err(SecurityError::LooseAcl { .. })
    ));
}

#[test]
fn a_trusted_file_may_be_read_by_all_but_not_written_by_them() {
    let base = common::private_tempdir();
    let bridge = base.path().join("bridge.py");
    fs::write(&bridge, "print('bridge')\n").expect("write");
    check_trusted_file_as(&bridge, current_uid()).expect("a file of the user is trusted");
    grant_everyone(&bridge, "(R)");
    check_trusted_file_as(&bridge, current_uid()).expect("readable by all is still trusted");
    grant_everyone(&bridge, "(W)");
    let error = check_trusted_file_as(&bridge, current_uid()).expect_err("writable by all");
    assert!(matches!(error, SecurityError::LooseAcl { .. }), "{error:?}");
    assert!(matches!(
        check_trusted_file_as(Path::new("bridge.py"), current_uid()),
        Err(SecurityError::InvalidPath { .. })
    ));
}

#[test]
fn a_junction_that_leaves_the_zone_is_refused() {
    let base = common::private_tempdir();
    let zone = base.path().join("zone");
    let outside = base.path().join("outside");
    fs::create_dir_all(&zone).expect("zone");
    fs::create_dir_all(&outside).expect("outside");
    let link = zone.join("link");
    let made = Command::new("cmd")
        .arg("/c")
        .arg("mklink")
        .arg("/J")
        .arg(&link)
        .arg(&outside)
        .output()
        .expect("mklink");
    assert!(made.status.success(), "{made:?}");
    let error = resolve_under(&zone, &link.join("file.txt")).expect_err("escapes by the junction");
    assert!(
        matches!(error, SecurityError::PathEscapes { .. }),
        "{error:?}"
    );
    assert!(resolve_under(&zone, &zone.join("new").join("file.txt")).is_ok());
}

// --- les emplacements ------------------------------------------------------------------------

fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: BTreeMap<String, OsString> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
        .collect();
    move |name| map.get(name).cloned()
}

#[test]
fn the_locations_follow_local_application_data() {
    let paths = config::resolve_paths(lookup(&[("LOCALAPPDATA", r"C:\Users\alice\AppData\Local")]))
        .expect("paths");
    let root = Path::new(r"C:\Users\alice\AppData\Local\libremodular-mcp");
    assert_eq!(paths.config_dir, root.join("config"));
    assert_eq!(paths.state_dir, root.join("state"));
    assert_eq!(
        paths.runtime_dir.as_deref(),
        Some(root.join("run").as_path())
    );
    assert_eq!(paths.token_file(), root.join("config").join("token"));
}

#[test]
fn overrides_replace_a_base_and_must_be_absolute() {
    let paths = config::resolve_paths(lookup(&[
        ("LOCALAPPDATA", r"C:\Users\alice\AppData\Local"),
        ("LIBREMODULAR_CONFIG_DIR", r"D:\cfg"),
    ]))
    .expect("paths");
    assert_eq!(paths.config_dir, Path::new(r"D:\cfg\libremodular-mcp"));
    assert!(
        config::resolve_paths(lookup(&[
            ("LOCALAPPDATA", r"C:\Users\alice\AppData\Local"),
            ("LIBREMODULAR_STATE_DIR", "relative"),
        ]))
        .is_err()
    );
    assert!(config::resolve_paths(lookup(&[("LOCALAPPDATA", "relative")])).is_err());
}

#[test]
fn a_missing_local_application_data_gives_no_volatile_directory() {
    let paths = config::resolve_paths(lookup(&[
        ("LIBREMODULAR_CONFIG_DIR", r"D:\cfg"),
        ("LIBREMODULAR_STATE_DIR", r"D:\state"),
    ]))
    .expect("paths");
    assert!(paths.runtime_dir.is_none());
    let error = paths.require_runtime_dir().expect_err("no fallback");
    assert!(error.to_string().contains("LOCALAPPDATA"), "{error}");
}

// --- le lanceur ------------------------------------------------------------------------------

#[test]
fn the_launcher_is_refused_with_its_reason_and_the_exit_code_2() {
    let base = common::private_tempdir();
    let locate = || -> Result<Paths, config::ConfigError> {
        config::resolve_paths(lookup(&[(
            "LOCALAPPDATA",
            base.path().to_str().expect("utf-8"),
        )]))
    };
    for arguments in [
        vec!["libremodular-launch", "libreoffice", "--stop"],
        vec![
            "libremodular-launch",
            "libreoffice",
            "--bridge-file",
            r"C:\x\uno_bridge.py",
        ],
    ] {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = launch::run(
            arguments.into_iter().map(OsString::from),
            &locate,
            &Programs::default(),
            &[],
            &mut out,
            &mut err,
        );
        let said = String::from_utf8(err).expect("utf-8");
        assert_eq!(code, 2, "{said}");
        assert!(
            said.contains("not available on Windows") && said.contains("by hand"),
            "{said}"
        );
    }
}

// --- le délai d'un script du mode cli ---------------------------------------------------------

fn system_variables() -> Vec<(OsString, OsString)> {
    ["SystemRoot", "PATH", "TEMP", "TMP"]
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| ((*name).into(), value)))
        .collect()
}

fn powershell(script: &str, current_dir: &Path) -> ProcessSpec {
    ProcessSpec {
        program: "powershell.exe".into(),
        args: vec!["-NoProfile".into(), "-Command".into(), script.into()],
        current_dir: current_dir.to_path_buf(),
        env: system_variables(),
    }
}

fn processes_with_the_marker(marker: &str) -> usize {
    let query = format!(
        "@(Get-CimInstance Win32_Process -Filter \"Name='PING.EXE'\" | Where-Object {{ $_.CommandLine -like '*{marker}*' }}).Count"
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &query])
        .output()
        .expect("powershell");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(usize::MAX)
}

#[test]
fn a_script_that_runs_too_long_is_killed_with_its_descendants() {
    let base = common::private_tempdir();
    // `ping -n 71` vit 70 secondes ; seul ce test emploie ce nombre.
    let spec = powershell(
        "Start-Process ping -ArgumentList '-n','71','127.0.0.1' -WindowStyle Hidden; Start-Sleep 70",
        base.path(),
    );
    let started = Instant::now();
    let outcome = run(&spec, Duration::from_secs(3), 1 << 20);
    assert!(matches!(outcome, Err(RunError::TimedOut)), "{outcome:?}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the call returned late: {:?}",
        started.elapsed()
    );
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        processes_with_the_marker("-n 71"),
        0,
        "a descendant outlived its script"
    );
}

#[test]
fn a_script_that_finishes_returns_its_output_and_leaves_nothing_behind() {
    let base = common::private_tempdir();
    let spec = powershell("Write-Output 'hello'; exit 3", base.path());
    let finished = run(&spec, Duration::from_secs(30), 1 << 20).expect("runs");
    assert_eq!(finished.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&finished.stdout).contains("hello"));
}

// --- la garde de motifs -----------------------------------------------------------------------

const WORK: &str = r"C:\Users\alice\run\work\gimp";

fn refused_writes(script: &str) -> Vec<String> {
    let policy = GuardPolicy {
        work_dir: WORK,
        extra_write_roots: &[],
        home_dir: Some(r"C:\Users\alice"),
    };
    guard_script(script, &policy)
        .categories()
        .iter()
        .map(|category| format!("{category:?}"))
        .collect()
}

#[test]
fn writes_outside_the_work_directory_are_found_with_windows_paths() {
    for outside in [
        r#"open(r"C:\Windows\evil.txt", "w")"#,
        r#"open("C:/Windows/evil.txt", "w")"#,
        r#"open("C:\\Windows\\evil.txt", "w")"#,
        r#"open(r"\\server\share\evil.txt", "w")"#,
        r#"open("~\\evil.txt", "w")"#,
    ] {
        assert_eq!(
            refused_writes(outside),
            ["WriteOutsideWorkDir"],
            "{outside}"
        );
    }
    for inside in [
        format!(r#"open(r"{WORK}\a.txt", "w")"#),
        format!(r#"open("{}/a.txt", "w")"#, WORK.replace('\\', "/")),
    ] {
        assert!(refused_writes(&inside).is_empty(), "{inside}");
    }
}

// --- le Hub et Inkscape installé --------------------------------------------------------------

fn inkscape_is_installed() -> bool {
    ["ProgramFiles", "LOCALAPPDATA"].iter().any(|variable| {
        std::env::var_os(variable).is_some_and(|base| {
            ["Inkscape", r"Programs\Inkscape"].iter().any(|tail| {
                Path::new(&base)
                    .join(tail)
                    .join(r"bin\inkscape.com")
                    .is_file()
            })
        })
    })
}

#[test]
fn the_hub_drives_the_installed_inkscape_when_it_is_there() {
    if !inkscape_is_installed() {
        eprintln!("Inkscape is not installed: skipped");
        return;
    }
    let base = common::private_tempdir();
    let hub = common::hub::build(
        base.path(),
        common::hub::config("[apps.inkscape]\nenabled = true\n"),
        vec![],
    );
    let mut session = common::hub::Session::start(hub.server.clone());
    let answer = session.execute(
        "inkscape",
        "import json, os\nprint('LMM_RESULT: ' + json.dumps(os.environ['LMM_WORK_DIR']))\n",
    );
    assert_eq!(answer["status"], "ok", "{answer}");
    let work = hub.volatile.join("work").join("inkscape");
    assert_eq!(answer["result"], json!(work.to_string_lossy()));
    let refused = session.execute("inkscape", "import subprocess\n");
    assert_eq!(refused["status"], "refused", "{refused}");
    let state = session.state("inkscape");
    assert_eq!(state["connected"], true, "{state}");
    assert_eq!(
        state["state"]["layers"],
        json!([{"id": "layer1", "label": "Layer 1"}]),
        "{state}"
    );
    assert!(
        state["app_version"]
            .as_str()
            .is_some_and(|version| version.starts_with('1')),
        "{state}"
    );
}
