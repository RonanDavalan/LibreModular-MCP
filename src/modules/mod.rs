//! L'interface que chaque module d'application implémente, et le module générique `tcp`.
//!
//! Pourquoi ce module existe
//! Ajouter une application ne doit toucher ni le serveur ni le protocole : le serveur ne voit
//! jamais que ce trait. Les modules savent comment joindre une application et lire ses réponses ;
//! chaque contrôle de sécurité reste dans le Hub qui les appelle.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::protocol::{BridgeClient, BridgeConnector, BridgeError};

pub mod inkscape;
pub mod libreoffice;

/// Combien de temps le Hub attend la réponse à un `ping`.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
/// Combien de temps le module `tcp` attend avant de se reconnecter après une connexion perdue :
/// un pont libère sa place unique un instant après la fermeture de la connexion précédente, et
/// une tentative immédiate serait refusée.
pub const RECONNECT_DELAY: Duration = Duration::from_millis(100);
/// La raison donnée pour une application activée qui n'a pas de module dans cette construction.
pub const NOT_IN_BUILD: &str = "module not available in this build";

/// Dit si une application peut être jointe, pour autant qu'une sonde puisse le dire.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Health {
    pub connected: bool,
    pub app_version: Option<String>,
    pub bridge_version: Option<String>,
}

/// La réponse d'un pont à un script, telle que l'application l'a rapportée.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecReply {
    /// Dit si le script est allé jusqu'au bout sans exception.
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub result: Value,
    /// `null`, ou `{"type", "message", "traceback"}` exactement comme l'application les a donnés.
    pub error: Value,
}

/// L'état d'une application, tel que son pont l'a rapporté.
#[derive(Debug, Clone, PartialEq)]
pub struct StateReply {
    pub state: Value,
    /// Où le pont dit avoir écrit la capture ; le Hub doit encore l'accepter.
    pub screenshot: Option<PathBuf>,
    pub warnings: Vec<String>,
    pub app_version: Option<String>,
    pub bridge_version: Option<String>,
}

/// Pourquoi un module n'a produit aucune réponse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleError {
    /// L'application est encore occupée par une requête antérieure.
    Busy,
    /// Aucune réponse dans le délai permis.
    Timeout,
    /// L'application ne peut pas être jointe ; le texte dit pourquoi.
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

/// Une application, telle que le Hub la joint.
pub trait Module: Send + Sync {
    /// Sonde l'application sans rien y exécuter.
    fn health(&self) -> Health;

    /// Exécute `script` une fois, en attendant au plus `timeout_s` secondes. `work_dir` est le
    /// répertoire que le Hub a préparé pour les fichiers de cette application.
    ///
    /// # Erreurs
    /// [`ModuleError`] quand l'application ne répond pas.
    fn execute(
        &self,
        script: &str,
        timeout_s: u32,
        work_dir: &Path,
    ) -> Result<ExecReply, ModuleError>;

    /// Demande l'état courant, et une capture écrite dans `capture_path`. `work_dir` est le même
    /// répertoire que [`Module::execute`] reçoit pour cette application, pour qu'un module dont le
    /// document vit là puisse le lire sans avoir exécuté de script d'abord.
    ///
    /// # Erreurs
    /// [`ModuleError`] quand l'application ne répond pas.
    fn state(&self, capture_path: &Path, work_dir: &Path) -> Result<StateReply, ModuleError>;

    /// Libère la connexion. Ne ferme ni n'arrête jamais l'application.
    fn close(&self);
}

/// Une application activée qui ne peut pas être utilisée : chaque appel dit pourquoi.
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

    fn state(&self, _: &Path, _: &Path) -> Result<StateReply, ModuleError> {
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

/// Garde les trois champs documentés d'une erreur, chacun en texte.
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

/// Le module générique du mode `tcp` : un pont chargé dans l'application, qui écoute sur un port
/// de bouclage.
///
/// La connexion est ouverte au premier usage. `health` et `state` peuvent se reconnecter une fois
/// quand la connexion a été perdue ; `execute` ne le fait que si rien du script n'a été écrit, et
/// n'envoie jamais un script une seconde fois dès qu'il a pu atteindre le pont. Toute seconde
/// tentative attend d'abord [`RECONNECT_DELAY`].
pub struct TcpModule {
    connector: BridgeConnector,
    state_timeout: Duration,
    reconnect_delay: Duration,
    /// La famille nommée dans chaque requête, pour un pont qui en sert plusieurs.
    family: Option<&'static str>,
    slot: SharedConnection,
}

/// L'unique connexion à un pont. Un pont n'accepte qu'une connexion : les modules de plusieurs
/// cibles qu'un même pont sert tiennent donc le même emplacement.
pub type SharedConnection = Arc<Mutex<Option<Arc<BridgeClient>>>>;

/// Un emplacement sans connexion pour l'instant.
#[must_use]
pub fn new_connection_slot() -> SharedConnection {
    Arc::new(Mutex::new(None))
}

impl TcpModule {
    /// `state_timeout` borne l'attente d'une réponse `state`. Une seconde tentative de connexion
    /// attend [`RECONNECT_DELAY`].
    #[must_use]
    pub fn new(connector: BridgeConnector, state_timeout: Duration) -> Self {
        TcpModule {
            connector,
            state_timeout,
            reconnect_delay: RECONNECT_DELAY,
            family: None,
            slot: new_connection_slot(),
        }
    }

    /// Le même module pour une famille d'un pont qui en sert plusieurs : chaque requête `execute` et
    /// `state` la nomme.
    #[must_use]
    pub fn with_family(mut self, family: &'static str) -> Self {
        self.family = Some(family);
        self
    }

    /// Le même module sur une connexion que d'autres modules emploient aussi, pour que les cibles
    /// servies par un pont ouvrent une seule connexion et en partagent l'état d'occupation.
    #[must_use]
    pub fn with_shared_connection(mut self, slot: SharedConnection) -> Self {
        self.slot = slot;
        self
    }

    fn request_payload(&self, mut payload: Value) -> Value {
        if let (Some(family), Value::Object(entries)) = (self.family, &mut payload) {
            entries.insert("family".to_owned(), Value::String(family.to_owned()));
        }
        payload
    }

    /// Le même module avec une autre attente avant une seconde tentative de connexion.
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

    /// Envoie une requête sans danger à répéter ; se reconnecte une fois si la connexion a été perdue.
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
            // Occupé signifie qu'un script tourne : le pont est là.
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
        let payload = self.request_payload(json!({
            "type": "execute",
            "script": script,
            "timeout_s": timeout_s,
            "work_dir": work_dir.to_string_lossy(),
        }));
        let timeout = Duration::from_secs(u64::from(timeout_s));
        let mut attempt = 0;
        let (answer, client) = loop {
            attempt += 1;
            if attempt > 1 {
                // Un pont libère sa place unique un instant après la fermeture de la connexion qui la tenait :
                // une tentative immédiate serait refusée.
                std::thread::sleep(self.reconnect_delay);
            }
            let client = self.client().map_err(module_error)?;
            match client.request(payload.clone(), timeout) {
                Ok(answer) => break (answer, client),
                // Rien n'a été écrit : essayer sur une connexion neuve n'est pas un rejeu.
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

    fn state(&self, capture_path: &Path, _: &Path) -> Result<StateReply, ModuleError> {
        let payload = self.request_payload(json!({
            "type": "state",
            "capture_path": capture_path.to_string_lossy(),
        }));
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
