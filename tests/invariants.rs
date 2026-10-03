//! Mechanical invariants of the code base, replayed on every run.
//!
//! Why this module exists
//! Security properties that only hold by convention rot quietly. Each test here
//! reads the code (or runs it in a fresh process) and fails when a rule is
//! broken, and each ships a counter-proof showing the rule would be noticed.

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

// --- reading the code --------------------------------------------------------------

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

/// The code of a source file without its full-line comments.
fn code_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect()
}

/// The text of a function, from its opening brace to the matching closing one.
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

// --- process hardening precedes any token access (read from the source) ---------

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

// --- process hardening precedes any token access (observed in a fresh process) ---

const CHILD_KIND: &str = "LMM_INVARIANT_CHILD";
const CHILD_FILE: &str = "LMM_INVARIANT_FILE";
const CHILD_TEST: &str = "hardening_and_private_modes_hold_in_a_fresh_process";

fn core_limit() -> Rlimit {
    getrlimit(Resource::Core)
}

/// The body of the child process: report what it observed, never the token.
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

/// Run this very test binary in a new process in child mode and return what it reported.
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
    // Loading and creating the token lower the core-dump limit; reading it without
    // hardening does not, which is what the counter-proof relies on.
    must_pass(check_the_core_limit_is_zero_afterwards("load", base.path()));
    must_pass(check_the_core_limit_is_zero_afterwards(
        "create",
        base.path(),
    ));
    must_fail(check_the_core_limit_is_zero_afterwards(
        "load-without-hardening",
        base.path(),
    ));
    // Everything the Hub creates is private even when the process umask is zero.
    must_pass(check_created_files_have_no_group_or_other_bit(
        "modes",
        base.path(),
    ));
    must_fail(check_created_files_have_no_group_or_other_bit(
        "modes-without-explicit-modes",
        base.path(),
    ));
}

// --- security logic lives only in security.rs ------------------------------------

const SECURITY_ONLY_DEFINITIONS: [&str; 12] = [
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
];

/// What a file other than the security module must not contain. Calling a
/// security function is fine; implementing one is not.
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

// --- control files are only opened through the permission check -------------------

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

#[test]
fn security_rs_opens_files_only_without_following_links_and_creates_directories_private() {
    let source = source_of("security.rs");
    let opens = code_lines(&source)
        .iter()
        .filter(|line| line.contains("rustix::fs::open("))
        .count();
    assert_eq!(opens, 1, "exactly one place opens a file");
    let body = function_body(&source, "open_regular_nofollow").expect("open function");
    assert!(
        body.contains("OFlags::NOFOLLOW"),
        "the single open must not follow links"
    );
    assert!(
        body.contains("FileMode::RUSR | FileMode::WUSR"),
        "created files are 0600"
    );
    let builders = source.matches("DirBuilder::new()").count();
    let private = source.matches(".mode(PRIVATE_DIR_MODE)").count();
    assert_eq!(
        builders, private,
        "every directory is created with an explicit private mode"
    );
    assert!(source.contains("const PRIVATE_DIR_MODE: u32 = 0o700;"));
    assert!(source.contains("const PRIVATE_FILE_MODE: u32 = 0o600;"));
}
