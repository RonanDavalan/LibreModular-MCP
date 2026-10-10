//! Les applications vers lesquelles le Hub peut router, construites depuis la configuration.
//!
//! Pourquoi ce module existe
//! Le modèle désigne une cible par un texte ; seul le nom exact d'une application que
//! l'opérateur a activée peut atteindre un module. Garder ici la table des applications
//! connues fait qu'en ajouter une ne touche jamais le serveur ni le protocole.

use std::sync::Arc;
use std::time::Duration;

use std::path::Path;

use crate::config::{self, AppSettings, Config, Limits, Mode};
use crate::modules::inkscape::{INHERITED_VARIABLES, InkscapeModule, InkscapePaths, WorkFiles};
use crate::modules::libreoffice::{self, LibreOfficeModule};
use crate::modules::{
    Module, NOT_IN_BUILD, SharedConnection, TcpModule, UnavailableModule, new_connection_slot,
};
use crate::protocol::BridgeConnector;
use crate::security::{self, Token};

/// Les connexions que plusieurs cibles d'un même pont partagent, créées une fois par
/// registre : un pont n'accepte qu'une connexion, donc les cibles qu'il sert ne doivent
/// pas en ouvrir chacune une.
#[derive(Default)]
struct SharedLinks {
    by_port: Vec<(u16, SharedConnection)>,
}

impl SharedLinks {
    /// La connexion au pont sur `port`, créée par la première cible qui la demande.
    fn for_port(&mut self, port: u16) -> SharedConnection {
        if let Some((_, slot)) = self.by_port.iter().find(|(known, _)| *known == port) {
            return Arc::clone(slot);
        }
        let slot = new_connection_slot();
        self.by_port.push((port, Arc::clone(&slot)));
        slot
    }
}

/// Comment une application connue est jointe et comment son module est construit.
pub struct KnownApp {
    pub name: &'static str,
    pub mode: Mode,
    build: fn(&'static str, &AppSettings, &Token, &Limits, &mut SharedLinks) -> Arc<dyn Module>,
}

/// `app` est le nom de [`KNOWN_APPS`] : le seul nom que son pont peut annoncer.
fn build_tcp(
    app: &'static str,
    settings: &AppSettings,
    token: &Token,
    limits: &Limits,
    _shared: &mut SharedLinks,
) -> Arc<dyn Module> {
    match settings.port {
        Some(port) => Arc::new(TcpModule::new(
            BridgeConnector::new(app, port, token.clone(), *limits),
            Duration::from_secs(u64::from(limits.default_timeout_s)),
        )),
        None => Arc::new(UnavailableModule::new(format!(
            "apps.{}.port is not set",
            settings.name
        ))),
    }
}

/// Le côté du Hub des fichiers avec lesquels travaille le module Inkscape : chaque
/// contrôle est celui du module de sécurité, le module ne fait que nommer les chemins.
#[derive(Debug, Clone, Copy, Default)]
pub struct HubWorkFiles;

impl WorkFiles for HubWorkFiles {
    fn ensure_document(&self, path: &Path, initial: &[u8]) -> Result<(), String> {
        security::ensure_private_file(path, initial).map_err(|error| error.to_string())
    }

    fn write_script(&self, path: &Path, text: &[u8]) -> Result<(), String> {
        security::replace_private_file(path, text).map_err(|error| error.to_string())
    }
}

/// Inkscape exécute un processus par script, avec des chemins de programme fixes et les
/// contrôles de fichiers du Hub ; il ne reçoit jamais le jeton.
fn build_inkscape(
    _app: &'static str,
    _settings: &AppSettings,
    _token: &Token,
    limits: &Limits,
    _shared: &mut SharedLinks,
) -> Arc<dyn Module> {
    Arc::new(InkscapeModule::new(
        InkscapePaths::default(),
        *limits,
        config::environment_values(&INHERITED_VARIABLES),
        Arc::new(HubWorkFiles),
    ))
}

/// Les trois cibles de LibreOffice atteignent un seul pont, que le Hub désigne par
/// `bridge_port` : il ne parle jamais UNO lui-même, c'est le pont qui le fait.
fn build_libreoffice(
    app: &'static str,
    settings: &AppSettings,
    token: &Token,
    limits: &Limits,
    shared: &mut SharedLinks,
) -> Arc<dyn Module> {
    let Some(family) = libreoffice::family_of(app) else {
        return Arc::new(UnavailableModule::new(NOT_IN_BUILD));
    };
    let Some(port) = settings.bridge_port else {
        return Arc::new(UnavailableModule::new(format!(
            "apps.{}.bridge_port is not set",
            settings.name
        )));
    };
    Arc::new(LibreOfficeModule::new(
        BridgeConnector::new(libreoffice::BRIDGE_APP, port, token.clone(), *limits),
        Duration::from_secs(u64::from(limits.default_timeout_s)),
        family,
        shared.for_port(port),
    ))
}

/// Les applications qui ont un module dans cette construction.
pub const KNOWN_APPS: [KnownApp; 7] = [
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
    KnownApp {
        name: "inkscape",
        mode: Mode::Cli,
        build: build_inkscape,
    },
    KnownApp {
        name: "libreoffice-writer",
        mode: Mode::Uno,
        build: build_libreoffice,
    },
    KnownApp {
        name: "libreoffice-calc",
        mode: Mode::Uno,
        build: build_libreoffice,
    },
    KnownApp {
        name: "libreoffice-impress",
        mode: Mode::Uno,
        build: build_libreoffice,
    },
];

struct Entry {
    name: &'static str,
    mode: Mode,
    module: Arc<dyn Module>,
}

/// Les applications activées et leurs modules. Rien d'autre ne peut être atteint.
pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    /// Le registre de production : un module par application activée.
    #[must_use]
    pub fn from_config(config: &Config, token: &Token) -> Registry {
        Registry::with_modules(config, token, Vec::new())
    }

    /// Comme [`Registry::from_config`], avec certains modules fournis par l'appelant.
    /// Un module injecté n'est utilisé que si la configuration active son application ;
    /// il ne peut jamais rendre atteignable une application désactivée.
    #[must_use]
    pub fn with_modules(
        config: &Config,
        token: &Token,
        mut injected: Vec<(String, Arc<dyn Module>)>,
    ) -> Registry {
        let mut entries = Vec::new();
        let mut shared = SharedLinks::default();
        for settings in config.apps().iter().filter(|app| app.enabled) {
            let given = injected
                .iter()
                .position(|(name, _)| name == settings.name)
                .map(|at| injected.swap_remove(at).1);
            let module =
                given.unwrap_or_else(|| build(settings, token, config.limits(), &mut shared));
            entries.push(Entry {
                name: settings.name,
                mode: settings.mode,
                module,
            });
        }
        Registry { entries }
    }

    /// Les noms des applications activées, dans l'ordre de la configuration.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|entry| entry.name).collect()
    }

    /// Le module d'une application activée, par son nom exact.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Module>> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| Arc::clone(&entry.module))
    }

    /// Les applications activées avec leur mode et leur module.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, Mode, Arc<dyn Module>)> + '_ {
        self.entries
            .iter()
            .map(|entry| (entry.name, entry.mode, Arc::clone(&entry.module)))
    }

    /// Libère chaque connexion. Aucune application n'est fermée.
    pub fn close_all(&self) {
        for entry in &self.entries {
            entry.module.close();
        }
    }
}

fn build(
    settings: &AppSettings,
    token: &Token,
    limits: &Limits,
    shared: &mut SharedLinks,
) -> Arc<dyn Module> {
    let known = KNOWN_APPS.iter().find(|known| known.name == settings.name);
    let supported = known.is_some_and(|known| known.mode == settings.mode);
    match known {
        Some(known) if supported => (known.build)(known.name, settings, token, limits, shared),
        Some(known) => Arc::new(UnavailableModule::new(format!(
            "mode {} is not supported for {}; use mode {}",
            settings.mode.as_str(),
            settings.name,
            known.mode.as_str()
        ))),
        None => Arc::new(UnavailableModule::new(NOT_IN_BUILD)),
    }
}
