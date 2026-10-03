//! Tests of the single output point.
//!
//! Why this module exists
//! Whatever the Hub returns must never carry the token and must stay bounded.

#[macro_use]
mod common;

use common::{FAKE_TOKEN, Verdict, must_fail, must_pass};
use libremodular_mcp::config::Limits;
use libremodular_mcp::output::{OutputError, REDACTION, finalize, to_text};
use libremodular_mcp::security::Token;
use serde_json::{Value, json};

fn token() -> Token {
    Token::new(FAKE_TOKEN).expect("token")
}

fn small() -> Limits {
    Limits {
        max_output_bytes: 64,
        ..Limits::default()
    }
}

fn run(payload: &Value, limits: &Limits) -> Value {
    finalize(payload, &token(), limits).expect("finalize")
}

fn flat(value: &Value) -> String {
    value.to_string()
}

type Finalizer<'f> = &'f dyn Fn(&Value) -> Value;

fn check_removes_token_everywhere(finalizer: Finalizer<'_>) -> Verdict {
    let upper = FAKE_TOKEN.to_uppercase();
    let payload = json!({
        "plain": format!("before {FAKE_TOKEN} after"),
        FAKE_TOKEN: "key",
        "nested": {"deep": [format!("x{FAKE_TOKEN}"), {"deeper": FAKE_TOKEN}]},
        "error": {"traceback": format!("File \"<libremodular>\", line 1\n  token = \"{FAKE_TOKEN}\"")},
        "upper": upper,
        "mixed": "Test-Token-0000",
    });
    let out = flat(&finalizer(&payload));
    ensure!(
        !out.to_lowercase().contains(FAKE_TOKEN),
        "the token survived: {out}"
    );
    Ok(())
}

fn check_removes_token_cut_by_truncation(finalizer: Finalizer<'_>) -> Verdict {
    let text = format!("{}{FAKE_TOKEN}{}", "x".repeat(48), "y".repeat(100));
    let out = finalizer(&json!({ "stdout": text }));
    ensure!(out["truncated"] == Value::Bool(true), "nothing was cut");
    let stdout = out["stdout"].as_str().unwrap_or_default();
    ensure!(
        !stdout.contains(&FAKE_TOKEN[..5]),
        "a prefix of the token survived: {stdout}"
    );
    Ok(())
}

fn top_level_only(payload: &Value) -> Value {
    let mut out = payload.clone();
    if let Value::Object(map) = &mut out {
        for value in map.values_mut() {
            if let Value::String(text) = value {
                *text = text.replace(FAKE_TOKEN, "");
            }
        }
    }
    out
}

fn cut_then_remove(payload: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Value::Object(map) = payload {
        for (key, value) in map {
            let text = value.as_str().unwrap_or_default();
            let cut: String = text.chars().take(53).collect();
            out.insert(
                key.clone(),
                Value::String(format!("{}[truncated]", cut.replace(FAKE_TOKEN, ""))),
            );
        }
    }
    out.insert("truncated".to_owned(), Value::Bool(true));
    Value::Object(out)
}

#[test]
fn finalize_removes_the_token_from_values_keys_nested_data_and_traces() {
    must_pass(check_removes_token_everywhere(&|p| {
        run(p, &Limits::default())
    }));
    must_fail(check_removes_token_everywhere(&top_level_only));
}

#[test]
fn finalize_removes_a_token_that_truncation_would_have_cut() {
    must_pass(check_removes_token_cut_by_truncation(&|p| run(p, &small())));
    must_fail(check_removes_token_cut_by_truncation(&cut_then_remove));
}

#[test]
fn the_replacement_is_visible_so_the_reader_knows_something_was_removed() {
    let out = run(&json!({"a": format!("x{FAKE_TOKEN}y")}), &Limits::default());
    assert_eq!(out["a"], Value::String(format!("x{REDACTION}y")));
}

#[test]
fn text_around_a_token_and_non_ascii_text_are_left_intact() {
    let payload = json!({"a": format!("é日本{FAKE_TOKEN}語—{FAKE_TOKEN}ü"), "b": "plain ünïcode"});
    let out = run(&payload, &Limits::default());
    assert_eq!(
        out["a"],
        Value::String(format!("é日本{REDACTION}語—{REDACTION}ü"))
    );
    assert_eq!(out["b"], payload["b"]);
}

#[test]
fn finalize_bounds_strings_and_marks_truncation() {
    let out = run(
        &json!({"stdout": "é".repeat(1000), "stderr": "ok"}),
        &small(),
    );
    assert_eq!(out["truncated"], Value::Bool(true));
    let stdout = out["stdout"].as_str().expect("string");
    assert!(stdout.ends_with("[truncated]") && stdout.len() <= 64);
    assert_eq!(out["stderr"], "ok");
    assert!(
        run(&json!({"stderr": "ok"}), &small())
            .get("truncated")
            .is_none()
    );
}

#[test]
fn finalize_preserves_scalar_types() {
    let out = run(
        &json!({"ok": true, "n": 3, "f": 1.5, "none": null, "items": [1, "a"]}),
        &Limits::default(),
    );
    assert_eq!(
        out,
        json!({"ok": true, "n": 3, "f": 1.5, "none": null, "items": [1, "a"]})
    );
    assert!(out["ok"].is_boolean() && out["n"].is_i64() && out["f"].is_f64());
}

#[test]
fn finalize_always_returns_data_that_serialises() {
    let out = run(
        &json!({"long key": {"é": [null, 1e300, -0.5]}, "5": "intkey"}),
        &Limits::default(),
    );
    let text = to_text(&out).expect("serialise");
    assert_eq!(
        serde_json::from_str::<Value>(&text).expect("valid JSON"),
        out
    );
}

#[test]
fn finalize_refuses_a_payload_that_is_not_an_object_and_an_empty_token() {
    for payload in [json!([1, 2]), json!("text"), json!(3), Value::Null] {
        assert!(matches!(
            finalize(&payload, &token(), &Limits::default()),
            Err(OutputError::NotAnObject)
        ));
    }
    assert!(Token::new("").is_err());
}

#[test]
fn finalize_reports_a_limit_that_cannot_hold_the_marker() {
    let tiny = Limits {
        max_output_bytes: 3,
        ..Limits::default()
    };
    assert!(matches!(
        finalize(&json!({"a": "too long for the limit"}), &token(), &tiny),
        Err(OutputError::Bound(_))
    ));
}

/// `depth` objects nested in each other, built by moving the value up one level
/// at a time: the `json!` macro would copy the whole inner value at each level.
fn wrap(mut value: Value, depth: usize) -> Value {
    for _ in 0..depth {
        let mut level = serde_json::Map::new();
        level.insert("n".to_owned(), value);
        value = Value::Object(level);
    }
    value
}

fn nested(depth: usize) -> Value {
    wrap(Value::Null, depth)
}

fn object_with(key: &str, value: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(key.to_owned(), value);
    Value::Object(map)
}

#[test]
fn finalize_survives_pathological_depth() {
    let out = run(&object_with("deep", nested(500)), &Limits::default());
    let text = to_text(&out).expect("serialise");
    assert!(text.contains("[nested too deep]"));
}

#[test]
fn finalize_survives_a_hundred_thousand_levels_without_walking_them() {
    let handle = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(|| {
            let payload = object_with("deep", nested(100_000));
            let out = run(&payload, &Limits::default());
            let text = to_text(&out).expect("serialise");
            assert!(text.contains("[nested too deep]"));
            assert!(text.len() < 10_000);
        })
        .expect("spawn");
    handle.join().expect("the deep payload crashed finalize");
}

#[test]
fn a_token_hidden_deeper_than_the_limit_is_not_returned_either() {
    let value = wrap(json!(FAKE_TOKEN), 200);
    let out = run(&object_with("deep", value), &Limits::default());
    assert!(!flat(&out).contains(FAKE_TOKEN));
}

#[test]
fn to_text_is_compact_utf8_json() {
    assert_eq!(
        to_text(&json!({"a": "é", "b": [1, 2]})).expect("text"),
        r#"{"a":"é","b":[1,2]}"#
    );
}
