//! The interface every application module implements, and the generic `tcp` module.
//!
//! Why this module exists
//! Adding an application must not touch the server or the protocol: the server
//! only ever sees this trait. Modules know how to reach an application and how
//! to read its answers; every security check stays in the Hub that calls them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::protocol::{BridgeClient, BridgeConnector, BridgeError};

pub mod inkscape;

/// How long the Hub waits for the answer to a `ping`.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the `tcp` module waits before it connects again after a lost
/// connection: a bridge frees its single place a moment after the previous
/// connection closed, and an immediate attempt would be turned away.
pub const RECONNECT_DELAY: Duration = Duration::from_millis(100);
/// The reason given for an enabled application that has no module in this build.
pub const NOT_IN_BUILD: &str = "module not available in this build";

/// Whether an application can be reached, as far as a probe can tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Health {
    pub connected: bool,
    pub app_version: Option<String>,
    pub bridge_version: Option<String>,
}

/// The answer of a bridge to a script, as the application reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecReply {
    /// Whether the script ran to its end without an exception.
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub result: Value,
    /// `null`, or `{"type", "message", "traceback"}` exactly as the application gave them.
    pub error: Value,
}

/// The state of an application, as its bridge reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct StateReply {
    pub state: Value,
    /// Where the bridge says it wrote the capture; the Hub still has to accept it.
    pub screenshot: Option<PathBuf>,
    pub warnings: Vec<String>,
    pub app_version: Option<String>,
    pub bridge_version: Option<String>,
}

/// Why a module produced no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleError {
    /// The application is still busy with an earlier request.
    Busy,
    /// No answer within the allowed time.
    Timeout,
    /// The application cannot be reached; the text says why.
    Unavailable(String),
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleError::Busy => f.write_str("the application is still running a previous request"),
            ModuleError::Timeout => f.write_str("the application did not answer in time"),
            ModuleError::Unavailable(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for ModuleError {}

/// One application, as the Hub reaches it.
pub trait Module: Send + Sync {
    /// Probe the application without running anything in it.
    fn health(&self) -> Health;

    /// Run `script` once, waiting at most `timeout_s` seconds. `work_dir` is the
    /// directory the Hub prepared for this application's files.
    ///
    /// # Errors
    /// [`ModuleError`] when there is no answer from the application.
    fn execute(
        &self,
        script: &str,
        timeout_s: u32,
        work_dir: &Path,
    ) -> Result<ExecReply, ModuleError>;

    /// Ask for the current state, and for a capture written to `capture_path`.
    ///
    /// # Errors
    /// [`ModuleError`] when there is no answer from the application.
    fn state(&self, capture_path: &Path) -> Result<StateReply, ModuleError>;

    /// Release the connection. Never closes or stops the application.
    fn close(&self);
}

/// An enabled application that cannot be used: every call says why.
#[derive(Debug, Clone)]
pub struct UnavailableModule {
    reason: String,
}

impl UnavailableModule {
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        UnavailableModule {
            reason: reason.into(),
        }
    }
}

impl Module for UnavailableModule {
    fn health(&self) -> Health {
        Health::default()
    }

    fn execute(&self, _: &str, _: u32, _: &Path) -> Result<ExecReply, ModuleError> {
        Err(ModuleError::Unavailable(self.reason.clone()))
    }

    fn state(&self, _: &Path) -> Result<StateReply, ModuleError> {
        Err(ModuleError::Unavailable(self.reason.clone()))
    }

    fn close(&self) {}
}

fn text(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Keep the three documented fields of an error, each as text.
fn error_value(raw: Option<&Value>) -> Value {
    match raw {
        Some(Value::Object(error)) => json!({
            "type": text(error, "type"),
            "message": text(error, "message"),
            "traceback": text(error, "traceback"),
        }),
        _ => json!({
            "type": "BridgeError",
            "message": "the bridge reported a failure without details",
            "traceback": "",
        }),
    }
}

const MALFORMED: &str = "the bridge sent a malformed answer";

fn is_busy(object: &Map<String, Value>) -> bool {
    object.get("busy").and_then(Value::as_bool) == Some(true)
}

/// The generic module of the `tcp` mode: a bridge loaded in the application,
/// listening on a loopback port.
///
/// The connection is opened on first use. `health` and `state` may reconnect
/// once when the connection was lost; `execute` never sends a script again once
/// it may have reached the bridge.
pub struct TcpModule {
    connector: BridgeConnector,
    state_timeout: Duration,
    reconnect_delay: Duration,
    slot: Mutex<Option<Arc<BridgeClient>>>,
}

impl TcpModule {
    /// `state_timeout` bounds the wait for a `state` answer. A second attempt to
    /// connect waits [`RECONNECT_DELAY`].
    #[must_use]
    pub fn new(connector: BridgeConnector, state_timeout: Duration) -> Self {
        TcpModule {
            connector,
            state_timeout,
            reconnect_delay: RECONNECT_DELAY,
            slot: Mutex::new(None),
        }
    }

    /// The same module with another wait before a second attempt to connect.
    #[must_use]
    pub fn with_reconnect_delay(mut self, delay: Duration) -> Self {
        self.reconnect_delay = delay;
        self
    }

    fn client(&self) -> Result<Arc<BridgeClient>, BridgeError> {
        let mut slot = self
            .slot
            .lock()
            .map_err(|_| BridgeError::Disconnected { sent: false })?;
        if let Some(client) = slot.as_ref()
            && client.is_usable()
        {
            return Ok(Arc::clone(client));
        }
        let fresh = Arc::new(self.connector.connect()?);
        *slot = Some(Arc::clone(&fresh));
        Ok(fresh)
    }

    fn forget(&self, client: &Arc<BridgeClient>) {
        client.close();
        if let Ok(mut slot) = self.slot.lock()
            && slot.as_ref().is_some_and(|held| Arc::ptr_eq(held, client))
        {
            *slot = None;
        }
    }

    /// Send a request that is safe to repeat; reconnect once if the connection was lost.
    fn idempotent(
        &self,
        payload: &Value,
        timeout: Duration,
    ) -> Result<(Value, Arc<BridgeClient>), BridgeError> {
        let mut last = BridgeError::Disconnected { sent: false };
        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(self.reconnect_delay);
            }
            let client = match self.client() {
                Ok(client) => client,
                Err(error) => {
                    last = error;
                    continue;
                }
            };
            match client.request(payload.clone(), timeout) {
                Ok(answer) => return Ok((answer, client)),
                Err(error @ (BridgeError::Busy | BridgeError::Timeout)) => return Err(error),
                Err(error) => {
                    self.forget(&client);
                    last = error;
                }
            }
        }
        Err(last)
    }
}

fn module_error(error: BridgeError) -> ModuleError {
    match error {
        BridgeError::Busy => ModuleError::Busy,
        BridgeError::Timeout => ModuleError::Timeout,
        other => ModuleError::Unavailable(other.to_string()),
    }
}

impl Module for TcpModule {
    fn health(&self) -> Health {
        match self.idempotent(&json!({"type": "ping"}), HEALTH_TIMEOUT) {
            Ok((_, client)) => Health {
                connected: true,
                app_version: Some(client.hello().app_version.clone()),
                bridge_version: Some(client.hello().bridge_version.clone()),
            },
            // Busy means a script is running: the bridge is there.
            Err(BridgeError::Busy) => match self.slot.lock().ok().and_then(|slot| slot.clone()) {
                Some(client) => Health {
                    connected: true,
                    app_version: Some(client.hello().app_version.clone()),
                    bridge_version: Some(client.hello().bridge_version.clone()),
                },
                None => Health::default(),
            },
            Err(_) => Health::default(),
        }
    }

    fn execute(
        &self,
        script: &str,
        timeout_s: u32,
        work_dir: &Path,
    ) -> Result<ExecReply, ModuleError> {
        let payload = json!({
            "type": "execute",
            "script": script,
            "timeout_s": timeout_s,
            "work_dir": work_dir.to_string_lossy(),
        });
        let timeout = Duration::from_secs(u64::from(timeout_s));
        let mut attempt = 0;
        let (answer, client) = loop {
            attempt += 1;
            let client = self.client().map_err(module_error)?;
            match client.request(payload.clone(), timeout) {
                Ok(answer) => break (answer, client),
                // Nothing was written: trying on a fresh connection is not a replay.
                Err(BridgeError::Disconnected { sent: false }) if attempt < 2 => {
                    self.forget(&client);
                }
                Err(error @ (BridgeError::Busy | BridgeError::Timeout)) => {
                    return Err(module_error(error));
                }
                Err(error) => {
                    self.forget(&client);
                    return Err(module_error(error));
                }
            }
        };
        let Value::Object(answer) = answer else {
            self.forget(&client);
            return Err(ModuleError::Unavailable(MALFORMED.to_owned()));
        };
        if is_busy(&answer) {
            return Err(ModuleError::Busy);
        }
        let Some(ok) = answer.get("ok").and_then(Value::as_bool) else {
            self.forget(&client);
            return Err(ModuleError::Unavailable(MALFORMED.to_owned()));
        };
        Ok(ExecReply {
            ok,
            stdout: text(&answer, "stdout"),
            stderr: text(&answer, "stderr"),
            result: answer.get("result").cloned().unwrap_or(Value::Null),
            error: if ok {
                Value::Null
            } else {
                error_value(answer.get("error"))
            },
        })
    }

    fn state(&self, capture_path: &Path) -> Result<StateReply, ModuleError> {
        let payload = json!({
            "type": "state",
            "capture_path": capture_path.to_string_lossy(),
        });
        let (answer, client) = self
            .idempotent(&payload, self.state_timeout)
            .map_err(module_error)?;
        let Value::Object(answer) = answer else {
            self.forget(&client);
            return Err(ModuleError::Unavailable(MALFORMED.to_owned()));
        };
        if is_busy(&answer) {
            return Err(ModuleError::Busy);
        }
        if answer.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(ModuleError::Unavailable(
                "the bridge could not report the state".to_owned(),
            ));
        }
        let warnings = answer
            .get("warnings")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map_or_else(|| item.to_string(), str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(StateReply {
            state: answer.get("state").cloned().unwrap_or(Value::Null),
            screenshot: answer
                .get("screenshot")
                .and_then(Value::as_str)
                .map(PathBuf::from),
            warnings,
            app_version: Some(client.hello().app_version.clone()),
            bridge_version: Some(client.hello().bridge_version.clone()),
        })
    }

    fn close(&self) {
        if let Ok(mut slot) = self.slot.lock()
            && let Some(client) = slot.take()
        {
            client.close();
        }
    }
}
