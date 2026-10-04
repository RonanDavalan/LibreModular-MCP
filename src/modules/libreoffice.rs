//! The `uno` module of LibreOffice: three targets, one bridge, one connection.
//!
//! Why this module exists
//! Writer, Calc and Impress are three targets of one LibreOffice, and its bridge
//! accepts a single connection. The Hub never speaks UNO: it talks to the bridge
//! like to any other, naming the family in each request, and the three targets
//! must share that connection and its busy state so that a script running on one
//! is never followed by another on a different document.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::modules::{
    ExecReply, Health, Module, ModuleError, SharedConnection, StateReply, TcpModule,
};
use crate::protocol::BridgeConnector;

/// The name the bridge of LibreOffice announces in its `hello`, whatever the target.
pub const BRIDGE_APP: &str = "libreoffice";

/// The prefix of the three targets' names.
const TARGET_PREFIX: &str = "libreoffice-";

/// The family a bridge request names for `target`: `writer`, `calc` or `impress`.
#[must_use]
pub fn family_of(target: &str) -> Option<&'static str> {
    match target.strip_prefix(TARGET_PREFIX)? {
        "writer" => Some("writer"),
        "calc" => Some("calc"),
        "impress" => Some("impress"),
        _ => None,
    }
}

/// One target of LibreOffice. It delegates to the generic `tcp` module, which
/// owns the lazy connection and the second attempt, and adds what only this
/// application knows how to say about its state.
pub struct LibreOfficeModule {
    inner: TcpModule,
    family: &'static str,
}

impl LibreOfficeModule {
    /// `shared` is the connection of the bridge, the same for the three targets.
    #[must_use]
    pub fn new(
        connector: BridgeConnector,
        state_timeout: Duration,
        family: &'static str,
        shared: SharedConnection,
    ) -> Self {
        LibreOfficeModule {
            inner: TcpModule::new(connector, state_timeout)
                .with_family(family)
                .with_shared_connection(shared),
            family,
        }
    }

    /// The same module with another wait before a second attempt to connect.
    #[must_use]
    pub fn with_reconnect_delay(mut self, delay: Duration) -> Self {
        self.inner = self.inner.with_reconnect_delay(delay);
        self
    }
}

impl Module for LibreOfficeModule {
    fn health(&self) -> Health {
        self.inner.health()
    }

    fn execute(
        &self,
        script: &str,
        timeout_s: u32,
        work_dir: &Path,
    ) -> Result<ExecReply, ModuleError> {
        self.inner.execute(script, timeout_s, work_dir)
    }

    fn state(&self, capture_path: &Path, work_dir: &Path) -> Result<StateReply, ModuleError> {
        let mut reply = self.inner.state(capture_path, work_dir)?;
        // The bridge reports `active: null` when LibreOffice has no current
        // document or one of another family, and cannot say which: say it here,
        // so that the absence is never read as an empty document.
        if reply.state.get("active") == Some(&Value::Null) {
            reply.warnings.push(format!(
                "no active {family} document: LibreOffice has no current document, \
                 or its current document is not a {family} document",
                family = self.family
            ));
        }
        Ok(reply)
    }

    fn close(&self) {
        self.inner.close();
    }
}
