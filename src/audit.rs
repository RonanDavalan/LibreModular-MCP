//! Journal d'audit JSON Lines, en ajout seul.
//!
//! Pourquoi ce module existe
//! Après coup, l'opérateur doit pouvoir dire ce que le modèle a demandé au Hub
//! d'exécuter, sans que le journal devienne une seconde copie des documents, des
//! scripts ou du jeton. Quand le journal ne peut pas être écrit, exécuter un script
//! sans trace est pire que refuser : l'échec est donc signalé à l'appelant.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::config::Limits;
use crate::output::{OutputError, finalize, to_text};
use crate::security::{
    Category, ControlAccess, SecurityError, Token, ensure_private_dir, open_control_file,
};

/// Les clés fixes d'une ligne du journal, dans l'ordre où elles sont écrites.
pub const ENTRY_KEYS: [&str; 9] = [
    "ts",
    "tool",
    "target_app",
    "status",
    "duration_ms",
    "script_bytes",
    "script_sha256",
    "guard",
    "output_bytes",
];
pub const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const DEFAULT_GENERATIONS: usize = 5;

/// Le journal ne peut pas être écrit ; l'appelant doit refuser d'exécuter.
#[derive(Debug)]
pub struct AuditUnavailable {
    reason: String,
}

impl AuditUnavailable {
    fn new(cause: &dyn fmt::Display) -> Self {
        AuditUnavailable {
            reason: cause.to_string(),
        }
    }
}

impl fmt::Display for AuditUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "audit journal unavailable: {}", self.reason)
    }
}

impl std::error::Error for AuditUnavailable {}

/// Ce que le garde-fou a décidé, tel que le journal l'enregistre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardOutcome {
    /// Rien trouvé : écrit comme une liste vide.
    Passed,
    /// Refusé, avec les catégories trouvées.
    Refused(Vec<Category>),
    /// L'opérateur a levé le garde-fou : écrit `"bypassed"`.
    Bypassed,
}

impl GuardOutcome {
    fn to_value(&self) -> Value {
        match self {
            GuardOutcome::Passed => Value::Array(Vec::new()),
            GuardOutcome::Refused(categories) => Value::Array(
                categories
                    .iter()
                    .map(|category| Value::String(category.id().to_owned()))
                    .collect(),
            ),
            GuardOutcome::Bypassed => Value::String("bypassed".to_owned()),
        }
    }
}

/// Un appel d'outil. Seuls ces champs peuvent atteindre le journal : il n'y a pas de
/// place pour qu'une sortie, un document ou le texte d'un script s'y glisse.
#[derive(Debug, Clone, Default)]
pub struct AuditEntry {
    /// Laissé vide, le journal date l'entrée avec son horloge.
    pub ts: Option<String>,
    pub tool: Option<String>,
    pub target_app: Option<String>,
    pub status: Option<String>,
    pub duration_ms: Option<u64>,
    pub script_bytes: Option<u64>,
    pub script_sha256: Option<String>,
    pub guard: Option<GuardOutcome>,
    pub output_bytes: Option<u64>,
}

/// Taille en octets et SHA-256 d'un script, pour le journal.
#[must_use]
pub fn script_fingerprint(script: &str) -> (usize, String) {
    let digest = Sha256::digest(script.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest.as_slice() {
        hex.push_str(&format!("{byte:02x}"));
    }
    (script.len(), hex)
}

/// Jours depuis le 01/01/1970 vers une date du calendrier (grégorien proleptique).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * month_index + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    })
    .unwrap_or(1);
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// RFC 3339 en UTC, à la milliseconde.
#[must_use]
pub fn format_timestamp(moment: SystemTime) -> String {
    let (seconds, millis) = match moment.duration_since(UNIX_EPOCH) {
        Ok(after) => (
            i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
            after.subsec_millis(),
        ),
        Err(before) => {
            let before: Duration = before.duration();
            (-i64::try_from(before.as_secs()).unwrap_or(i64::MAX), 0)
        }
    };
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let in_day = seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        in_day / 3600,
        in_day % 3600 / 60,
        in_day % 60
    )
}

type Clock = Box<dyn Fn() -> SystemTime + Send + Sync>;

/// Le journal d'audit : un objet JSON par ligne, privé, en ajout seul, avec rotation.
pub struct AuditLog {
    path: PathBuf,
    token: Token,
    limits: Limits,
    max_bytes: u64,
    generations: usize,
    audit_script_text: bool,
    clock: Clock,
}

impl AuditLog {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, token: Token, limits: Limits) -> Self {
        AuditLog {
            path: path.into(),
            token,
            limits,
            max_bytes: DEFAULT_MAX_BYTES,
            generations: DEFAULT_GENERATIONS,
            audit_script_text: false,
            clock: Box::new(SystemTime::now),
        }
    }

    /// Fait la rotation dès que le journal dépasse `max_bytes`, en gardant `generations` anciens fichiers.
    #[must_use]
    pub fn with_rotation(mut self, max_bytes: u64, generations: usize) -> Self {
        self.max_bytes = max_bytes;
        self.generations = generations.max(1);
        self
    }

    /// Écrit le texte de chaque script dans le journal. Désactivé sauf demande de l'opérateur.
    #[must_use]
    pub fn with_script_text(mut self, enabled: bool) -> Self {
        self.audit_script_text = enabled;
        self
    }

    #[must_use]
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn line(&self, entry: &AuditEntry, script_text: Option<&str>) -> Result<String, OutputError> {
        let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
        let number = |value: Option<u64>| value.map_or(Value::Null, Value::from);
        let timestamp = entry
            .ts
            .clone()
            .unwrap_or_else(|| format_timestamp((self.clock)()));
        let mut fields = Map::new();
        fields.insert("ts".to_owned(), Value::String(timestamp));
        fields.insert("tool".to_owned(), text(&entry.tool));
        fields.insert("target_app".to_owned(), text(&entry.target_app));
        fields.insert("status".to_owned(), text(&entry.status));
        fields.insert("duration_ms".to_owned(), number(entry.duration_ms));
        fields.insert("script_bytes".to_owned(), number(entry.script_bytes));
        fields.insert("script_sha256".to_owned(), text(&entry.script_sha256));
        fields.insert(
            "guard".to_owned(),
            entry
                .guard
                .as_ref()
                .map_or(Value::Null, GuardOutcome::to_value),
        );
        fields.insert("output_bytes".to_owned(), number(entry.output_bytes));
        if self.audit_script_text
            && let Some(script) = script_text
        {
            fields.insert("script_text".to_owned(), Value::String(script.to_owned()));
        }
        let safe = finalize(&Value::Object(fields), &self.token, &self.limits)?;
        let Value::Object(safe) = safe else {
            return Err(OutputError::NotAnObject);
        };
        // Les clés fixes viennent d'abord, dans un ordre fixe, pour que le journal se lise toujours de la même façon.
        let mut parts = Vec::new();
        let mut ordered: Vec<&str> = ENTRY_KEYS.to_vec();
        ordered.extend(["script_text", "truncated"]);
        for key in ordered {
            if let Some(value) = safe.get(key) {
                parts.push(format!(
                    "{}:{}",
                    to_text(&Value::String(key.to_owned()))?,
                    to_text(value)?
                ));
            }
        }
        Ok(format!("{{{}}}\n", parts.join(",")))
    }

    /// Vérifie qu'une ligne pourrait être ajoutée maintenant, sans en écrire une.
    ///
    /// Un script ne doit pas atteindre une application quand son appel n'a pas pu être
    /// enregistré : cette vérification passe donc avant tout envoi.
    ///
    /// # Erreurs
    /// [`AuditUnavailable`] quand le répertoire ou le journal ne peut pas être utilisé en sécurité.
    pub fn probe(&self) -> Result<(), AuditUnavailable> {
        let parent = self.path.parent().ok_or_else(|| {
            AuditUnavailable::new(&SecurityError::InvalidPath {
                path: self.path.clone(),
            })
        })?;
        ensure_private_dir(parent).map_err(|error| AuditUnavailable::new(&error))?;
        open_control_file(&self.path, ControlAccess::AppendOrCreate)
            .map(drop)
            .map_err(|error| AuditUnavailable::new(&error))
    }

    /// Ajoute une ligne. `script_text` n'est écrit que si la configuration l'active.
    ///
    /// # Erreurs
    /// [`AuditUnavailable`] chaque fois que la ligne ne peut pas être écrite en sécurité.
    pub fn record(
        &self,
        entry: &AuditEntry,
        script_text: Option<&str>,
    ) -> Result<(), AuditUnavailable> {
        let line = self
            .line(entry, script_text)
            .map_err(|error| AuditUnavailable::new(&error))?;
        let parent = self.path.parent().ok_or_else(|| {
            AuditUnavailable::new(&SecurityError::InvalidPath {
                path: self.path.clone(),
            })
        })?;
        ensure_private_dir(parent).map_err(|error| AuditUnavailable::new(&error))?;
        let mut file = open_control_file(&self.path, ControlAccess::AppendOrCreate)
            .map_err(|error| AuditUnavailable::new(&error))?;
        file.write_all(line.as_bytes())
            .map_err(|error| AuditUnavailable::new(&error))?;
        let size = file
            .metadata()
            .map_err(|error| AuditUnavailable::new(&error))?
            .len();
        drop(file);
        if size > self.max_bytes {
            self.rotate()
                .map_err(|error| AuditUnavailable::new(&error))?;
        }
        Ok(())
    }

    fn generation(&self, index: usize) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{index}"));
        PathBuf::from(name)
    }

    fn rotate(&self) -> std::io::Result<()> {
        let exists = |path: &Path| fs::symlink_metadata(path).is_ok();
        let oldest = self.generation(self.generations);
        if exists(&oldest) {
            fs::remove_file(&oldest)?;
        }
        for index in (1..self.generations).rev() {
            let source = self.generation(index);
            if exists(&source) {
                fs::rename(&source, self.generation(index + 1))?;
            }
        }
        fs::rename(&self.path, self.generation(1))
    }
}

impl fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuditLog")
            .field("path", &self.path)
            .field("max_bytes", &self.max_bytes)
            .field("generations", &self.generations)
            .finish_non_exhaustive()
    }
}
