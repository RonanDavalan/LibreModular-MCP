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

// --- the server starts hardened before it reads the token -------------------------

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

// --- every tool answer leaves through the single output point ----------------------

const TOOLS: [&str; 2] = ["execute_script", "get_state"];

/// Each tool returns what `finish` produced, and `finish` is the only place of
/// the server that serialises an answer, after `output::finalize`.
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

// --- the network: loopback only, and only in the protocol module -----------------

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

/// Network use outside `protocol.rs`, any listening socket, and any address other
/// than 127.0.0.1 written in the code.
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

/// Every `a.b.c.d` made of four numbers in a line.
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

// --- modules never import the security or output modules -------------------------

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

// --- only the protocol on the standard output -----------------------------------

fn stdout_writes(files: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    for (name, source) in files.iter().filter(|(name, _)| name != "cli.rs") {
        for (index, line) in code_lines(source).into_iter().enumerate() {
            // `eprintln!` and `eprint!` write to the error stream and are allowed.
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

// --- the bridges: token and sockets only in the core, loopback only -------------

/// Every Python file under `bridges/`, as (path relative to `bridges/`, text).
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

/// The code of a Python file without its full-line comments.
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

/// Token comparisons and listening sockets in a bridge file other than the
/// core. Generated files are checked separately: they must be the core and an
/// adapter, nothing more.
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

/// What a generated bridge holds besides the core and its adapter: only
/// comments, blank lines and the final call.
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

/// Any address other than 127.0.0.1 in a bridge file, and, in the core, any
/// outgoing connection or process launch.
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

// --- starting processes: only the Inkscape module ----------------------------------

/// The source files allowed to start a process.
const PROCESS_STARTERS: [&str; 1] = ["modules/inkscape.rs"];

/// Whether `word` appears in `line` as a whole identifier.
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
fn only_the_inkscape_module_starts_processes() {
    let files = source_files();
    assert!(files.iter().any(|(name, _)| name == "modules/inkscape.rs"));
    let problems = process_starts(&files);
    assert!(problems.is_empty(), "{problems:?}");
    let inkscape = source_of("modules/inkscape.rs");
    assert!(inkscape.contains("Command::new(&spec.program)"));
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
        ("registry.rs", "let child = command.spawn()?;"),
        (
            "bin/libremodular-launch.rs",
            "use std::os::unix::process::CommandExt;",
        ),
    ] {
        must_fail(check(name, defective));
    }
    must_pass(check(
        "cli.rs",
        "use std::process::ExitCode;\nSome(Command::Serve) | None => {}\nlet t = std::thread::spawn(f);",
    ));
}

// --- the Inkscape module answers only through the trait --------------------------------

/// The Inkscape module returns its answers and writes nowhere else, and only the
/// registry builds it: the server sees nothing but the `Module` trait.
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
