//! Invariants mécaniques du code, rejoués à chaque exécution.
//!
//! Pourquoi ce module existe
//! Les propriétés de sécurité qui ne tiennent que par convention pourrissent en silence. Chaque
//! test ici lit le code (ou l'exécute dans un processus neuf) et échoue quand une règle est
//! rompue, et chacun apporte une contre-preuve qui montre que la règle serait remarquée.

#[macro_use]
mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{FAKE_TOKEN, FILE_TOKEN, Verdict, must_fail, must_pass, write_file};
use libremodular_mcp::audit::{AuditEntry, AuditLog};
use libremodular_mcp::cli::run_init;
use libremodular_mcp::config::{self, Limits, Paths};
use libremodular_mcp::security::{Token, create_token, load_token_as, read_control_file_as};
use rustix::fs::Mode;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit, umask};

// --- lecture du code ----------------------------------------------------------------

fn source_files() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, found: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("source directory")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, root, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path
                    .strip_prefix(root)
                    .expect("inside src")
                    .to_string_lossy()
                    .into_owned();
                found.push((name, fs::read_to_string(&path).expect("source text")));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found
}

fn source_of(name: &str) -> String {
    source_files()
        .into_iter()
        .find(|(file, _)| file == name)
        .map(|(_, text)| text)
        .unwrap_or_else(|| panic!("{name} not found"))
}

/// Le code d'un fichier source sans ses commentaires de ligne entière.
fn code_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect()
}

/// Le texte d'une fonction, de son accolade ouvrante à l'accolade fermante correspondante.
fn function_body(source: &str, name: &str) -> Option<String> {
    let start = [format!("fn {name}("), format!("fn {name}<")]
        .iter()
        .filter_map(|needle| source.find(needle.as_str()))
        .min()?;
    let open = start + source[start..].find('{')?;
    let mut depth = 0_i32;
    for (offset, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(source[open..open + offset + 1].to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

// --- le durcissement du processus précède tout accès au jeton (lu dans la source) ---

fn check_hardening_precedes_token_access(source: &str, function: &str, access: &[&str]) -> Verdict {
    let Some(body) = function_body(source, function) else {
        return Err(format!("function {function} not found"));
    };
    let Some(first_access) = access.iter().filter_map(|call| body.find(call)).min() else {
        return Err(format!("{function} never accesses the token"));
    };
    let Some(hardening) = body.find("harden_process()") else {
        return Err(format!("{function} never hardens the process"));
    };
    ensure!(
        hardening < first_access,
        "{function} reads the token before hardening"
    );
    Ok(())
}

const DEFECTIVE_LOAD: &str = "
pub fn load_token_as(path: &Path) -> Result<Token, SecurityError> {
    let data = read_control_file_as(path, 4096, 0)?;
    harden_process()?;
    Token::new(String::new())
}
";

#[test]
fn hardening_precedes_token_access_in_load_token_create_token_and_init() {
    let security = source_of("security.rs");
    let cli = source_of("cli.rs");
    must_pass(check_hardening_precedes_token_access(
        &security,
        "load_token_as",
        &["read_control_file_as("],
    ));
    must_pass(check_hardening_precedes_token_access(
        &security,
        "create_token",
        &["open_control_file(", "ensure_private_dir("],
    ));
    must_pass(check_hardening_precedes_token_access(
        &cli,
        "init_steps",
        &["load_token(", "create_token(", "open_control_file("],
    ));
}

#[test]
fn counter_proof_detects_a_late_hardening_call() {
    must_fail(check_hardening_precedes_token_access(
        DEFECTIVE_LOAD,
        "load_token_as",
        &["read_control_file_as("],
    ));
    must_fail(check_hardening_precedes_token_access(
        "fn load_token_as() { read_control_file_as(); }",
        "load_token_as",
        &["read_control_file_as("],
    ));
}

// --- le durcissement du processus précède tout accès au jeton (observé dans un processus neuf) ---

const CHILD_KIND: &str = "LMM_INVARIANT_CHILD";
const CHILD_FILE: &str = "LMM_INVARIANT_FILE";
const CHILD_TEST: &str = "hardening_and_private_modes_hold_in_a_fresh_process";

fn core_limit() -> Rlimit {
    getrlimit(Resource::Core)
}

/// Le corps du processus enfant : rapporter ce qu'il a observé, jamais le jeton.
fn child(kind: &str, target: &Path) {
    match kind {
        "load" | "load-without-hardening" | "create" => {
            let hard = core_limit().maximum;
            setrlimit(
                Resource::Core,
                Rlimit {
                    current: hard,
                    maximum: hard,
                },
            )
            .expect("raise the soft limit");
            let before = core_limit();
            match kind {
                "load" => drop(load_token_as(target, common::current_uid()).expect("load")),
                "create" => create_token(target, false).expect("create"),
                _ => drop(read_control_file_as(target, 4096, common::current_uid()).expect("read")),
            }
            let after = core_limit();
            println!(
                "CHILD: before={:?}/{:?} after={:?}/{:?}",
                before.current, before.maximum, after.current, after.maximum
            );
        }
        "modes" | "modes-without-explicit-modes" => {
            umask(Mode::empty());
            let paths = paths_under(target);
            assert_eq!(run_init(&paths, false, &mut Vec::new(), &mut Vec::new()), 0);
            umask(Mode::empty());
            let log = AuditLog::new(
                paths.audit_file(),
                Token::new(FAKE_TOKEN).expect("token"),
                Limits::default(),
            )
            .with_rotation(200, 5);
            for _ in 0..8 {
                log.record(
                    &AuditEntry {
                        tool: Some("get_state".to_owned()),
                        ..AuditEntry::default()
                    },
                    None,
                )
                .expect("record");
            }
            umask(Mode::empty());
            assert_eq!(run_init(&paths, true, &mut Vec::new(), &mut Vec::new()), 0);
            if kind == "modes-without-explicit-modes" {
                umask(Mode::empty());
                fs::write(paths.state_dir.join("leaky.txt"), "x").expect("defective write");
                fs::create_dir(paths.state_dir.join("leaky-dir")).expect("defective mkdir");
            }
            let mut leaked = Vec::new();
            let mut stack = vec![target.to_path_buf()];
            let mut seen_rotated = false;
            while let Some(dir) = stack.pop() {
                for entry in fs::read_dir(&dir).expect("list") {
                    let path = entry.expect("entry").path();
                    let info = fs::symlink_metadata(&path).expect("stat");
                    seen_rotated |= path.to_string_lossy().ends_with("audit.jsonl.1");
                    if info.mode() & 0o077 != 0 {
                        leaked.push(format!("{:o} {}", info.mode() & 0o7777, path.display()));
                    }
                    if info.is_dir() {
                        stack.push(path);
                    }
                }
            }
            println!("CHILD: leaked={} rotated={seen_rotated}", leaked.len());
        }
        other => panic!("unknown child kind {other}"),
    }
}

fn paths_under(root: &Path) -> Paths {
    let variables: BTreeMap<&str, OsString> = BTreeMap::from([
        ("HOME", OsString::from(root)),
        ("LIBREMODULAR_CONFIG_DIR", root.join("c").into_os_string()),
        ("LIBREMODULAR_STATE_DIR", root.join("s").into_os_string()),
    ]);
    config::resolve_paths(|name| variables.get(name).cloned()).expect("paths")
}

/// Exécute ce binaire de test lui-même dans un nouveau processus, en mode enfant, et rend ce qu'il a rapporté.
fn run_child(kind: &str, target: &Path) -> Result<String, String> {
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD_KIND, kind)
        .env(CHILD_FILE, target)
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    ensure!(
        output.status.success(),
        "the child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    text.lines()
        .find_map(|line| line.split_once("CHILD: "))
        .map(|(_, report)| report.to_owned())
        .ok_or_else(|| "the child reported nothing".to_owned())
}

fn check_the_core_limit_is_zero_afterwards(kind: &str, base: &Path) -> Verdict {
    let dir = common::fresh(base);
    let target = if kind == "create" {
        dir.join("cfg").join("token")
    } else {
        let token = dir.join("token");
        write_file(&token, 0o600, FILE_TOKEN);
        token
    };
    let report = run_child(kind, &target)?;
    ensure!(
        !report.contains("before=Some(0)/Some(0)"),
        "the limit could not be observed: {report}"
    );
    let (before, after) = report.split_once(" after=").ok_or("unexpected report")?;
    let before = before.trim_start_matches("before=");
    let (soft, hard) = before.split_once('/').ok_or("unexpected report")?;
    ensure!(
        soft == hard,
        "the child did not start with the soft limit raised: {report}"
    );
    ensure!(
        after == "Some(0)/Some(0)",
        "the core limit after the call is {after}, not zero"
    );
    Ok(())
}

fn check_created_files_have_no_group_or_other_bit(kind: &str, base: &Path) -> Verdict {
    let dir = common::fresh(base);
    let report = run_child(kind, &dir)?;
    ensure!(
        report.contains("rotated=true"),
        "the journal was never rotated: {report}"
    );
    ensure!(
        report.contains("leaked=0"),
        "files or directories with group/other bits: {report}"
    );
    Ok(())
}

#[test]
fn hardening_and_private_modes_hold_in_a_fresh_process() {
    if let Ok(kind) = std::env::var(CHILD_KIND) {
        let target = PathBuf::from(std::env::var_os(CHILD_FILE).expect("target path"));
        child(&kind, &target);
        return;
    }
    let base = common::private_tempdir();
    // Charger et créer le jeton abaissent la limite des vidages mémoire ; le lire sans durcissement
    // ne l'abaisse pas, et c'est ce sur quoi s'appuie la contre-preuve.
    must_pass(check_the_core_limit_is_zero_afterwards("load", base.path()));
    must_pass(check_the_core_limit_is_zero_afterwards(
        "create",
        base.path(),
    ));
    must_fail(check_the_core_limit_is_zero_afterwards(
        "load-without-hardening",
        base.path(),
    ));
    // Tout ce que le Hub crée est privé même quand le umask du processus est nul.
    must_pass(check_created_files_have_no_group_or_other_bit(
        "modes",
        base.path(),
    ));
    must_fail(check_created_files_have_no_group_or_other_bit(
        "modes-without-explicit-modes",
        base.path(),
    ));
}

// --- la logique de sécurité ne vit que dans security.rs -----------------------------

const SECURITY_ONLY_DEFINITIONS: [&str; 14] = [
    "guard_script",
    "resolve_under",
    "check_target",
    "check_script_size",
    "bounded",
    "harden_process",
    "load_token",
    "create_token",
    "read_control_file",
    "open_control_file",
    "open_regular_nofollow",
    "ensure_private_dir",
    "check_trusted_file",
    "read_system_file",
];

/// Ce qu'un fichier autre que le module de sécurité ne doit pas contenir. Appeler une fonction de
/// sécurité est permis ; en implémenter une ne l'est pas.
fn stray_security_logic(source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for (index, line) in code_lines(source).into_iter().enumerate() {
        let lower = line.to_lowercase();
        if lower.contains("token")
            && ["==", "!=", ".eq(", "ct_eq", "compare"]
                .iter()
                .any(|op| lower.contains(op))
        {
            problems.push(format!("token comparison at line {}", index + 1));
        }
        if line.contains("ruff_python") || line.contains("ruff_text_size") {
            problems.push(format!("pattern analysis at line {}", index + 1));
        }
        for needle in ["canonicalize(", "read_link(", "realpath"] {
            if line.contains(needle) {
                problems.push(format!("path resolution ({needle}) at line {}", index + 1));
            }
        }
        if line.contains("getrandom::") {
            problems.push(format!("token generation at line {}", index + 1));
        }
        for name in SECURITY_ONLY_DEFINITIONS {
            let definition = format!("fn {name}");
            if let Some(at) = line.find(&definition) {
                let next = line[at + definition.len()..].chars().next();
                if matches!(next, Some('(' | '<')) {
                    problems.push(format!("definition of {name} at line {}", index + 1));
                }
            }
        }
    }
    problems
}

#[test]
fn no_security_logic_outside_security_rs() {
    for (name, source) in source_files() {
        if name == "security.rs" {
            continue;
        }
        let problems = stray_security_logic(&source);
        assert!(problems.is_empty(), "{name}: {problems:?}");
    }
}

#[test]
fn counter_proof_detects_stray_logic() {
    let check = |source: &str| -> Verdict {
        let problems = stray_security_logic(source);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    for defective in [
        "fn same(token: &str, other: &str) -> bool { token == other }",
        "fn same(token: &Token, other: &str) -> bool { token.as_str().eq(other) }",
        "use ruff_python_parser::parse_module;",
        "let real = std::fs::canonicalize(path);",
        "let target = std::fs::read_link(path);",
        "fn resolve_under(root: &Path, candidate: &Path) -> PathBuf { candidate.to_path_buf() }",
        "pub fn guard_script(script: &str) -> bool { true }",
        "fn bounded<T>(text: &str) -> String { text.to_owned() }",
        "getrandom::fill(&mut bytes)",
    ] {
        must_fail(check(defective));
    }
    must_pass(check(
        "// token == other is only a comment here\nlet size = bounded_value;",
    ));
}

// --- les fichiers de contrôle ne s'ouvrent que par le contrôle des permissions ------

const RAW_FILE_ACCESS: [&str; 11] = [
    "File::open(",
    "File::create(",
    "File::options(",
    "OpenOptions",
    "fs::write(",
    "fs::read(",
    "fs::read_to_string(",
    "rustix::fs::open",
    "DirBuilder",
    "create_dir(",
    "create_dir_all(",
];

fn raw_file_access(source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for (index, line) in code_lines(source).into_iter().enumerate() {
        for needle in RAW_FILE_ACCESS {
            if line.contains(needle) {
                problems.push(format!("{needle} at line {}", index + 1));
            }
        }
    }
    problems
}

#[test]
fn control_files_are_only_opened_and_written_through_security_rs() {
    for (name, source) in source_files() {
        if name == "security.rs" {
            continue;
        }
        let problems = raw_file_access(&source);
        assert!(problems.is_empty(), "{name}: {problems:?}");
    }
}

#[test]
fn counter_proof_detects_a_raw_file_access() {
    let check = |source: &str| -> Verdict {
        let problems = raw_file_access(source);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    for defective in [
        "std::fs::write(\"audit.jsonl\", line)?;",
        "let mut file = File::create(path)?;",
        "let file = std::fs::OpenOptions::new().append(true).open(path)?;",
        "let text = std::fs::read_to_string(path)?;",
        "std::fs::create_dir_all(path)?;",
        "let fd = rustix::fs::open(path, flags, mode)?;",
    ] {
        must_fail(check(defective));
    }
    must_pass(check(
        "std::fs::rename(&source, &target)?;\nstd::fs::remove_file(&path)?;",
    ));
}

/// À quoi doit ressembler l'unique endroit qui ouvre un fichier : un seul `open`, sans suivre les
/// liens, créant des fichiers en 0600, et chaque répertoire créé privé.
fn open_discipline_problems(source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let opens = code_lines(source)
        .iter()
        .filter(|line| line.contains("rustix::fs::open("))
        .count();
    if opens != 1 {
        problems.push(format!("{opens} places open a file, expected exactly one"));
    }
    match function_body(source, "open_regular_nofollow") {
        None => problems.push("open_regular_nofollow not found".to_owned()),
        Some(body) => {
            if !body.contains("OFlags::NOFOLLOW") {
                problems.push("the single open follows links".to_owned());
            }
            if !body.contains("FileMode::RUSR | FileMode::WUSR") {
                problems.push("created files are not 0600".to_owned());
            }
            if ["GRP", "OTH"].iter().any(|bits| body.contains(bits)) {
                problems.push("a group or other permission bit is named".to_owned());
            }
        }
    }
    let builders = source.matches("DirBuilder::new()").count();
    let private = source.matches(".mode(PRIVATE_DIR_MODE)").count();
    if builders != private {
        problems.push("a directory is created without an explicit private mode".to_owned());
    }
    if !source.contains("const PRIVATE_DIR_MODE: u32 = 0o700;") {
        problems.push("PRIVATE_DIR_MODE is not 0o700".to_owned());
    }
    if !source.contains("const PRIVATE_FILE_MODE: u32 = 0o600;") {
        problems.push("PRIVATE_FILE_MODE is not 0o600".to_owned());
    }
    problems
}

#[test]
fn security_rs_opens_files_only_without_following_links_and_creates_directories_private() {
    let problems = open_discipline_problems(&source_of("security.rs"));
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn counter_proof_a_second_open_or_a_followed_link_or_a_loose_mode_is_detected() {
    let real = source_of("security.rs");
    let check = |source: &str| -> Verdict {
        let problems = open_discipline_problems(source);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    must_pass(check(&real));
    let followed = real.replacen("OFlags::NOFOLLOW", "OFlags::CLOEXEC", 1);
    let second_open =
        format!("{real}\nfn sneaky() {{ let fd = rustix::fs::open(path, flags, mode); }}\n");
    let loose_directory = format!("{real}\nfn leaky() {{ DirBuilder::new().create(path); }}\n");
    let loose_mode = real.replacen(
        "const PRIVATE_DIR_MODE: u32 = 0o700;",
        "const PRIVATE_DIR_MODE: u32 = 0o755;",
        1,
    );
    let wide_files = real.replacen(
        "FileMode::RUSR | FileMode::WUSR",
        "FileMode::RUSR | FileMode::WUSR | FileMode::RGRP",
        1,
    );
    for defective in [
        &followed,
        &second_open,
        &loose_directory,
        &loose_mode,
        &wide_files,
    ] {
        assert_ne!(defective, &real, "the counter-proof must change the source");
        must_fail(check(defective));
    }
}

// --- le serveur démarre durci avant de lire le jeton --------------------------------

#[test]
fn the_server_hardens_the_process_before_reading_the_token() {
    let cli = source_of("cli.rs");
    must_pass(check_hardening_precedes_token_access(
        &cli,
        "prepare_server",
        &["load_token(", "AuditLog::new("],
    ));
    must_fail(check_hardening_precedes_token_access(
        "fn prepare_server() { let token = load_token(&path)?; harden_process()?; }",
        "prepare_server",
        &["load_token("],
    ));
}

// --- chaque réponse d'outil sort par l'unique point de sortie -----------------------

const TOOLS: [&str; 2] = ["execute_script", "get_state"];

/// Chaque outil rend ce que `finish` a produit, et `finish` est le seul endroit du serveur qui
/// sérialise une réponse, après `output::finalize`.
fn single_exit_point(source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for tool in TOOLS {
        match function_body(source, tool) {
            None => problems.push(format!("tool {tool} not found")),
            Some(body) => {
                let code = code_lines(&body).join("\n");
                if !code
                    .trim_end()
                    .trim_end_matches('}')
                    .trim_end()
                    .ends_with(")")
                    || !code.contains("self.finish(")
                {
                    problems.push(format!("{tool} does not end with self.finish(...)"));
                }
            }
        }
    }
    match function_body(source, "finish") {
        None => problems.push("finish not found".to_owned()),
        Some(body) => {
            let finalize = body.find("output::finalize(");
            let to_text = body.find("output::to_text(");
            if !matches!((finalize, to_text), (Some(a), Some(b)) if a < b) {
                problems.push("finish does not finalize before serialising".to_owned());
            }
        }
    }
    let code = code_lines(source).join("\n");
    for needle in [
        "serde_json::to_string",
        "serde_json::to_vec",
        "to_string_pretty",
    ] {
        if code.contains(needle) {
            problems.push(format!(
                "{needle} in the server: an answer could skip finalize"
            ));
        }
    }
    let finishes = code.matches("output::to_text(").count();
    if finishes != 1 {
        problems.push(format!("{finishes} calls to output::to_text, expected one"));
    }
    problems
}

#[test]
fn each_tool_answers_through_the_single_output_point() {
    let problems = single_exit_point(&source_of("server.rs"));
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn counter_proof_a_tool_that_serialises_on_its_own_is_detected() {
    let real = source_of("server.rs");
    let check = |source: &str| -> Verdict {
        let problems = single_exit_point(source);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    must_pass(check(&real));
    let bypass = real.replacen(
        "self.finish(&payload, entry, None)",
        "serde_json::to_string(&payload).unwrap_or_default()",
        1,
    );
    assert_ne!(bypass, real, "the counter-proof must change the source");
    must_fail(check(&bypass));
    let unscrubbed = real.replacen("output::finalize(", "output::skip_finalize(", 1);
    must_fail(check(&unscrubbed));
    must_fail(check(
        "fn finish(&self) -> String { output::to_text(&payload) }\nfn execute_script() { self.finish() }\nfn get_state() { self.finish() }",
    ));
}

// --- le réseau : bouclage seulement, et seulement dans le module du protocole -------

const NETWORK_APIS: [&str; 8] = [
    "std::net",
    "tokio::net",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "UnixListener",
    "UnixStream",
    "socket2",
];

/// L'usage du réseau hors de `protocol.rs`, toute socket d'écoute, et toute adresse autre que
/// 127.0.0.1 écrite dans le code.
fn network_problems(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, source) in files {
        for (index, line) in code_lines(source).into_iter().enumerate() {
            let at = format!("{name}:{}", index + 1);
            if name != "protocol.rs" {
                for api in NETWORK_APIS {
                    if line.contains(api) {
                        problems.push(format!("{at}: {api} outside protocol.rs"));
                    }
                }
            }
            for listening in [
                "TcpListener",
                "UdpSocket",
                "UnixListener",
                "::bind(",
                "listen(",
            ] {
                if line.contains(listening) {
                    problems.push(format!("{at}: {listening} (the Hub never listens)"));
                }
            }
            for other in [
                "0.0.0.0",
                "UNSPECIFIED",
                "Ipv6Addr",
                "\"localhost\"",
                "::1",
                "to_socket_addrs",
                "lookup_host",
            ] {
                if line.contains(other) {
                    problems.push(format!("{at}: {other}"));
                }
            }
            for address in dotted_quads(line) {
                if address != "127.0.0.1" {
                    problems.push(format!("{at}: address {address}"));
                }
            }
            if let Some(start) = line.find("Ipv4Addr::new(") {
                let rest = &line[start..];
                if !rest.starts_with("Ipv4Addr::new(127, 0, 0, 1)") {
                    problems.push(format!("{at}: an address other than 127.0.0.1"));
                }
            }
        }
    }
    problems
}

/// Chaque `a.b.c.d` fait de quatre nombres dans une ligne.
fn dotted_quads(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index].is_ascii_digit()
            && (index == 0 || !chars[index - 1].is_ascii_alphanumeric())
        {
            let mut end = index;
            while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == '.') {
                end += 1;
            }
            let candidate: String = chars[index..end].iter().collect();
            let candidate = candidate.trim_end_matches('.');
            let parts: Vec<&str> = candidate.split('.').collect();
            if parts.len() == 4 && parts.iter().all(|p| !p.is_empty()) {
                found.push(candidate.to_owned());
            }
            index = end.max(index + 1);
        } else {
            index += 1;
        }
    }
    found
}

#[test]
fn the_network_is_only_used_by_protocol_rs_and_only_toward_127_0_0_1() {
    let files = source_files();
    assert!(files.iter().any(|(name, _)| name == "protocol.rs"));
    let problems = network_problems(&files);
    assert!(problems.is_empty(), "{problems:?}");
    let protocol = source_of("protocol.rs");
    assert!(protocol.contains("pub const BRIDGE_ADDRESS: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);"));
}

#[test]
fn counter_proof_stray_network_use_is_detected() {
    let check = |name: &str, source: &str| -> Verdict {
        let problems = network_problems(&[(name.to_owned(), source.to_owned())]);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    for (name, defective) in [
        ("server.rs", "use std::net::TcpStream;"),
        (
            "modules/mod.rs",
            "let s = tokio::net::TcpStream::connect(addr).await;",
        ),
        (
            "protocol.rs",
            "let l = TcpListener::bind(\"127.0.0.1:9301\")?;",
        ),
        (
            "protocol.rs",
            "let l = std::net::UdpSocket::bind(address)?;",
        ),
        (
            "protocol.rs",
            "const BRIDGE_ADDRESS: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);",
        ),
        ("protocol.rs", "let a = \"192.168.1.20:9301\";"),
        ("protocol.rs", "let a = Ipv4Addr::UNSPECIFIED;"),
        ("protocol.rs", "let a = (\"localhost\", 9301);"),
    ] {
        must_fail(check(name, defective));
    }
    must_pass(check(
        "protocol.rs",
        "pub const BRIDGE_ADDRESS: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);\nlet v = \"3.0.4\";",
    ));
}

// --- les modules n'importent jamais les modules de sécurité ni de sortie ------------

fn module_imports(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, source) in files
        .iter()
        .filter(|(name, _)| name.starts_with("modules/"))
    {
        for (index, line) in code_lines(source).into_iter().enumerate() {
            for forbidden in ["crate::security", "crate::output", "security::", "output::"] {
                if line.contains(forbidden) {
                    problems.push(format!("{name}:{}: {forbidden}", index + 1));
                }
            }
        }
    }
    problems
}

#[test]
fn modules_never_import_security_or_output() {
    let files = source_files();
    assert!(files.iter().any(|(name, _)| name == "modules/mod.rs"));
    let problems = module_imports(&files);
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn counter_proof_a_module_importing_security_is_detected() {
    let problems = module_imports(&[(
        "modules/gimp.rs".to_owned(),
        "use crate::security::guard_script;".to_owned(),
    )]);
    assert!(!problems.is_empty());
}

// --- seul le protocole sur la sortie standard ---------------------------------------

fn stdout_writes(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, source) in files.iter().filter(|(name, _)| name != "cli.rs") {
        for (index, line) in code_lines(source).into_iter().enumerate() {
            // `eprintln!` et `eprint!` écrivent sur le flux d'erreur et sont permis.
            let prints = line.matches("println!").count() + line.matches("print!(").count();
            let eprints = line.matches("eprintln!").count() + line.matches("eprint!(").count();
            if prints > eprints {
                problems.push(format!("{name}:{}: print to stdout", index + 1));
            }
            for needle in ["stdout()", "dbg!("] {
                if line.contains(needle) {
                    problems.push(format!("{name}:{}: {needle}", index + 1));
                }
            }
        }
    }
    problems
}

#[test]
fn nothing_but_cli_rs_writes_to_the_standard_output() {
    let problems = stdout_writes(&source_files());
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn counter_proof_a_print_in_the_server_is_detected() {
    for defective in [
        "println!(\"debug {payload}\");",
        "print!(\"{text}\");",
        "eprintln!(\"fine\"); println!(\"not fine\");",
        "let mut out = std::io::stdout();",
    ] {
        let problems = stdout_writes(&[("server.rs".to_owned(), defective.to_owned())]);
        assert!(!problems.is_empty(), "{defective}");
    }
    assert!(
        stdout_writes(&[("server.rs".to_owned(), "eprintln!(\"log\");".to_owned())]).is_empty()
    );
}

// --- les ponts : jeton et sockets seulement dans le cœur, bouclage seulement --------

/// Chaque fichier Python sous `bridges/`, comme (chemin relatif à `bridges/`, texte).
fn bridge_files() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, found: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("bridges directory")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if !path.ends_with("__pycache__") {
                    walk(&path, root, found);
                }
            } else if path.extension().is_some_and(|e| e == "py") {
                let name = path
                    .strip_prefix(root)
                    .expect("inside bridges")
                    .to_string_lossy()
                    .into_owned();
                found.push((name, fs::read_to_string(&path).expect("bridge text")));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("bridges");
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found
}

fn bridge_text(name: &str) -> String {
    bridge_files()
        .into_iter()
        .find(|(file, _)| file == name)
        .map(|(_, text)| text)
        .unwrap_or_else(|| panic!("bridges/{name} not found"))
}

/// Le code d'un fichier Python sans ses commentaires de ligne entière.
fn python_code_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect()
}

const BRIDGE_SOCKET_SERVERS: [&str; 7] = [
    ".bind(",
    ".listen(",
    "socket.socket(",
    "create_server(",
    "socketserver",
    "http.server",
    "start_server(",
];

/// Les comparaisons de jeton et les sockets d'écoute dans un fichier de pont autre que le cœur.
/// Les fichiers générés sont contrôlés à part : ils doivent être le cœur et un adaptateur, rien de plus.
fn stray_bridge_security(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, source) in files {
        if name == "_core.py" || name.starts_with("generated/") {
            continue;
        }
        for (index, line) in python_code_lines(source).into_iter().enumerate() {
            let at = format!("bridges/{name}:{}", index + 1);
            let lower = line.to_lowercase();
            if lower.contains("compare_digest")
                || (lower.contains("token")
                    && ["==", "!=", " is ", " in "]
                        .iter()
                        .any(|op| lower.contains(op)))
            {
                problems.push(format!("{at}: token comparison outside the core"));
            }
            for needle in BRIDGE_SOCKET_SERVERS {
                if line.contains(needle) {
                    problems.push(format!("{at}: {needle} outside the core"));
                }
            }
        }
    }
    problems
}

/// Ce qu'un pont généré contient en plus du cœur et de son adaptateur : seulement des
/// commentaires, des lignes vides et l'appel final.
fn generated_extras(generated: &str, core: &str, adapter: &str) -> Vec<String> {
    let Some(rest) = generated
        .split_once(core.trim_end())
        .map(|(head, tail)| format!("{head}{tail}"))
    else {
        return vec!["the generated bridge does not contain the core".to_owned()];
    };
    let Some(rest) = rest
        .split_once(adapter.trim_end())
        .map(|(head, tail)| format!("{head}{tail}"))
    else {
        return vec!["the generated bridge does not contain the adapter".to_owned()];
    };
    rest.lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with('#') && *line != "launch(ADAPTER(), DEFAULT_PORT)"
        })
        .map(|line| format!("extra code in the generated bridge: {line}"))
        .collect()
}

#[test]
fn bridges_compare_the_token_and_open_sockets_only_in_the_core() {
    let files = bridge_files();
    assert!(files.iter().any(|(name, _)| name == "_core.py"));
    assert!(files.iter().any(|(name, _)| name == "adapters/gimp.py"));
    let problems = stray_bridge_security(&files);
    assert!(problems.is_empty(), "{problems:?}");
    let core = bridge_text("_core.py");
    assert!(core.contains("hmac.compare_digest(given, self._token)"));
    for (name, text) in files
        .iter()
        .filter(|(name, _)| name.starts_with("generated/"))
    {
        let adapter_name = name
            .trim_start_matches("generated/")
            .trim_end_matches("_bridge.py");
        let adapter = bridge_text(&format!("adapters/{adapter_name}.py"));
        let extras = generated_extras(text, &core, &adapter);
        assert!(extras.is_empty(), "{name}: {extras:?}");
    }
}

#[test]
fn counter_proof_stray_bridge_security_is_detected() {
    let check = |name: &str, source: &str| -> Verdict {
        let problems = stray_bridge_security(&[(name.to_owned(), source.to_owned())]);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    for defective in [
        "if hmac.compare_digest(token, given):\n    pass",
        "if given_token == expected:\n    pass",
        "server.bind((\"127.0.0.1\", 9301))",
        "server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)",
        "import http.server",
    ] {
        must_fail(check("adapters/gimp.py", defective));
    }
    must_pass(check(
        "adapters/gimp.py",
        "# token == other is only a comment\nGimp = _gimp()",
    ));
    let core = bridge_text("_core.py");
    let adapter = bridge_text("adapters/gimp.py");
    let sound = bridge_text("generated/gimp_bridge.py");
    assert!(generated_extras(&sound, &core, &adapter).is_empty());
    let padded = sound.replace(
        "launch(ADAPTER(), DEFAULT_PORT)",
        "_extra = socket.socket()\nlaunch(ADAPTER(), DEFAULT_PORT)",
    );
    assert!(!generated_extras(&padded, &core, &adapter).is_empty());
    let edited = sound.replacen("MAX_LINE_BYTES = 8", "MAX_LINE_BYTES = 9", 1);
    assert!(!generated_extras(&edited, &core, &adapter).is_empty());
}

const CORE_FORBIDDEN: [&str; 19] = [
    ".connect(",
    "connect_ex(",
    "create_connection(",
    "import urllib",
    "urllib.",
    "http.client",
    "ftplib",
    "smtplib",
    "import requests",
    "subprocess",
    "os.system(",
    "os.popen(",
    "os.exec",
    "os.spawn",
    "os.posix_spawn",
    "os.fork",
    "import pty",
    "multiprocessing",
    "asyncio",
];

/// Toute adresse autre que 127.0.0.1 dans un fichier de pont, et, dans le cœur, toute connexion
/// sortante ou tout lancement de processus.
fn bridge_network_problems(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, source) in files {
        for (index, line) in python_code_lines(source).into_iter().enumerate() {
            let at = format!("bridges/{name}:{}", index + 1);
            for address in dotted_quads(line) {
                if address != "127.0.0.1" {
                    problems.push(format!("{at}: address {address}"));
                }
            }
            for other in [
                "INADDR_ANY",
                "AF_INET6",
                "localhost",
                "\"::",
                "'::",
                "bind((\"\"",
                "bind(('',",
                "getaddrinfo",
                "gethostbyname",
            ] {
                if line.contains(other) {
                    problems.push(format!("{at}: {other}"));
                }
            }
            if name == "_core.py" {
                for needle in CORE_FORBIDDEN {
                    if line.contains(needle) {
                        problems.push(format!("{at}: {needle} in the core"));
                    }
                }
            }
        }
    }
    problems
}

#[test]
fn bridges_name_no_address_but_127_0_0_1_and_the_core_neither_connects_nor_launches() {
    let files = bridge_files();
    let problems = bridge_network_problems(&files);
    assert!(problems.is_empty(), "{problems:?}");
    let core = bridge_text("_core.py");
    assert!(core.contains("\nLISTEN_ADDRESS = \"127.0.0.1\"\n"));
    let binds: Vec<&str> = python_code_lines(&core)
        .into_iter()
        .filter(|line| line.contains(".bind("))
        .map(str::trim)
        .collect();
    assert_eq!(binds, vec!["listener.bind((LISTEN_ADDRESS, port))"]);
}

#[test]
fn counter_proof_a_bridge_reaching_out_is_detected() {
    let check = |name: &str, source: &str| -> Verdict {
        let problems = bridge_network_problems(&[(name.to_owned(), source.to_owned())]);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    for (name, defective) in [
        ("_core.py", "listener.bind((\"0.0.0.0\", port))"),
        ("_core.py", "listener.bind((\"\", port))"),
        ("_core.py", "LISTEN_ADDRESS = \"localhost\""),
        ("adapters/gimp.py", "HOST = \"192.0.2.10\""),
        ("_core.py", "import subprocess"),
        ("_core.py", "os.system(\"true\")"),
        (
            "_core.py",
            "link = socket.create_connection((LISTEN_ADDRESS, 80))",
        ),
        (
            "_core.py",
            "from urllib.request import urlopen\nurlopen(\"x\")",
        ),
    ] {
        must_fail(check(name, defective));
    }
    must_pass(check(
        "_core.py",
        "LISTEN_ADDRESS = \"127.0.0.1\"\nBRIDGE_VERSION = \"1\"",
    ));
}

// --- démarrage de processus : seulement le module Inkscape et le lanceur ------------

/// Les fichiers source autorisés à démarrer un processus.
const PROCESS_STARTERS: [&str; 2] = ["modules/inkscape.rs", "launch.rs"];

/// Dit si `word` apparaît dans `line` comme un identifiant entier.
fn has_word(line: &str, word: &str) -> bool {
    line.match_indices(word).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + word.len()..].chars().next();
        let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !part(before) && !part(after)
    })
}

fn process_starts(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, source) in files {
        if PROCESS_STARTERS.contains(&name.as_str()) {
            continue;
        }
        for (index, line) in code_lines(source).into_iter().enumerate() {
            let at = format!("{name}:{}", index + 1);
            let imports_command = line.contains("process::")
                && (has_word(line, "Command") || has_word(line, "CommandExt"));
            if imports_command || line.contains("Command::new(") || line.contains(".spawn()") {
                problems.push(format!("{at}: starts a process"));
            }
        }
    }
    problems
}

#[test]
fn only_the_inkscape_module_and_the_launcher_start_processes() {
    let files = source_files();
    for starter in PROCESS_STARTERS {
        assert!(files.iter().any(|(name, _)| name == starter), "{starter}");
    }
    let problems = process_starts(&files);
    assert!(problems.is_empty(), "{problems:?}");
    let inkscape = source_of("modules/inkscape.rs");
    assert!(inkscape.contains("Command::new(&spec.program)"));
    let launcher = source_of("launch.rs");
    assert!(launcher.contains("Command::new(&spawn.program)"));
}

#[test]
fn counter_proof_a_process_started_elsewhere_is_detected() {
    let check = |name: &str, source: &str| -> Verdict {
        let problems = process_starts(&[(name.to_owned(), source.to_owned())]);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    for (name, defective) in [
        ("server.rs", "use std::process::Command;"),
        ("cli.rs", "use std::process::{Command, Stdio};"),
        (
            "modules/mod.rs",
            "let child = std::process::Command::new(\"sh\");",
        ),
        (
            "modules/libreoffice.rs",
            "let child = Command::new(\"soffice\").spawn()?;",
        ),
        ("registry.rs", "let child = command.spawn()?;"),
        (
            "bin/libremodular-launch.rs",
            "use std::os::unix::process::CommandExt;",
        ),
        ("security.rs", "let child = Command::new(\"id\");"),
    ] {
        must_fail(check(name, defective));
    }
    // Les deux fichiers qui peuvent démarrer un processus, et ce qui n'en a que l'air.
    must_pass(check(
        "launch.rs",
        "let child = Command::new(&spawn.program).spawn()?;",
    ));
    must_pass(check(
        "modules/inkscape.rs",
        "let child = Command::new(&spec.program).spawn()?;",
    ));
    must_pass(check(
        "cli.rs",
        "use std::process::ExitCode;\nSome(Command::Serve) | None => {}\nlet t = std::thread::spawn(f);",
    ));
}

// --- le module Inkscape ne répond que par le trait ----------------------------------

/// Le module Inkscape rend ses réponses et n'écrit nulle part ailleurs, et seul le registre le
/// construit : le serveur ne voit que le trait `Module`.
fn inkscape_bypasses(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, source) in files {
        for (index, line) in code_lines(source).into_iter().enumerate() {
            let at = format!("{name}:{}", index + 1);
            if name == "modules/inkscape.rs" {
                for needle in [
                    "crate::output",
                    "crate::server",
                    "crate::audit",
                    "crate::cli",
                    "print!",
                    "println!",
                    "eprint!",
                    "eprintln!",
                    "io::stdout(",
                    "io::stderr(",
                ] {
                    if line.contains(needle) {
                        problems.push(format!("{at}: {needle}"));
                    }
                }
            } else if name != "registry.rs"
                && (line.contains("InkscapeModule") || line.contains("modules::inkscape"))
            {
                problems.push(format!("{at}: reaches the Inkscape module directly"));
            }
        }
    }
    problems
}

#[test]
fn the_inkscape_module_answers_only_through_the_module_trait() {
    let files = source_files();
    let problems = inkscape_bypasses(&files);
    assert!(problems.is_empty(), "{problems:?}");
    let inkscape = source_of("modules/inkscape.rs");
    assert!(inkscape.contains("impl Module for InkscapeModule"));
}

#[test]
fn counter_proof_an_inkscape_answer_taking_another_way_is_detected() {
    let check = |name: &str, source: &str| -> Verdict {
        let problems = inkscape_bypasses(&[(name.to_owned(), source.to_owned())]);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    for (name, defective) in [
        ("modules/inkscape.rs", "eprintln!(\"{reply:?}\");"),
        (
            "modules/inkscape.rs",
            "let text = crate::output::to_text(&reply);",
        ),
        (
            "modules/inkscape.rs",
            "std::io::stdout().write_all(&finished.stdout)?;",
        ),
        ("server.rs", "use crate::modules::inkscape::InkscapeModule;"),
    ] {
        must_fail(check(name, defective));
    }
    must_pass(check(
        "registry.rs",
        "use crate::modules::inkscape::{InkscapeModule, InkscapePaths};",
    ));
}

// --- dépendances : exactement les crates approuvées, figées à une version -----------

/// Les crates que `[dependencies]` peut contenir, et rien d'autre.
const APPROVED_DEPENDENCIES: [&str; 12] = [
    "clap",
    "getrandom",
    "rmcp",
    "ruff_python_ast",
    "ruff_python_parser",
    "ruff_text_size",
    "rustix",
    "serde",
    "serde_json",
    "sha2",
    "tokio",
    "toml",
];

/// `=` suivi de trois nombres séparés par des points.
fn is_exact_version(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('=') else {
        return false;
    };
    let parts: Vec<&str> = rest.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

fn dependency_problems(manifest: &str) -> Vec<String> {
    let Ok(table) = toml::from_str::<toml::Table>(manifest) else {
        return vec!["Cargo.toml does not parse".to_owned()];
    };
    let mut problems = Vec::new();
    if table.contains_key("features") {
        problems.push("a [features] table: no feature may remove a protection".to_owned());
    }
    let Some(dependencies) = table.get("dependencies").and_then(toml::Value::as_table) else {
        problems.push("no [dependencies] table".to_owned());
        return problems;
    };
    for name in dependencies.keys() {
        if !APPROVED_DEPENDENCIES.contains(&name.as_str()) {
            problems.push(format!("{name} is not an approved crate"));
        }
    }
    for name in APPROVED_DEPENDENCIES {
        if !dependencies.contains_key(name) {
            problems.push(format!("{name} is missing"));
        }
    }
    for (name, specification) in dependencies {
        let version = match specification {
            toml::Value::String(version) => Some(version.as_str()),
            toml::Value::Table(fields) => fields.get("version").and_then(toml::Value::as_str),
            _ => None,
        };
        if !version.is_some_and(is_exact_version) {
            problems.push(format!("{name}: the version is not pinned as =x.y.z"));
        }
        if let toml::Value::Table(fields) = specification {
            for key in ["path", "git", "registry", "optional"] {
                if fields.contains_key(key) {
                    problems.push(format!("{name}: {key} is not allowed"));
                }
            }
        }
    }
    problems
}

#[test]
fn dependencies_are_exactly_the_approved_crates_pinned_to_one_version() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("Cargo.toml");
    let problems = dependency_problems(&manifest);
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn counter_proof_an_extra_unpinned_optional_or_missing_dependency_is_detected() {
    let real = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("Cargo.toml");
    let check = |manifest: &str| -> Verdict {
        let problems = dependency_problems(manifest);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    must_pass(check(&real));
    let extra = real.replacen(
        "[dependencies]\n",
        "[dependencies]\nreqwest = \"=0.12.0\"\n",
        1,
    );
    let unpinned = real.replacen("serde_json = \"=", "serde_json = \"", 1);
    let ranged = real.replacen("getrandom = \"=", "getrandom = \"^", 1);
    let optional = real.replacen(
        "sha2 = \"=0.11.0\"",
        "sha2 = { version = \"=0.11.0\", optional = true }",
        1,
    );
    let from_path = real
        .replacen(
            "toml = \"=",
            "toml = { path = \"../toml\", version = \"=",
            1,
        )
        .replacen("1.1.6\"", "1.1.6\" }", 1);
    let missing = real
        .lines()
        .filter(|line| !line.starts_with("clap = "))
        .collect::<Vec<_>>()
        .join("\n");
    let featured = format!("{real}\n[features]\nno-guard = []\n");
    for defective in [
        &extra, &unpinned, &ranged, &optional, &from_path, &missing, &featured,
    ] {
        assert_ne!(
            defective, &real,
            "the counter-proof must change the manifest"
        );
        must_fail(check(defective));
    }
}

// --- les fichiers du dépôt : aucune donnée d'opérateur, aucun chemin réel -----------

/// Le préfixe de répertoire par lequel commencerait un chemin réel, écrit en deux morceaux pour
/// que ce fichier ne le contienne pas.
fn home_prefix() -> String {
    ["/ho", "me/"].concat()
}

/// Chaque fichier du dépôt, sauf ce que la construction et les outils y déposent : les répertoires
/// `target` et `__pycache__` et les répertoires cachés autres que `.github`.
fn repository_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, top: bool, found: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .expect("directory")
            .map(|entry| entry.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            if path.is_dir() {
                let hidden = top && name.starts_with('.') && name != ".github";
                if name == "__pycache__" || (top && name == "target") || hidden {
                    continue;
                }
                found.push(path.clone());
                walk(&path, false, found);
            } else {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, true, &mut found);
    found
}

fn repository_problems(root: &Path) -> Vec<String> {
    let home = home_prefix();
    let mut problems = Vec::new();
    for path in repository_files(root) {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.is_dir() {
            if name == "captures" {
                problems.push(format!("{relative}: a captures directory"));
            }
            continue;
        }
        if name == "token" {
            problems.push(format!("{relative}: a token file"));
        }
        if name == "audit.jsonl" || name.starts_with("audit.jsonl.") {
            problems.push(format!("{relative}: an audit journal"));
        }
        if path.extension().is_some_and(|extension| extension == "png") {
            problems.push(format!("{relative}: a capture"));
        }
        if name == "config.toml" {
            problems.push(format!("{relative}: an operator configuration"));
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (at, _) in text.match_indices(&home) {
            let after = text.get(at + home.len()..).unwrap_or("");
            let example = after.strip_prefix("alice").is_some_and(|rest| {
                !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
            });
            if !example {
                let line = text
                    .get(..at)
                    .map_or(0, |head| head.matches('\n').count() + 1);
                problems.push(format!("{relative}:{line}: a path that is not an example"));
            }
        }
    }
    problems
}

#[test]
fn the_repository_holds_no_real_path_and_no_operator_file() {
    let problems = repository_problems(Path::new(env!("CARGO_MANIFEST_DIR")));
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn counter_proof_a_real_path_or_an_operator_file_is_detected() {
    let home = home_prefix();
    let build = |files: &[(&str, String)], directories: &[&str]| -> PathBuf {
        let base = common::private_tempdir().keep();
        for directory in directories {
            fs::create_dir_all(base.join(directory)).expect("directory");
        }
        for (name, text) in files {
            let path = base.join(name);
            fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
            fs::write(&path, text).expect("file");
        }
        base
    };
    let check = |base: &Path| -> Verdict {
        let problems = repository_problems(base);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    let clean = build(
        &[
            (
                "README.md",
                format!("see {home}alice/notes and {home}alice\n"),
            ),
            ("tests/a.py", format!("path = '{home}alice/work'\n")),
        ],
        &[],
    );
    must_pass(check(&clean));
    let ignored = build(
        &[
            ("target/debug/leak.txt", format!("{home}bob/secret\n")),
            ("bridges/__pycache__/x.txt", format!("{home}bob/secret\n")),
            (".claude/settings.json", format!("{home}bob/secret\n")),
        ],
        &[],
    );
    must_pass(check(&ignored));
    for (label, files, directories) in [
        (
            "another user",
            vec![("docs/a.md", format!("cd {home}bob/project\n"))],
            vec![],
        ),
        (
            "a longer name",
            vec![("docs/a.md", format!("cd {home}alice2/project\n"))],
            vec![],
        ),
        (
            "the bare prefix",
            vec![("docs/a.md", format!("cd {home}\n"))],
            vec![],
        ),
        (
            "a token file",
            vec![("config/token", "x".to_owned())],
            vec![],
        ),
        (
            "a journal",
            vec![("state/audit.jsonl", String::new())],
            vec![],
        ),
        (
            "a rotated journal",
            vec![("state/audit.jsonl.1", String::new())],
            vec![],
        ),
        (
            "a capture file",
            vec![("x/shot.png", String::new())],
            vec![],
        ),
        ("a captures directory", vec![], vec!["run/captures"]),
        (
            "an operator configuration",
            vec![("c/config.toml", String::new())],
            vec![],
        ),
    ] {
        let base = build(&files, &directories);
        assert!(check(&base).is_err(), "{label} was not detected");
    }
}

// --- server.rs : deux outils, aucun module d'application, un usage explicite de la sécurité ---

/// Les seuls éléments que `server.rs` prend au module de sécurité : il les appelle pour enchaîner
/// les contrôles dans un ordre fixe et n'en implémente aucun. En ajouter un est une décision.
const SERVER_SECURITY_ITEMS: [&str; 12] = [
    "self",
    "GuardPolicy",
    "Token",
    "accept_capture",
    "capture_path_for",
    "check_script_size",
    "check_target",
    "discard_capture",
    "ensure_private_dir",
    "format_refusal",
    "guard_script",
    "purge_captures",
];

const APPLICATION_MODULE_NAMES: [&str; 6] = [
    "modules::inkscape",
    "modules::libreoffice",
    "InkscapeModule",
    "LibreOfficeModule",
    "TcpModule",
    "libreoffice::",
];

fn server_problems(source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let code = code_lines(source).join("\n");
    let mut tools: Vec<String> = code
        .match_indices("#[tool(")
        .filter_map(|(at, _)| {
            let rest = code.get(at..)?;
            let after = rest.split_once("name = \"")?.1;
            Some(after.split('"').next()?.to_owned())
        })
        .collect();
    tools.sort_unstable();
    if tools != ["execute_script", "get_state"] {
        problems.push(format!("the tools declared are {tools:?}"));
    }
    for name in APPLICATION_MODULE_NAMES {
        if code.contains(name) {
            problems.push(format!("the server reaches an application module: {name}"));
        }
    }
    match code.split_once("use crate::security::{") {
        None => problems.push("no explicit import of the security items".to_owned()),
        Some((_, rest)) => {
            let list = rest.split_once("};").map_or("", |(list, _)| list);
            let mut items: Vec<&str> = list
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .collect();
            items.sort_unstable();
            let mut expected = SERVER_SECURITY_ITEMS.to_vec();
            expected.sort_unstable();
            if items != expected {
                problems.push(format!("the security items used are {items:?}"));
            }
        }
    }
    if code.matches("use crate::security").count() != 1 {
        problems.push("security is imported in more than one place".to_owned());
    }
    problems.extend(stray_security_logic(source));
    problems
}

#[test]
fn the_server_declares_two_tools_and_reaches_no_application_module() {
    let problems = server_problems(&source_of("server.rs"));
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn counter_proof_a_third_tool_an_application_module_or_a_new_security_item_is_detected() {
    let real = source_of("server.rs");
    let check = |source: &str| -> Verdict {
        let problems = server_problems(source);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    must_pass(check(&real));
    let third_tool = real.replacen(
        "#[tool(name = \"get_state\"",
        "#[tool(name = \"delete_everything\", description = \"no\")]\n    async fn third(&self) -> String { String::new() }\n\n    #[tool(name = \"get_state\"",
        1,
    );
    let renamed_tool = real.replacen("name = \"get_state\"", "name = \"get_status\"", 1);
    let application = format!("{real}\nuse crate::modules::inkscape::InkscapeModule;\n");
    let tcp = format!("{real}\nlet module = TcpModule::new(connector, timeout);\n");
    let new_item = real.replacen(
        "purge_captures,\n};",
        "purge_captures, open_control_file,\n};",
        1,
    );
    let own_logic = format!("{real}\nfn check_target(name: &str) -> bool {{ true }}\n");
    let second_import = format!("{real}\nuse crate::security::SecurityError;\n");
    for defective in [
        &third_tool,
        &renamed_tool,
        &application,
        &tcp,
        &new_item,
        &own_logic,
        &second_import,
    ] {
        assert_ne!(defective, &real, "the counter-proof must change the source");
        must_fail(check(defective));
    }
}

// --- le README : les mots qu'il ne doit pas employer --------------------------------

/// `AI` comme mot à part entière, `Windows` et `macOS`, n'importe où dans le texte.
fn readme_word_problems(text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let words: Vec<&str> = line
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        for forbidden in ["AI", "Windows", "macOS"] {
            if words.contains(&forbidden) {
                problems.push(format!("line {}: {forbidden}", number + 1));
            }
        }
    }
    problems
}

#[test]
fn the_readme_never_says_ai_windows_or_macos() {
    let text = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .expect("README.md");
    let problems = readme_word_problems(&text);
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn counter_proof_a_forbidden_word_in_the_readme_is_detected() {
    for defective in [
        "An AI that drives applications.",
        "AI-powered scripts",
        "Runs on Windows 11.",
        "Also tried on macOS.",
        "(AI)",
    ] {
        assert!(!readme_word_problems(defective).is_empty(), "{defective}");
    }
    for fine in [
        "A language model drives applications.",
        "The MAIN thread said so.",
        "an Inkscape window the operator opened",
        "mail, maintain, again",
    ] {
        assert!(readme_word_problems(fine).is_empty(), "{fine}");
    }
}

// --- le README et les guides des modules disent la même chose -----------------------

const MODULE_GUIDES: [&str; 5] = ["gimp", "krita", "blender", "inkscape", "libreoffice"];

/// Chaque nombre avec au moins un point dans `text`, comme `3.0.4`.
fn dotted_numbers(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|word| word.trim_matches('.'))
        .filter(|word| word.contains('.') && word.split('.').all(|part| !part.is_empty()))
        .map(str::to_owned)
        .collect()
}

/// La phrase entre `**Status:` et le `**` de fermeture d'un guide.
fn guide_status(guide: &str) -> Option<String> {
    let after = guide.split_once("**Status:")?.1;
    let status = after.split_once("**")?.0;
    Some(status.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Le texte entre un titre `## Example` et le titre suivant.
fn example_section(text: &str) -> Option<&str> {
    let after = text.split_once("\n## Example\n")?.1;
    Some(after.split("\n## ").next().unwrap_or(after))
}

/// Les corps des blocs délimités de `section` avec le langage `lang`.
fn fenced_blocks<'a>(section: &'a str, lang: &str) -> Vec<&'a str> {
    let opening = format!("```{lang}\n");
    section
        .split(&opening)
        .skip(1)
        .filter_map(|rest| rest.split_once("```").map(|(body, _)| body))
        .collect()
}

fn readme_guide_problems(readme: &str, guides: &[(&str, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, guide) in guides {
        let Some(status) = guide_status(guide) else {
            problems.push(format!("{name}: the guide has no status sentence"));
            continue;
        };
        if !status.starts_with("tried in ") && !status.starts_with("not tried") {
            problems.push(format!(
                "{name}: the status says neither tried in nor not tried"
            ));
        }
        let link = format!("docs/modules/{name}.md");
        let rows: Vec<&str> = readme
            .lines()
            .filter(|line| line.starts_with('|') && line.contains(&link))
            .collect();
        let [row] = rows.as_slice() else {
            problems.push(format!(
                "{name}: {} rows of the README name this guide",
                rows.len()
            ));
            continue;
        };
        let mut readme_versions = dotted_numbers(row);
        let mut guide_versions = dotted_numbers(&status);
        readme_versions.sort_unstable();
        guide_versions.sort_unstable();
        if readme_versions != guide_versions {
            problems.push(format!(
                "{name}: the README says {readme_versions:?}, the guide {guide_versions:?}"
            ));
        }
    }
    let libreoffice = guides
        .iter()
        .find(|(name, _)| *name == "libreoffice")
        .map(|(_, guide)| guide.as_str())
        .unwrap_or("");
    match (example_section(readme), example_section(libreoffice)) {
        (Some(from_readme), Some(from_guide)) => {
            for lang in ["python", "json"] {
                // Le premier bloc est le script, puis la réponse ; le guide continue avec l'état qui a suivi,
                // que le README ne répète pas.
                let first = |section| fenced_blocks(section, lang).first().copied();
                if first(from_readme).is_none() || first(from_readme) != first(from_guide) {
                    problems.push(format!("the {lang} block of the example differs"));
                }
            }
        }
        _ => problems.push("an Example section is missing".to_owned()),
    }
    problems
}

fn read_docs() -> (String, Vec<(&'static str, String)>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = fs::read_to_string(root.join("README.md")).expect("README.md");
    let guides = MODULE_GUIDES
        .iter()
        .map(|name| {
            let path = root.join("docs").join("modules").join(format!("{name}.md"));
            (*name, fs::read_to_string(path).expect("guide"))
        })
        .collect();
    (readme, guides)
}

#[test]
fn the_readme_and_the_module_guides_say_the_same_thing() {
    let (readme, guides) = read_docs();
    let problems = readme_guide_problems(&readme, &guides);
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn counter_proof_a_readme_that_differs_from_a_guide_is_detected() {
    let (readme, guides) = read_docs();
    let check = |readme: &str, guides: &[(&str, String)]| -> Verdict {
        let problems = readme_guide_problems(readme, guides);
        ensure!(problems.is_empty(), "{problems:?}");
        Ok(())
    };
    must_pass(check(&readme, &guides));
    let other_version = readme.replacen("| 3.0.4,", "| 3.0.5,", 1);
    let other_answer = readme.replacen("\"duration_ms\": 170", "\"duration_ms\": 17", 1);
    let other_script = readme.replacen("setValue(42)", "setValue(43)", 1);
    for defective in [&other_version, &other_answer, &other_script] {
        assert_ne!(
            defective, &readme,
            "the counter-proof must change the README"
        );
        must_fail(check(defective, &guides));
    }
    let untried: Vec<(&str, String)> = guides
        .iter()
        .map(|(name, guide)| {
            let text = if *name == "krita" {
                guide.replacen("**Status: tried in", "**Status: maybe in", 1)
            } else {
                guide.clone()
            };
            (*name, text)
        })
        .collect();
    must_fail(check(&readme, &untried));
}
