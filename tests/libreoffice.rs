//! The LibreOffice module of the Hub: three targets that share one bridge.
//!
//! Why this module exists
//! Writer, Calc and Impress reach the same bridge, which accepts one connection
//! and runs one script at a time. These tests show that the three targets open a
//! single connection, name their family in every request, share the busy state,
//! and report the absence of an active document, each check failing on a set of
//! modules built to break it.

#[macro_use]
mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fake_bridge::{FakeBridge, HelloMode, Reply, handler};
use common::hub::{Session, build, config};
use common::{FAKE_TOKEN, Verdict, must_fail, must_pass};
use libremodular_mcp::config::Limits;
use libremodular_mcp::modules::libreoffice::{BRIDGE_APP, LibreOfficeModule, family_of};
use libremodular_mcp::modules::{Module, ModuleError, TcpModule, new_connection_slot};
use libremodular_mcp::protocol::BridgeConnector;
use libremodular_mcp::registry::Registry;
use libremodular_mcp::security::Token;
use serde_json::{Value, json};

const TARGETS: [&str; 3] = [
    "libreoffice-writer",
    "libreoffice-calc",
    "libreoffice-impress",
];
const WORK: &str = "/home/alice/work";

fn token() -> Token {
    Token::new(FAKE_TOKEN).expect("token")
}

fn connector(port: u16) -> BridgeConnector {
    BridgeConnector::new(BRIDGE_APP, port, token(), Limits::default())
}

fn ok_script() -> Reply {
    Reply::Json(json!({"ok": true, "stdout": "", "stderr": "", "result": 1, "error": null}))
}

fn ok_state() -> Reply {
    Reply::Json(json!({
        "ok": true,
        "state": {"documents": [], "active": {"slides": 3}},
        "screenshot": null,
        "warnings": [],
    }))
}

fn answers_everything() -> common::fake_bridge::Handler {
    handler(|request| {
        if request["type"] == "state" {
            ok_state()
        } else {
            ok_script()
        }
    })
}

fn config_for_port(port: u16) -> libremodular_mcp::config::Config {
    config(&format!(
        "[apps.libreoffice-writer]\nenabled = true\nbridge_port = {port}\n\
         [apps.libreoffice-calc]\nenabled = true\nbridge_port = {port}\n\
         [apps.libreoffice-impress]\nenabled = true\nbridge_port = {port}\n"
    ))
}

/// How the three modules are made: the registry, or something built to be wrong.
type Make = fn(u16) -> Vec<Arc<dyn Module>>;

/// The production way: a registry built from the configuration.
fn from_the_registry(port: u16) -> Vec<Arc<dyn Module>> {
    let registry = Registry::from_config(&config_for_port(port), &token());
    TARGETS
        .iter()
        .map(|target| registry.get(target).expect("an enabled target"))
        .collect()
}

/// Defective on purpose: each target opens a connection of its own.
fn one_connection_per_target(port: u16) -> Vec<Arc<dyn Module>> {
    TARGETS
        .iter()
        .map(|target| {
            Arc::new(LibreOfficeModule::new(
                connector(port),
                Duration::from_secs(5),
                family_of(target).expect("family"),
                new_connection_slot(),
            )) as Arc<dyn Module>
        })
        .collect()
}

/// Defective on purpose: one shared connection, but the family is never named.
fn without_the_family(port: u16) -> Vec<Arc<dyn Module>> {
    let shared = new_connection_slot();
    TARGETS
        .iter()
        .map(|_| {
            Arc::new(
                TcpModule::new(connector(port), Duration::from_secs(5))
                    .with_shared_connection(Arc::clone(&shared)),
            ) as Arc<dyn Module>
        })
        .collect()
}

/// Defective on purpose: the shared connection and the family, but no word about
/// an absent active document.
fn without_the_warning(port: u16) -> Vec<Arc<dyn Module>> {
    let shared = new_connection_slot();
    TARGETS
        .iter()
        .map(|target| {
            Arc::new(
                TcpModule::new(connector(port), Duration::from_secs(5))
                    .with_family(family_of(target).expect("family"))
                    .with_shared_connection(Arc::clone(&shared)),
            ) as Arc<dyn Module>
        })
        .collect()
}

// --- the names ------------------------------------------------------------------

#[test]
fn each_target_names_its_family_and_nothing_else_does() {
    assert_eq!(family_of("libreoffice-writer"), Some("writer"));
    assert_eq!(family_of("libreoffice-calc"), Some("calc"));
    assert_eq!(family_of("libreoffice-impress"), Some("impress"));
    for other in [
        "libreoffice",
        "libreoffice-draw",
        "libreoffice-",
        "gimp",
        "Libreoffice-calc",
    ] {
        assert_eq!(family_of(other), None, "{other}");
    }
    assert_eq!(BRIDGE_APP, "libreoffice");
}

// --- one connection ---------------------------------------------------------------

/// A bridge with a single place, like the real one: the three targets must all be
/// served, and by a single connection.
fn check_the_targets_open_one_connection(make: Make) -> Verdict {
    let bridge =
        FakeBridge::start_single_place(FAKE_TOKEN, HelloMode::Libreoffice, answers_everything());
    let modules = make(bridge.port());
    for (target, module) in TARGETS.iter().zip(&modules) {
        let health = module.health();
        ensure!(health.connected, "{target}: the bridge is not reachable");
        let reply = module
            .execute("x = 1", 5, Path::new(WORK))
            .map_err(|error| format!("{target}: {error}"))?;
        ensure!(reply.ok, "{target}: {reply:?}");
    }
    ensure!(
        bridge.connections() == 1,
        "{} connections were opened",
        bridge.connections()
    );
    for module in &modules {
        module.close();
    }
    Ok(())
}

#[test]
fn the_three_targets_share_the_single_connection_of_the_bridge() {
    must_pass(check_the_targets_open_one_connection(from_the_registry));
}

#[test]
fn counter_proof_a_connection_per_target_is_turned_away() {
    must_fail(check_the_targets_open_one_connection(
        one_connection_per_target,
    ));
}

// --- the family -----------------------------------------------------------------

type Seen = Arc<Mutex<Vec<(String, Value)>>>;

fn recording_bridge(seen: &Seen) -> FakeBridge {
    let record = Arc::clone(seen);
    FakeBridge::start_single_place(
        FAKE_TOKEN,
        HelloMode::Libreoffice,
        handler(move |request| {
            record.lock().expect("seen").push((
                request["type"].as_str().unwrap_or("").to_owned(),
                request["family"].clone(),
            ));
            if request["type"] == "state" {
                ok_state()
            } else {
                ok_script()
            }
        }),
    )
}

/// Every execute and state request names the family of its target.
fn check_every_request_names_its_family(make: Make) -> Verdict {
    let seen: Seen = Arc::default();
    let bridge = recording_bridge(&seen);
    let modules = make(bridge.port());
    for (target, module) in TARGETS.iter().zip(&modules) {
        module
            .execute("x = 1", 5, Path::new(WORK))
            .map_err(|error| format!("{target}: execute: {error}"))?;
        module
            .state(Path::new("/run/user/1000/captures/x.png"), Path::new(WORK))
            .map_err(|error| format!("{target}: state: {error}"))?;
    }
    let expected: Vec<(String, Value)> = [
        ("execute", "writer"),
        ("state", "writer"),
        ("execute", "calc"),
        ("state", "calc"),
        ("execute", "impress"),
        ("state", "impress"),
    ]
    .iter()
    .map(|(kind, family)| ((*kind).to_owned(), json!(family)))
    .collect();
    let got = seen.lock().expect("seen").clone();
    ensure!(got == expected, "requests seen by the bridge: {got:?}");
    Ok(())
}

#[test]
fn every_request_carries_the_family_of_its_target() {
    must_pass(check_every_request_names_its_family(from_the_registry));
}

#[test]
fn counter_proof_requests_without_a_family_are_detected() {
    must_fail(check_every_request_names_its_family(without_the_family));
}

#[test]
fn a_tcp_target_never_sends_a_family() {
    let seen: Seen = Arc::default();
    let bridge = recording_bridge(&seen);
    let module = TcpModule::new(connector(bridge.port()), Duration::from_secs(5));
    module
        .execute("x = 1", 5, Path::new(WORK))
        .expect("execute");
    let got = seen.lock().expect("seen").clone();
    assert_eq!(got, vec![("execute".to_owned(), Value::Null)]);
}

// --- the busy state ----------------------------------------------------------------

/// A script running on Calc makes Writer answer busy, and nothing is sent for it.
fn check_a_running_script_makes_the_other_targets_busy(make: Make) -> Verdict {
    let bridge = FakeBridge::start_single_place(
        FAKE_TOKEN,
        HelloMode::Libreoffice,
        handler(|_| Reply::Late(Duration::from_millis(800), ok_script_value())),
    );
    let modules = make(bridge.port());
    let (writer, calc) = (Arc::clone(&modules[0]), Arc::clone(&modules[1]));
    // The bridge is reached once before, so that connecting is not what is timed.
    ensure!(writer.health().connected, "the bridge is not reachable");
    let running = std::thread::spawn(move || calc.execute("slow()", 5, Path::new(WORK)));
    let deadline = Instant::now() + Duration::from_secs(5);
    while bridge.scripts().is_empty() {
        if running.is_finished() {
            let ended = running
                .join()
                .map_err(|_| "the first script panicked".to_owned())?;
            return Err(format!(
                "the first script ended before it arrived: {ended:?}"
            ));
        }
        ensure!(Instant::now() < deadline, "the first script never arrived");
        std::thread::sleep(Duration::from_millis(2));
    }
    let second = writer.execute("x = 1", 5, Path::new(WORK));
    ensure!(
        matches!(second, Err(ModuleError::Busy)),
        "a second target during a script answered {second:?}"
    );
    ensure!(
        bridge.scripts() == vec!["slow()".to_owned()],
        "scripts sent: {:?}",
        bridge.scripts()
    );
    let first = running
        .join()
        .map_err(|_| "the first script panicked".to_owned())?;
    ensure!(first.is_ok(), "{first:?}");
    let after = writer.execute("x = 1", 5, Path::new(WORK));
    ensure!(
        after.is_ok(),
        "the target stayed busy afterwards: {after:?}"
    );
    Ok(())
}

fn ok_script_value() -> Value {
    json!({"ok": true, "stdout": "", "stderr": "", "result": null, "error": null})
}

#[test]
fn a_script_on_one_target_makes_another_answer_busy() {
    must_pass(check_a_running_script_makes_the_other_targets_busy(
        from_the_registry,
    ));
}

#[test]
fn counter_proof_targets_with_connections_of_their_own_do_not_share_the_busy_state() {
    must_fail(check_a_running_script_makes_the_other_targets_busy(
        one_connection_per_target,
    ));
}

// --- the state --------------------------------------------------------------------

fn state_bridge(active: Value) -> FakeBridge {
    FakeBridge::start_single_place(
        FAKE_TOKEN,
        HelloMode::Libreoffice,
        handler(move |_| {
            Reply::Json(json!({
                "ok": true,
                "state": {"documents": [], "active": active.clone()},
                "screenshot": null,
                "warnings": ["from the bridge"],
            }))
        }),
    )
}

/// An absent active document is said in a warning; a present one is not.
fn check_an_absent_active_document_is_reported(make: Make) -> Verdict {
    let absent = state_bridge(Value::Null);
    let reply = make(absent.port())[1]
        .state(Path::new("/run/user/1000/captures/x.png"), Path::new(WORK))
        .map_err(|error| format!("state: {error}"))?;
    ensure!(
        reply.warnings.len() == 2 && reply.warnings[0] == "from the bridge",
        "{:?}",
        reply.warnings
    );
    ensure!(
        reply.warnings[1].contains("no active calc document")
            && reply.warnings[1].contains("not a calc document"),
        "{:?}",
        reply.warnings
    );
    ensure!(reply.state["active"] == Value::Null, "{:?}", reply.state);
    let present = state_bridge(json!({"sheets": []}));
    let reply = make(present.port())[1]
        .state(Path::new("/run/user/1000/captures/x.png"), Path::new(WORK))
        .map_err(|error| format!("state: {error}"))?;
    ensure!(
        reply.warnings == ["from the bridge"],
        "{:?}",
        reply.warnings
    );
    Ok(())
}

#[test]
fn an_absent_active_document_is_reported_in_a_warning_and_a_present_one_is_not() {
    must_pass(check_an_absent_active_document_is_reported(
        from_the_registry,
    ));
}

#[test]
fn counter_proof_a_missing_warning_is_detected() {
    must_fail(check_an_absent_active_document_is_reported(
        without_the_warning,
    ));
}

#[test]
fn a_state_without_an_active_part_gets_no_such_warning() {
    // An empty state is what the bridge sends without LibreOffice: it is not an
    // absent document.
    let bridge = FakeBridge::start_single_place(
        FAKE_TOKEN,
        HelloMode::Libreoffice,
        handler(|_| {
            Reply::Json(json!({"ok": true, "state": {}, "screenshot": null,
                "warnings": ["the libreoffice scripting API cannot be imported in this process"]}))
        }),
    );
    let reply = from_the_registry(bridge.port())[0]
        .state(Path::new("/run/user/1000/captures/x.png"), Path::new(WORK))
        .expect("state");
    assert_eq!(reply.warnings.len(), 1, "{:?}", reply.warnings);
}

// --- the Hub -----------------------------------------------------------------------

#[test]
fn the_hub_lists_the_three_targets_in_uno_mode_with_the_same_health() {
    let base = common::private_tempdir();
    let bridge =
        FakeBridge::start_single_place(FAKE_TOKEN, HelloMode::Libreoffice, answers_everything());
    let hub = build(base.path(), config_for_port(bridge.port()), Vec::new());
    let mut session = Session::start(hub.server.clone());
    let listing = session.state("hub");
    let modules = listing["state"]["modules"].as_array().expect("modules");
    let names: Vec<&str> = modules.iter().filter_map(|m| m["name"].as_str()).collect();
    assert_eq!(names, TARGETS.to_vec());
    for module in modules {
        assert_eq!(module["mode"], "uno", "{module}");
        assert_eq!(module["connected"], true, "{module}");
        assert_eq!(module["app_version"], "3.0.4", "{module}");
        assert_eq!(module["bridge_version"], "1", "{module}");
    }
    assert_eq!(bridge.connections(), 1, "one connection for the three");
    assert_eq!(bridge.requests(), 3, "one probe per target, nothing more");
    assert!(bridge.kinds().iter().all(|kind| kind == "ping"));
    assert!(bridge.scripts().is_empty());
}

#[test]
fn a_script_and_a_state_reach_the_bridge_through_the_hub_with_the_family() {
    let base = common::private_tempdir();
    let seen: Seen = Arc::default();
    let bridge = recording_bridge(&seen);
    let hub = build(base.path(), config_for_port(bridge.port()), Vec::new());
    let mut session = Session::start(hub.server.clone());
    let answer = session.execute("libreoffice-impress", "result = 1");
    assert_eq!(answer["status"], "ok", "{answer}");
    let state = session.state("libreoffice-calc");
    assert_eq!(state["connected"], true, "{state}");
    assert_eq!(state["state"]["active"], json!({"slides": 3}));
    let got = seen.lock().expect("seen").clone();
    assert_eq!(
        got,
        vec![
            ("execute".to_owned(), json!("impress")),
            ("state".to_owned(), json!("calc")),
        ]
    );
    assert_eq!(bridge.connections(), 1);
}

#[test]
fn a_target_that_is_not_enabled_stays_unreachable_when_another_is() {
    let base = common::private_tempdir();
    let bridge =
        FakeBridge::start_single_place(FAKE_TOKEN, HelloMode::Libreoffice, answers_everything());
    let configuration = config(&format!(
        "[apps.libreoffice-calc]\nenabled = true\nbridge_port = {}\n",
        bridge.port()
    ));
    let hub = build(base.path(), configuration, Vec::new());
    let mut session = Session::start(hub.server.clone());
    assert_eq!(session.execute("libreoffice-calc", "x = 1")["status"], "ok");
    let refused = session.execute("libreoffice-writer", "x = 1");
    assert_eq!(refused["status"], "refused", "{refused}");
    assert_eq!(bridge.scripts().len(), 1);
}

#[test]
fn targets_configured_on_two_ports_use_two_connections() {
    let base = common::private_tempdir();
    let first =
        FakeBridge::start_single_place(FAKE_TOKEN, HelloMode::Libreoffice, answers_everything());
    let second =
        FakeBridge::start_single_place(FAKE_TOKEN, HelloMode::Libreoffice, answers_everything());
    let configuration = config(&format!(
        "[apps.libreoffice-writer]\nenabled = true\nbridge_port = {}\n\
         [apps.libreoffice-calc]\nenabled = true\nbridge_port = {}\n\
         [apps.libreoffice-impress]\nenabled = true\nbridge_port = {}\n",
        first.port(),
        second.port(),
        first.port()
    ));
    let hub = build(base.path(), configuration, Vec::new());
    let mut session = Session::start(hub.server.clone());
    for target in TARGETS {
        assert_eq!(session.execute(target, "x = 1")["status"], "ok", "{target}");
    }
    assert_eq!(
        first.connections(),
        1,
        "writer and impress share their bridge"
    );
    assert_eq!(second.connections(), 1);
    assert_eq!(first.scripts().len(), 2);
    assert_eq!(second.scripts().len(), 1);
}

// --- closing ------------------------------------------------------------------------

#[test]
fn closing_the_three_modules_is_harmless_and_sends_nothing() {
    let bridge =
        FakeBridge::start_single_place(FAKE_TOKEN, HelloMode::Libreoffice, answers_everything());
    let modules = from_the_registry(bridge.port());
    assert!(modules[0].health().connected);
    let requests = bridge.requests();
    for module in &modules {
        module.close();
        module.close();
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        bridge.requests(),
        requests,
        "closing sends nothing to the bridge"
    );
    // The connection is opened again on the next use, by whichever target asks.
    assert!(modules[2].health().connected);
    assert_eq!(bridge.connections(), 2);
}

// --- a mode the module does not serve -----------------------------------------------

#[test]
fn a_libreoffice_target_in_another_mode_is_unavailable_with_the_reason() {
    let base = common::private_tempdir();
    let hub = build(
        base.path(),
        config("[apps.libreoffice-calc]\nenabled = true\nmode = \"tcp\"\n"),
        Vec::new(),
    );
    let mut session = Session::start(hub.server.clone());
    let answer = session.execute("libreoffice-calc", "x = 1");
    assert_eq!(answer["status"], "unavailable", "{answer}");
    assert_eq!(
        answer["reason"],
        "mode tcp is not supported for libreoffice-calc; use mode uno"
    );
}
