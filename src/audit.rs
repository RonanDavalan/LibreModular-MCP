//! Append-only JSON Lines audit journal.
//!
//! Why this module exists
//! After the fact, the operator must be able to tell what the model asked the Hub
//! to run, without the journal itself becoming a second copy of the documents, the
//! scripts or the token. When the journal cannot be written, running a script
//! without a trace is worse than refusing, so failure is reported to the caller.

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

/// The fixed keys of a journal line, in the order they are written.
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

/// The journal cannot be written; the caller must refuse to execute.
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

/// What the guard decided, as the journal records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardOutcome {
    /// Nothing found: written as an empty list.
    Passed,
    /// Refused, with the categories found.
    Refused(Vec<Category>),
    /// The operator lifted the guard: written as `"bypassed"`.
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

/// One tool call. Only these fields can reach the journal: there is no place for
/// an output, a document or a script text to slip in.
#[derive(Debug, Clone, Default)]
pub struct AuditEntry {
    /// Left empty, the journal stamps the entry with its clock.
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

/// Size in bytes and SHA-256 of a script, for the journal.
#[must_use]
pub fn script_fingerprint(script: &str) -> (usize, String) {
    let digest = Sha256::digest(script.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest.as_slice() {
        hex.push_str(&format!("{byte:02x}"));
    }
    (script.len(), hex)
}

/// Days since 1970-01-01 to a calendar date (proleptic Gregorian).
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

/// RFC 3339 in UTC with millisecond precision.
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

/// The audit journal: one JSON object per line, private, append-only, rotated.
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

    /// Rotate once the journal exceeds `max_bytes`, keeping `generations` old files.
    #[must_use]
    pub fn with_rotation(mut self, max_bytes: u64, generations: usize) -> Self {
        self.max_bytes = max_bytes;
        self.generations = generations.max(1);
        self
    }

    /// Write the text of each script into the journal. Off unless the operator asks.
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
        // The fixed keys come first, in a fixed order, so the journal reads the same way each time.
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

    /// Append one line. `script_text` is written only when enabled by configuration.
    ///
    /// # Errors
    /// [`AuditUnavailable`] whenever the line cannot be written safely.
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
