//! Audit journal tests.
//!
//! Why this module exists
//! The journal must be private, bounded and free of script text and token.

#[macro_use]
mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{FAKE_TOKEN, Verdict, mode_of, must_fail, must_pass, write_file};
use libremodular_mcp::audit::{
    AuditEntry, AuditLog, AuditUnavailable, ENTRY_KEYS, GuardOutcome, format_timestamp,
    script_fingerprint,
};
use libremodular_mcp::config::Limits;
use libremodular_mcp::security::{Category, Token};
use serde_json::Value;

const SCRIPT: &str = "print('secret-looking script body')";

fn entry() -> AuditEntry {
    let (size, digest) = script_fingerprint(SCRIPT);
    AuditEntry {
        ts: None,
        tool: Some("execute_script".to_owned()),
        target_app: Some("gimp".to_owned()),
        status: Some("ok".to_owned()),
        duration_ms: Some(5),
        script_bytes: Some(size as u64),
        script_sha256: Some(digest),
        guard: Some(GuardOutcome::Passed),
        output_bytes: Some(10),
    }
}

fn year_2000() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(946_684_800)
}

fn make(path: &Path) -> AuditLog {
    AuditLog::new(
        path,
        Token::new(FAKE_TOKEN).expect("token"),
        Limits::default(),
    )
    .with_clock(Box::new(year_2000))
}

fn rows(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("journal")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect()
}

#[test]
fn one_json_line_per_record_with_mode_600_in_a_private_directory() {
    let base = common::private_tempdir();
    let path = base.path().join("state").join("audit.jsonl");
    let log = make(&path);
    log.record(&entry(), None).expect("first");
    log.record(
        &AuditEntry {
            status: Some("error".to_owned()),
            ..entry()
        },
        None,
    )
    .expect("second");
    let statuses: Vec<_> = rows(&path)
        .iter()
        .map(|r| r["status"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(statuses, ["ok", "error"]);
    assert_eq!(mode_of(&path), 0o600);
    assert_eq!(mode_of(path.parent().expect("parent")), 0o700);
    assert!(
        rows(&path)[0]["ts"]
            .as_str()
            .expect("ts")
            .starts_with("2000-01-01")
    );
}

#[test]
fn lines_carry_exactly_the_fixed_keys_in_a_fixed_order() {
    let base = common::private_tempdir();
    let path = base.path().join("audit.jsonl");
    make(&path).record(&entry(), Some(SCRIPT)).expect("record");
    let text = fs::read_to_string(&path).expect("journal");
    let mut last = 0;
    for key in ENTRY_KEYS {
        let at = text
            .find(&format!("\"{key}\":"))
            .unwrap_or_else(|| panic!("{key} is missing"));
        assert!(at >= last, "{key} is out of order");
        last = at;
    }
    let row = &rows(&path)[0];
    let keys: Vec<&str> = row
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), ENTRY_KEYS.len(), "{keys:?}");
    assert!(keys.iter().all(|k| ENTRY_KEYS.contains(k)), "{keys:?}");
}

type LogFactory<'f> = &'f dyn Fn(&Path) -> AuditLog;

fn check_script_text_policy(make_log: LogFactory<'_>, base: &Path, expect_text: bool) -> Verdict {
    let base = common::fresh(base);
    let path = base.join("audit.jsonl");
    make_log(&path)
        .record(&entry(), Some(SCRIPT))
        .expect("record");
    let present = fs::read_to_string(&path).expect("journal").contains(SCRIPT);
    ensure!(
        present == expect_text,
        "script text present: {present}, expected: {expect_text}"
    );
    Ok(())
}

#[test]
fn script_text_is_absent_by_default_and_present_when_enabled() {
    let base = common::private_tempdir();
    must_pass(check_script_text_policy(&make, base.path(), false));
    must_pass(check_script_text_policy(
        &|p| make(p).with_script_text(true),
        base.path(),
        true,
    ));
    must_fail(check_script_text_policy(
        &|p| make(p).with_script_text(true),
        base.path(),
        false,
    ));
}

#[test]
fn script_text_is_bounded_by_the_output_limit_and_scrubbed_of_the_token() {
    let base = common::private_tempdir();
    let path = base.path().join("audit.jsonl");
    let log = AuditLog::new(
        &path,
        Token::new(FAKE_TOKEN).expect("token"),
        Limits {
            max_output_bytes: 2048,
            ..Limits::default()
        },
    )
    .with_script_text(true);
    let script = format!("x = '{FAKE_TOKEN}'\n{}", "y".repeat(100_000));
    log.record(&entry(), Some(&script)).expect("record");
    let text = fs::read_to_string(&path).expect("journal");
    assert!(!text.contains(FAKE_TOKEN));
    assert!(text.len() < 4096, "{} bytes", text.len());
    assert_eq!(rows(&path)[0]["truncated"], Value::Bool(true));
}

#[test]
fn the_token_is_removed_from_entries() {
    let base = common::private_tempdir();
    let path = base.path().join("audit.jsonl");
    make(&path)
        .record(
            &AuditEntry {
                target_app: Some(format!("x{FAKE_TOKEN}")),
                ..entry()
            },
            None,
        )
        .expect("record");
    assert!(
        !fs::read_to_string(&path)
            .expect("journal")
            .contains(FAKE_TOKEN)
    );
}

#[test]
fn an_entry_has_no_field_that_could_carry_output_or_documents() {
    let value = AuditEntry::default();
    // Every field of the entry is listed here: adding one makes this test stop compiling.
    let AuditEntry {
        ts,
        tool,
        target_app,
        status,
        duration_ms,
        script_bytes,
        script_sha256,
        guard,
        output_bytes,
    } = value;
    assert!(ts.is_none() && tool.is_none() && target_app.is_none() && status.is_none());
    assert!(duration_ms.is_none() && script_bytes.is_none() && script_sha256.is_none());
    assert!(guard.is_none() && output_bytes.is_none());
}

#[test]
fn the_guard_outcome_is_written_as_a_list_of_categories_or_bypassed() {
    let base = common::private_tempdir();
    let path = base.path().join("audit.jsonl");
    let log = make(&path);
    for guard in [
        GuardOutcome::Passed,
        GuardOutcome::Refused(vec![Category::FileDeletion, Category::NetworkAccess]),
        GuardOutcome::Bypassed,
    ] {
        log.record(
            &AuditEntry {
                guard: Some(guard),
                ..entry()
            },
            None,
        )
        .expect("record");
    }
    let guards: Vec<Value> = rows(&path).iter().map(|r| r["guard"].clone()).collect();
    assert_eq!(guards[0], serde_json::json!([]));
    assert_eq!(guards[1], serde_json::json!(["G-1", "G-3"]));
    assert_eq!(guards[2], serde_json::json!("bypassed"));
}

#[test]
fn rotation_keeps_five_generations() {
    let base = common::private_tempdir();
    let path = base.path().join("audit.jsonl");
    let log = make(&path).with_rotation(300, 5);
    for _ in 0..60 {
        log.record(&entry(), None).expect("record");
    }
    let names: std::collections::BTreeSet<String> = fs::read_dir(base.path())
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    // The journal is rotated right after a write, so the current file may be
    // absent until the next record: it is allowed, the five generations are not optional.
    let generations: std::collections::BTreeSet<String> =
        (1..=5).map(|i| format!("audit.jsonl.{i}")).collect();
    let allowed: std::collections::BTreeSet<String> = generations
        .iter()
        .cloned()
        .chain(std::iter::once("audit.jsonl".to_owned()))
        .collect();
    assert!(generations.is_subset(&names), "{names:?}");
    assert!(names.is_subset(&allowed), "{names:?}");
    for name in &names {
        assert_eq!(mode_of(&base.path().join(name)), 0o600, "{name}");
    }
}

#[test]
fn rotation_never_follows_a_link_planted_as_an_old_generation() {
    let base = common::private_tempdir();
    let path = base.path().join("audit.jsonl");
    let victim = base.path().join("victim");
    write_file(&victim, 0o600, "keep me");
    symlink(&victim, base.path().join("audit.jsonl.2")).expect("symlink");
    let log = make(&path).with_rotation(300, 2);
    for _ in 0..30 {
        log.record(&entry(), None).expect("record");
    }
    assert_eq!(fs::read_to_string(&victim).expect("victim"), "keep me");
}

#[test]
fn unavailable_when_the_directory_cannot_be_used() {
    let base = common::private_tempdir();
    let blocker = base.path().join("blocker");
    write_file(&blocker, 0o600, "a file, not a directory");
    let outcome = make(&blocker.join("audit.jsonl")).record(&entry(), None);
    assert!(matches!(outcome, Err(AuditUnavailable { .. })));
}

#[test]
fn unavailable_when_the_journal_path_is_a_directory_even_for_root() {
    let base = common::private_tempdir();
    let path = base.path().join("audit.jsonl");
    fs::create_dir(&path).expect("dir");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("chmod");
    assert!(make(&path).record(&entry(), None).is_err());
}

#[test]
#[ignore = "root ignores permission bits"]
fn unavailable_when_the_directory_is_read_only() {
    let base = common::private_tempdir();
    let directory = base.path().join("ro");
    fs::create_dir(&directory).expect("dir");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500)).expect("chmod");
    let outcome = make(&directory.join("audit.jsonl")).record(&entry(), None);
    assert!(outcome.is_err());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("restore");
}

#[test]
fn unavailable_when_the_journal_is_too_open_or_a_symlink() {
    let base = common::private_tempdir();
    let directory = base.path().join("state");
    fs::create_dir(&directory).expect("dir");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("chmod");
    let path = directory.join("audit.jsonl");
    write_file(&path, 0o644, "");
    assert!(make(&path).record(&entry(), None).is_err());
    fs::remove_file(&path).expect("remove");
    let target = base.path().join("elsewhere");
    write_file(&target, 0o600, "");
    symlink(&target, &path).expect("symlink");
    assert!(make(&path).record(&entry(), None).is_err());
    assert_eq!(fs::read_to_string(&target).expect("target"), "");
}

#[test]
fn unavailable_when_the_directory_is_too_open() {
    let base = common::private_tempdir();
    let directory = base.path().join("open");
    fs::create_dir(&directory).expect("dir");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(
        make(&directory.join("audit.jsonl"))
            .record(&entry(), None)
            .is_err()
    );
}

#[test]
fn an_unavailable_journal_writes_nothing() {
    let base = common::private_tempdir();
    let directory = base.path().join("open");
    fs::create_dir(&directory).expect("dir");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("chmod");
    let _ = make(&directory.join("audit.jsonl")).record(&entry(), None);
    assert!(!directory.join("audit.jsonl").exists());
}

#[test]
fn script_fingerprint_is_size_and_sha256() {
    let (size, digest) = script_fingerprint("abc");
    assert_eq!(size, 3);
    assert_eq!(
        digest,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(script_fingerprint("é").0, 2);
}

#[test]
fn timestamps_are_utc_with_milliseconds_and_handle_leap_days() {
    let at = |seconds: u64, millis: u32| UNIX_EPOCH + Duration::new(seconds, millis * 1_000_000);
    assert_eq!(format_timestamp(at(0, 0)), "1970-01-01T00:00:00.000Z");
    assert_eq!(
        format_timestamp(at(946_684_800, 0)),
        "2000-01-01T00:00:00.000Z"
    );
    assert_eq!(
        format_timestamp(at(951_782_400, 0)),
        "2000-02-29T00:00:00.000Z"
    );
    assert_eq!(
        format_timestamp(at(1_709_164_800, 0)),
        "2024-02-29T00:00:00.000Z"
    );
    assert_eq!(
        format_timestamp(at(4_107_542_400, 0)),
        "2100-03-01T00:00:00.000Z"
    );
    assert_eq!(
        format_timestamp(at(1_000_000_000, 123)),
        "2001-09-09T01:46:40.123Z"
    );
    assert_eq!(
        format_timestamp(at(1_735_689_599, 999)),
        "2024-12-31T23:59:59.999Z"
    );
}
