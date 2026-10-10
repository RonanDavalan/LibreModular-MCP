//! Le module `uno` de LibreOffice : trois cibles, un pont, une connexion.
//!
//! Pourquoi ce module existe
//! Writer, Calc et Impress sont trois cibles d'un même LibreOffice, et son pont n'accepte qu'une
//! connexion. Le Hub ne parle jamais UNO : il parle au pont comme à n'importe quel autre, en
//! nommant la famille dans chaque requête, et les trois cibles doivent partager cette connexion et
//! son état d'occupation pour qu'un script qui tourne sur l'une ne soit jamais suivi d'un autre sur
//! un document différent.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::modules::{
    ExecReply, Health, Module, ModuleError, SharedConnection, StateReply, TcpModule,
};
use crate::protocol::BridgeConnector;

/// Le nom que le pont de LibreOffice annonce dans son `hello`, quelle que soit la cible.
pub const BRIDGE_APP: &str = "libreoffice";

/// Le préfixe des noms des trois cibles.
const TARGET_PREFIX: &str = "libreoffice-";

/// La famille qu'une requête au pont nomme pour `target` : `writer`, `calc` ou `impress`.
#[must_use]
pub fn family_of(target: &str) -> Option<&'static str> {
    match target.strip_prefix(TARGET_PREFIX)? {
        "writer" => Some("writer"),
        "calc" => Some("calc"),
        "impress" => Some("impress"),
        _ => None,
    }
}

/// Une cible de LibreOffice. Elle délègue au module générique `tcp`, qui possède la connexion
/// paresseuse et la seconde tentative, et ajoute ce que seule cette application sait dire de son état.
pub struct LibreOfficeModule {
    inner: TcpModule,
    family: &'static str,
}

impl LibreOfficeModule {
    /// `shared` est la connexion du pont, la même pour les trois cibles.
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

    /// Le même module avec une autre attente avant une seconde tentative de connexion.
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
        // Le pont rapporte `active: null` quand LibreOffice n'a aucun document courant ou un document
        // d'une autre famille, et ne peut pas dire lequel : on le dit ici, pour que l'absence ne soit jamais
        // lue comme un document vide.
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
