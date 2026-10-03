//! The Python bridges seen from the Hub: their own test suite, and the generated
//! GIMP bridge driven end to end by the Hub's `tcp` module.
//!
//! Why this module exists
//! `cargo test` is the one command that proves the product works. It must run
//! the Python tests of the bridges, fail when Python is missing rather than skip,
//! and show that the Hub's client and a real bridge file understand each other.

#[macro_use]
mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use common::{FILE_TOKEN, Verdict, must_fail, must_pass, write_file};
use libremodular_mcp::config::Limits;
use libremodular_mcp::modules::{Module, TcpModule};
use libremodular_mcp::protocol::BridgeConnector;
use libremodular_mcp::security::Token;
use serde_json::json;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const LISTENING: &str = "LMM bridge listening on 127.0.0.1:";

fn generated(name: &str) -> PathBuf {
    Path::new(ROOT).join("bridges").join("generated").join(name)
}

#[test]
fn the_python_tests_of_the_bridges_pass() {
    let output = Command::new("python3")
        .args(["-m", "unittest", "discover", "-s", "tests/bridges", "-v"])
        .current_dir(ROOT)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env_remove("LIBREMODULAR_BRIDGE_PORT")
        .env_remove("LIBREMODULAR_TOKEN_PATH")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("python3 is required to test the bridges: {error}"));
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the Python tests of the bridges failed:\n{report}"
    );
    let ran = report
        .lines()
        .find_map(|line| line.strip_prefix("Ran "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|count| count.parse::<u32>().ok())
        .unwrap_or(0);
    assert!(ran > 0, "no Python test was found:\n{report}");
    assert!(!report.contains(FILE_TOKEN));
}

/// A bridge running as a Python program; it is killed when dropped, whatever
/// the test concluded.
struct BridgeProcess {
    child: Child,
    port: u16,
    lines: Receiver<String>,
}

impl BridgeProcess {
    fn start(bridge_file: &Path, token_file: &Path) -> Result<BridgeProcess, String> {
        let mut child = Command::new("python3")
            .arg(bridge_file)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("LIBREMODULAR_BRIDGE_PORT", "0")
            .env("LIBREMODULAR_TOKEN_PATH", token_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("python3 is required: {error}"))?;
        let stderr = child.stderr.take();
        let (sender, lines) = mpsc::channel();
        let mut process = BridgeProcess {
            child,
            port: 0,
            lines,
        };
        let stderr = stderr.ok_or("the bridge has no error stream")?;
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = process
                .lines
                .recv_timeout(remaining)
                .map_err(|_| "the bridge never said where it listens".to_owned())?;
            ensure!(!line.contains(FILE_TOKEN), "the bridge printed the token");
            if let Some(port) = line.strip_prefix(LISTENING) {
                process.port = port
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad line {line}"))?;
                return Ok(process);
            }
        }
    }

    /// Everything the bridge wrote on its error stream since the listening line.
    fn said(&self) -> Vec<String> {
        self.lines.try_iter().collect()
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The Hub's own module must reach the bridge, run a script, get a Python
/// traceback back, read the state of an application without API, and probe it.
fn check_the_hub_drives_the_bridge(bridge_file: &Path) -> Verdict {
    let base = common::private_tempdir();
    let token_file = base.path().join("token");
    write_file(&token_file, 0o600, FILE_TOKEN);
    let work_dir = common::fresh(base.path());
    let captures = common::fresh(base.path());
    let bridge = BridgeProcess::start(bridge_file, &token_file)?;
    let token = Token::new(FILE_TOKEN).map_err(|error| error.to_string())?;
    let connector = BridgeConnector::new("gimp", bridge.port, token, Limits::default());
    let module = TcpModule::new(connector.clone(), Duration::from_secs(10));
    let outcome = (|| -> Verdict {
        let health = module.health();
        if !health.connected {
            // Give the bridge time to notice the closed connection and free its
            // single place, then ask the handshake itself what went wrong.
            std::thread::sleep(Duration::from_millis(300));
            let why = connector
                .connect()
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default();
            return Err(format!("health: the bridge is not reachable ({why})"));
        }
        ensure!(
            health.app_version.as_deref() == Some("unknown")
                && health.bridge_version.as_deref() == Some("1"),
            "health: {health:?}"
        );
        let script = "print('hello from the bridge')\nresult = {'sum': 1 + 2, 'dir': LMM_WORK_DIR}";
        let reply = module
            .execute(script, 10, &work_dir)
            .map_err(|error| error.to_string())?;
        ensure!(reply.ok, "the script failed: {reply:?}");
        ensure!(reply.stdout == "hello from the bridge\n", "{reply:?}");
        ensure!(
            reply.result == json!({"sum": 3, "dir": work_dir.to_string_lossy()}),
            "{reply:?}"
        );
        let reply = module
            .execute("x = 1\ny = undefined_name\n", 10, &work_dir)
            .map_err(|error| error.to_string())?;
        ensure!(!reply.ok, "a NameError was reported as success");
        ensure!(reply.error["type"] == "NameError", "{reply:?}");
        let traceback = reply.error["traceback"].as_str().unwrap_or_default();
        ensure!(
            traceback.contains("File \"<libremodular>\", line 2, in <module>"),
            "{traceback}"
        );
        let state = module
            .state(&captures.join("gimp-e2e.png"))
            .map_err(|error| error.to_string())?;
        ensure!(state.state == json!({}), "{state:?}");
        ensure!(state.screenshot.is_none(), "{state:?}");
        ensure!(state.app_version.as_deref() == Some("unknown"), "{state:?}");
        ensure!(
            state
                .warnings
                .iter()
                .any(|warning| warning.contains("cannot be imported")),
            "{state:?}"
        );
        Ok(())
    })();
    module.close();
    let said = bridge.said();
    ensure!(
        !said.iter().any(|line| line.contains(FILE_TOKEN)),
        "the bridge printed the token"
    );
    outcome
}

#[test]
fn the_hub_tcp_module_drives_the_generated_gimp_bridge() {
    must_pass(check_the_hub_drives_the_bridge(&generated(
        "gimp_bridge.py",
    )));
}

#[test]
fn counter_proof_a_bridge_whose_hello_lacks_the_protocol_is_refused() {
    let base = common::private_tempdir();
    let text = fs::read_to_string(generated("gimp_bridge.py")).expect("generated bridge");
    let line = "                \"protocol\": PROTOCOL_VERSION,\n";
    assert_eq!(
        text.matches(line).count(),
        1,
        "the defect must change one place"
    );
    let defective = base.path().join("gimp_bridge_without_protocol.py");
    fs::write(&defective, text.replacen(line, "", 1)).expect("write the defective bridge");
    must_fail(check_the_hub_drives_the_bridge(&defective));
}
