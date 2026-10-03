//! The applications the Hub can route to, built from the configuration.
//!
//! Why this module exists
//! The model names a target as text; only an exact name of an application the
//! operator enabled may reach a module. Keeping the table of known applications
//! here means adding one never touches the server or the protocol.

use std::sync::Arc;
use std::time::Duration;

use crate::config::{AppSettings, Config, Limits, Mode};
use crate::modules::{Module, NOT_IN_BUILD, TcpModule, UnavailableModule};
use crate::protocol::BridgeConnector;
use crate::security::Token;

/// How a known application is reached and how its module is built.
pub struct KnownApp {
    pub name: &'static str,
    pub mode: Mode,
    build: fn(&AppSettings, &Token, &Limits) -> Arc<dyn Module>,
}

fn build_tcp(settings: &AppSettings, token: &Token, limits: &Limits) -> Arc<dyn Module> {
    match settings.port {
        Some(port) => Arc::new(TcpModule::new(
            BridgeConnector::new(port, token.clone(), *limits),
            Duration::from_secs(u64::from(limits.default_timeout_s)),
        )),
        None => Arc::new(UnavailableModule::new(format!(
            "apps.{}.port is not set",
            settings.name
        ))),
    }
}

/// The applications that have a module in this build.
pub const KNOWN_APPS: [KnownApp; 3] = [
    KnownApp {
        name: "gimp",
        mode: Mode::Tcp,
        build: build_tcp,
    },
    KnownApp {
        name: "krita",
        mode: Mode::Tcp,
        build: build_tcp,
    },
    KnownApp {
        name: "blender",
        mode: Mode::Tcp,
        build: build_tcp,
    },
];

struct Entry {
    name: &'static str,
    mode: Mode,
    module: Arc<dyn Module>,
}

/// The enabled applications and their modules. Nothing else can be reached.
pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    /// The production registry: one module per enabled application.
    #[must_use]
    pub fn from_config(config: &Config, token: &Token) -> Registry {
        Registry::with_modules(config, token, Vec::new())
    }

    /// Like [`Registry::from_config`], with some modules supplied by the caller.
    /// An injected module is only used when the configuration enables its
    /// application; it can never make a disabled one reachable.
    #[must_use]
    pub fn with_modules(
        config: &Config,
        token: &Token,
        mut injected: Vec<(String, Arc<dyn Module>)>,
    ) -> Registry {
        let mut entries = Vec::new();
        for settings in config.apps().iter().filter(|app| app.enabled) {
            let given = injected
                .iter()
                .position(|(name, _)| name == settings.name)
                .map(|at| injected.swap_remove(at).1);
            let module = given.unwrap_or_else(|| build(settings, token, config.limits()));
            entries.push(Entry {
                name: settings.name,
                mode: settings.mode,
                module,
            });
        }
        Registry { entries }
    }

    /// The names of the enabled applications, in configuration order.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|entry| entry.name).collect()
    }

    /// The module of an enabled application, by exact name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Module>> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| Arc::clone(&entry.module))
    }

    /// The enabled applications with their mode and module.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, Mode, Arc<dyn Module>)> + '_ {
        self.entries
            .iter()
            .map(|entry| (entry.name, entry.mode, Arc::clone(&entry.module)))
    }

    /// Release every connection. No application is closed.
    pub fn close_all(&self) {
        for entry in &self.entries {
            entry.module.close();
        }
    }
}

fn build(settings: &AppSettings, token: &Token, limits: &Limits) -> Arc<dyn Module> {
    let known = KNOWN_APPS.iter().find(|known| known.name == settings.name);
    let supported = known.is_some_and(|known| known.mode == settings.mode);
    match known {
        Some(known) if supported => (known.build)(settings, token, limits),
        Some(known) => Arc::new(UnavailableModule::new(format!(
            "mode {} is not supported for {}; use mode {}",
            settings.mode.as_str(),
            settings.name,
            known.mode.as_str()
        ))),
        None => Arc::new(UnavailableModule::new(NOT_IN_BUILD)),
    }
}
