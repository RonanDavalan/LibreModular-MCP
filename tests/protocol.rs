//! The bridge client against the reference bridge.
//!
//! Why this module exists
//! The client is the only door between the Hub and code running with the user's
//! rights. These tests show that it authenticates, bounds what it reads, keeps a
//! single request in flight and never sends a script twice, and that each check
//! notices a bridge built to break it.

#[macro_use]
mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::fake_bridge::{FakeBridge, HelloMode, Reply, handler, raw_exchange};
use common::{FAKE_TOKEN, Verdict, must_fail, must_pass};
use libremodular_mcp::config::Limits;
use libremodular_mcp::protocol::{BridgeClient, BridgeError};
use libremodular_mcp::security::Token;
use serde_json::{Value, json};

fn token() -> Token {
    Token::new(FAKE_TOKEN).expect("token")
}

fn limits() -> Limits {
    Limits {
        max_line_bytes: 4096,
        ..Limits::default()
    }
}

fn ok_reply() -> Reply {
    Reply::Json(json!({"ok": true, "stdout": "", "stderr": "", "result": 1, "error": null}))
}

fn execute(script: &str) -> Value {
    json!({"type": "execute", "script": script, "timeout_s": 1, "work_dir": "/tmp/w"})
}

fn free_port() -> u16 {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind");
    listener.local_addr().expect("address").port()
}

#[test]
fn hello_is_checked_and_reports_the_bridge() {
    let bridge = FakeBridge::start(FAKE_TOKEN, HelloMode::Normal, handler(|_| ok_reply()));
    let client =
        BridgeClient::connect(bridge.port(), "gimp", &token(), &limits()).expect("connect");
    assert_eq!(client.hello().app, "gimp");
    assert_eq!(client.hello().app_version, "3.0.4");
    assert_eq!(client.hello().bridge_version, "1");
    let answer = client
        .request(execute("result = 1"), Duration::from_secs(2))
        .expect("answer");
    assert_eq!(answer["result"], 1);
    assert_eq!(answer["id"], "r1");
    assert_eq!(bridge.scripts(), vec!["result = 1".to_owned()]);
}

// --- the bridge refuses a wrong token: the reference implementation and its defect ---

/// A bridge must close, without answering, a connection whose hello carries a
/// wrong token or that does not start with a hello at all.
fn check_bridge_rejects_strangers(port: u16) -> Verdict {
    let wrong = b"{\"type\":\"hello\",\"token\":\"test-token-9999\",\"protocol\":1}\n";
    let answer = raw_exchange(port, wrong);
    ensure!(answer.is_empty(), "the bridge answered a wrong token");
    let answer = raw_exchange(port, b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    ensure!(answer.is_empty(), "the bridge answered an HTTP request");
    Ok(())
}

#[test]
fn the_reference_bridge_rejects_a_wrong_token_and_a_client_without_hello() {
    let bridge = FakeBridge::start(FAKE_TOKEN, HelloMode::Normal, handler(|_| ok_reply()));
    must_pass(check_bridge_rejects_strangers(bridge.port()));
    assert_eq!(bridge.stats().authenticated.load(Ordering::SeqCst), 0);
}

#[test]
fn counter_proof_a_bridge_that_accepts_any_token_is_detected() {
    let bridge = FakeBridge::start(
        FAKE_TOKEN,
        HelloMode::AcceptAnyToken,
        handler(|_| ok_reply()),
    );
    must_fail(check_bridge_rejects_strangers(bridge.port()));
}

#[test]
fn a_client_with_the_wrong_token_is_refused_without_the_token_in_the_message() {
    let bridge = FakeBridge::start(FAKE_TOKEN, HelloMode::Normal, handler(|_| ok_reply()));
    let wrong = Token::new("test-token-9999").expect("token");
    let error =
        BridgeClient::connect(bridge.port(), "gimp", &wrong, &limits()).expect_err("refused");
    assert!(matches!(error, BridgeError::Handshake(_)), "{error}");
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains("test-token"), "{shown}");
}

#[test]
fn every_bad_hello_is_a_typed_error_that_never_shows_the_token() {
    for mode in [
        HelloMode::Refuse,
        HelloMode::Malformed,
        HelloMode::Close,
        HelloMode::WrongProtocol,
    ] {
        let bridge = FakeBridge::start(FAKE_TOKEN, mode, handler(|_| ok_reply()));
        let error = BridgeClient::connect(bridge.port(), "gimp", &token(), &limits())
            .expect_err("a bad hello is refused");
        assert!(
            matches!(error, BridgeError::Handshake(_)),
            "{mode:?}: {error}"
        );
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains(FAKE_TOKEN), "{mode:?}: {shown}");
    }
}

// --- the bridge must be the application configured on its port --------------------

/// A way to connect that returns the application it accepted.
type Connect<'a> = &'a dyn Fn(u16) -> Result<String, BridgeError>;

/// A bridge announcing `krita` on the port configured for `gimp` must be refused
/// as a handshake failure that does not repeat the announced name.
fn check_another_application_is_refused(connect: Connect<'_>) -> Verdict {
    let bridge = FakeBridge::start(FAKE_TOKEN, HelloMode::OtherApp, handler(|_| ok_reply()));
    match connect(bridge.port()) {
        Ok(app) => Err(format!("a bridge announcing {app} was accepted for gimp")),
        Err(error @ BridgeError::Handshake(_)) => {
            let shown = format!("{error} {error:?}");
            ensure!(
                !shown.contains("krita"),
                "the announced name is repeated: {shown}"
            );
            Ok(())
        }
        Err(other) => Err(format!("not a handshake error: {other}")),
    }
}

/// Defective on purpose: a client that checks everything in the hello but the
/// application, written here without the Hub's code.
fn connect_without_checking_the_application(port: u16) -> Result<String, BridgeError> {
    let refused = || BridgeError::Handshake("defective client");
    let mut stream = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .map_err(BridgeError::Connect)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(BridgeError::Connect)?;
    let hello = json!({"type": "hello", "token": FAKE_TOKEN, "protocol": 1});
    writeln!(stream, "{hello}").map_err(BridgeError::Connect)?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|_| refused())?;
    let answer: Value = serde_json::from_str(&line).map_err(|_| refused())?;
    let valid = answer["type"] == "hello" && answer["ok"] == true && answer["protocol"] == 1;
    if !valid {
        return Err(refused());
    }
    answer["app"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(refused)
}

#[test]
fn a_bridge_announcing_another_application_is_refused() {
    must_pass(check_another_application_is_refused(&|port| {
        BridgeClient::connect(port, "gimp", &token(), &limits())
            .map(|client| client.hello().app.clone())
    }));
}

#[test]
fn counter_proof_a_client_that_ignores_the_announced_application_is_detected() {
    must_fail(check_another_application_is_refused(
        &connect_without_checking_the_application,
    ));
}

#[test]
fn nothing_listening_is_a_connection_error() {
    let error =
        BridgeClient::connect(free_port(), "gimp", &token(), &limits()).expect_err("no bridge");
    assert!(matches!(error, BridgeError::Connect(_)), "{error}");
}

// --- bounded lines -----------------------------------------------------------

fn check_endless_line_is_refused(client_limits: &Limits) -> Verdict {
    // A line far longer than the limit, never terminated.
    let bridge = FakeBridge::start(
        FAKE_TOKEN,
        HelloMode::Normal,
        handler(|_| Reply::Raw(vec![b'x'; 200_000])),
    );
    let client =
        BridgeClient::connect(bridge.port(), "gimp", &token(), client_limits).expect("connect");
    match client.request(execute("x"), Duration::from_secs(2)) {
        Err(BridgeError::LineTooLong { .. }) => Ok(()),
        other => Err(format!("an endless line gave {other:?}")),
    }
}

#[test]
fn a_line_longer_than_the_limit_is_refused_without_waiting_for_its_end() {
    must_pass(check_endless_line_is_refused(&limits()));
}

#[test]
fn counter_proof_without_a_line_limit_the_endless_line_is_not_refused() {
    let unbounded = Limits {
        max_line_bytes: usize::MAX / 2,
        ..Limits::default()
    };
    must_fail(check_endless_line_is_refused(&unbounded));
}

#[test]
fn a_request_longer_than_the_limit_is_not_sent() {
    let bridge = FakeBridge::start(FAKE_TOKEN, HelloMode::Normal, handler(|_| ok_reply()));
    let client =
        BridgeClient::connect(bridge.port(), "gimp", &token(), &limits()).expect("connect");
    let error = client
        .request(execute(&"x".repeat(10_000)), Duration::from_secs(2))
        .expect_err("too long");
    assert!(matches!(error, BridgeError::LineTooLong { .. }));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(bridge.requests(), 0);
}

#[test]
fn an_answer_with_another_id_is_a_protocol_error() {
    let bridge = FakeBridge::start(
        FAKE_TOKEN,
        HelloMode::Normal,
        handler(|_| Reply::Raw(b"{\"id\":\"r99\",\"ok\":true}\n".to_vec())),
    );
    let client =
        BridgeClient::connect(bridge.port(), "gimp", &token(), &limits()).expect("connect");
    let error = client
        .request(execute("x"), Duration::from_secs(2))
        .expect_err("wrong id");
    assert!(matches!(error, BridgeError::Protocol(_)), "{error}");
    assert!(!client.is_usable());
}

// --- one request at a time -------------------------------------------------------

#[test]
fn a_second_request_while_one_is_running_is_busy_at_once_and_not_sent() {
    let bridge = FakeBridge::start(
        FAKE_TOKEN,
        HelloMode::Normal,
        handler(|_| Reply::Late(Duration::from_millis(700), json!({"ok": true}))),
    );
    let client = Arc::new(
        BridgeClient::connect(bridge.port(), "gimp", &token(), &limits()).expect("connect"),
    );
    let first = {
        let client = Arc::clone(&client);
        std::thread::spawn(move || client.request(execute("first"), Duration::from_secs(5)))
    };
    std::thread::sleep(Duration::from_millis(150));
    let started = Instant::now();
    let second = client.request(execute("second"), Duration::from_secs(5));
    assert!(matches!(second, Err(BridgeError::Busy)), "{second:?}");
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "busy must not wait"
    );
    assert!(first.join().expect("thread").is_ok());
    assert_eq!(bridge.scripts(), vec!["first".to_owned()]);
}

#[test]
fn after_a_timeout_the_client_stays_busy_until_the_late_answer_arrives() {
    let bridge = FakeBridge::start(
        FAKE_TOKEN,
        HelloMode::Normal,
        handler(|request| {
            if request["script"] == "slow" {
                Reply::Late(
                    Duration::from_millis(800),
                    json!({"ok": true, "result": "late"}),
                )
            } else {
                Reply::Json(json!({"ok": true, "result": "fresh"}))
            }
        }),
    );
    let client =
        BridgeClient::connect(bridge.port(), "gimp", &token(), &limits()).expect("connect");
    let first = client.request(execute("slow"), Duration::from_millis(200));
    assert!(matches!(first, Err(BridgeError::Timeout)), "{first:?}");
    let second = client.request(execute("next"), Duration::from_secs(2));
    assert!(matches!(second, Err(BridgeError::Busy)), "{second:?}");
    assert_eq!(bridge.requests(), 1, "nothing is sent while busy");
    std::thread::sleep(Duration::from_millis(900));
    let third = client
        .request(execute("next"), Duration::from_secs(2))
        .expect("free again");
    assert_eq!(third["result"], "fresh", "the late answer is discarded");
    assert_eq!(bridge.scripts(), vec!["slow".to_owned(), "next".to_owned()]);
}

#[test]
fn a_connection_closed_after_the_request_is_reported_as_sent() {
    let bridge = FakeBridge::start(FAKE_TOKEN, HelloMode::Normal, handler(|_| Reply::Close));
    let client =
        BridgeClient::connect(bridge.port(), "gimp", &token(), &limits()).expect("connect");
    let error = client
        .request(execute("x"), Duration::from_secs(2))
        .expect_err("closed");
    assert!(
        matches!(error, BridgeError::Disconnected { sent: true }),
        "{error:?}"
    );
    assert!(!client.is_usable());
    let again = client.request(execute("x"), Duration::from_secs(2));
    assert!(
        matches!(again, Err(BridgeError::Disconnected { sent: false })),
        "{again:?}"
    );
    assert_eq!(bridge.requests(), 1);
}

#[test]
fn close_releases_the_connection_and_sends_nothing() {
    let bridge = FakeBridge::start(FAKE_TOKEN, HelloMode::Normal, handler(|_| ok_reply()));
    let client =
        BridgeClient::connect(bridge.port(), "gimp", &token(), &limits()).expect("connect");
    client.close();
    assert!(!client.is_usable());
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(bridge.requests(), 0);
}
