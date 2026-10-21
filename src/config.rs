//! Chargement de la configuration et résolution des chemins du Hub.
//!
//! Pourquoi ce module existe
//! Chaque limite et chaque interrupteur qui change la sévérité du Hub doit venir d'un seul
//! endroit, validé une fois, pour qu'une faute de frappe dans un fichier ne puisse jamais
//! affaiblir une protection en silence. Rien d'autre dans la crate ne lit la configuration
//! ni l'environnement.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::security::{self, SecurityError};

/// Nom du répertoire que ce programme utilise sous chaque répertoire de base.
pub const PACKAGE_DIR_NAME: &str = "libremodular-mcp";
/// Un fichier de configuration plus grand que cela est refusé.
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

const PORT_MIN: i64 = 1024;
const PORT_MAX: i64 = 65535;
const MIN_BYTE_LIMIT: i64 = 1024;
const MIN_SECONDS: i64 = 1;

/// Comment le Hub joint une application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tcp,
    Uno,
    Cli,
}

impl Mode {
    /// La valeur écrite dans le fichier de configuration.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Tcp => "tcp",
            Mode::Uno => "uno",
            Mode::Cli => "cli",
        }
    }

    fn parse(text: &str) -> Option<Mode> {
        match text {
            "tcp" => Some(Mode::Tcp),
            "uno" => Some(Mode::Uno),
            "cli" => Some(Mode::Cli),
            _ => None,
        }
    }
}

/// Les réglages intégrés d'une application connue.
#[derive(Debug, Clone, Copy)]
pub struct AppDefaults {
    pub name: &'static str,
    pub mode: Mode,
    pub port: Option<u16>,
    pub uno_port: Option<u16>,
    pub bridge_port: Option<u16>,
}

const fn app(
    name: &'static str,
    mode: Mode,
    port: Option<u16>,
    uno_port: Option<u16>,
    bridge_port: Option<u16>,
) -> AppDefaults {
    AppDefaults {
        name,
        mode,
        port,
        uno_port,
        bridge_port,
    }
}

/// Toutes les applications que le Hub connaît, avec leur branchement par défaut. Aucune
/// n'est activée tant que l'opérateur ne le dit pas.
pub const APP_DEFAULTS: [AppDefaults; 7] = [
    app("gimp", Mode::Tcp, Some(9301), None, None),
    app("krita", Mode::Tcp, Some(9302), None, None),
    app("blender", Mode::Tcp, Some(9303), None, None),
    app("inkscape", Mode::Cli, None, None, None),
    app(
        "libreoffice-writer",
        Mode::Uno,
        None,
        Some(2002),
        Some(9304),
    ),
    app("libreoffice-calc", Mode::Uno, None, Some(2002), Some(9304)),
    app(
        "libreoffice-impress",
        Mode::Uno,
        None,
        Some(2002),
        Some(9304),
    ),
];

fn known_applications() -> String {
    APP_DEFAULTS
        .iter()
        .map(|defaults| defaults.name)
        .collect::<Vec<_>>()
        .join(", ")
}

// --------------------------------------------------------------------------
// Erreurs
// --------------------------------------------------------------------------

/// Quelle sorte de faute le fichier contient. Jamais le texte fautif lui-même : une
/// configuration peut contenir des chemins ou des valeurs que l'opérateur ne veut pas
/// voir répétés.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    UnknownKey,
    WrongType,
    InvalidValue,
    Malformed,
}

/// La configuration, ou l'environnement dont elle dépend, est invalide.
#[derive(Debug)]
pub enum ConfigError {
    /// Les répertoires de base ne peuvent pas être déterminés ; le texte nomme la variable.
    Environment(String),
    /// Le fichier ne peut pas être cru ni lu.
    Unsafe(SecurityError),
    NotUtf8,
    /// Le fichier n'est pas ce que le schéma permet, à cette position quand elle est connue.
    Rejected {
        kind: Rejection,
        position: Option<(usize, usize)>,
    },
    /// Une valeur dépasse une borne ; nomme la clé et la borne, jamais la valeur.
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Environment(message) | ConfigError::Invalid(message) => {
                f.write_str(message)
            }
            ConfigError::Unsafe(error) => write!(f, "configuration file refused: {error}"),
            ConfigError::NotUtf8 => f.write_str("the configuration file is not valid UTF-8"),
            ConfigError::Rejected { kind, position } => {
                let what = match kind {
                    Rejection::UnknownKey => "has an unknown key or section",
                    Rejection::WrongType => "has a value of the wrong type",
                    Rejection::InvalidValue => "has an invalid value",
                    Rejection::Malformed => "is not valid TOML or does not match the schema",
                };
                match position {
                    Some((line, column)) => write!(
                        f,
                        "the configuration file {what} (line {line}, column {column})"
                    ),
                    None => write!(f, "the configuration file {what}"),
                }
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Unsafe(error) => Some(error),
            _ => None,
        }
    }
}

fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(message.into())
}

// --------------------------------------------------------------------------
// Réglages
// --------------------------------------------------------------------------

/// Les bornes de taille et de temps qui empêchent une requête d'épuiser le Hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_script_bytes: usize,
    pub max_output_bytes: usize,
    pub max_line_bytes: usize,
    pub default_timeout_s: u32,
    pub max_timeout_s: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_script_bytes: 262_144,
            max_output_bytes: 1_048_576,
            max_line_bytes: 8_388_608,
            default_timeout_s: 30,
            max_timeout_s: 300,
        }
    }
}

/// Les interrupteurs qui rendent le Hub moins strict. Tous désactivés par défaut.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SecuritySettings {
    pub allow_dangerous_scripts: bool,
    pub audit_script_text: bool,
    pub extra_write_roots: Vec<String>,
}

/// Les réglages d'une application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSettings {
    pub name: &'static str,
    pub enabled: bool,
    pub mode: Mode,
    pub port: Option<u16>,
    pub uno_port: Option<u16>,
    pub bridge_port: Option<u16>,
}

/// Une configuration validée. Elle ne peut plus être modifiée une fois construite.
///
/// ```compile_fail
/// let config = libremodular_mcp::config::Config::strict_defaults();
/// config.limits().max_script_bytes = 1;
/// ```
///
/// ```
/// let config = libremodular_mcp::config::Config::strict_defaults();
/// assert_eq!(config.limits().max_script_bytes, 262_144);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    limits: Limits,
    security: SecuritySettings,
    apps: Vec<AppSettings>,
}

impl Config {
    /// La configuration d'un fichier absent : les valeurs les plus strictes, rien d'activé.
    #[must_use]
    pub fn strict_defaults() -> Config {
        let apps = APP_DEFAULTS
            .iter()
            .map(|defaults| AppSettings {
                name: defaults.name,
                enabled: false,
                mode: defaults.mode,
                port: defaults.port,
                uno_port: defaults.uno_port,
                bridge_port: defaults.bridge_port,
            })
            .collect();
        Config {
            limits: Limits::default(),
            security: SecuritySettings::default(),
            apps,
        }
    }

    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    #[must_use]
    pub fn security(&self) -> &SecuritySettings {
        &self.security
    }

    #[must_use]
    pub fn apps(&self) -> &[AppSettings] {
        &self.apps
    }

    #[must_use]
    pub fn app(&self, name: &str) -> Option<&AppSettings> {
        self.apps.iter().find(|app| app.name == name)
    }

    /// Les noms des applications activées, dans l'ordre du registre.
    #[must_use]
    pub fn active_names(&self) -> Vec<&'static str> {
        self.apps
            .iter()
            .filter(|app| app.enabled)
            .map(|app| app.name)
            .collect()
    }

    /// Valide le contenu d'un fichier de configuration déjà lu par le module de sécurité.
    ///
    /// # Erreurs
    /// Chaque violation est une erreur qui nomme la clé ou la position, jamais une valeur.
    pub fn from_bytes(bytes: &[u8]) -> Result<Config, ConfigError> {
        let text = std::str::from_utf8(bytes).map_err(|_| ConfigError::NotUtf8)?;
        let raw: RawConfig = toml::from_str(text).map_err(|error| reject(text, &error))?;
        raw.validate()
    }
}

/// Charge le fichier de configuration. Un fichier absent donne les valeurs strictes par défaut ;
/// un fichier qui ne peut être ni cru ni lu est une erreur, jamais un repli.
///
/// # Erreurs
/// Une configuration peu sûre (liens, permissions, taille), illisible ou invalide.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    match security::read_control_file(path, MAX_CONFIG_BYTES) {
        Ok(bytes) => Config::from_bytes(&bytes),
        Err(error) if error.is_not_found() => Ok(Config::strict_defaults()),
        Err(error) => Err(ConfigError::Unsafe(error)),
    }
}

// --------------------------------------------------------------------------
// Analyse
// --------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    hub: Option<RawHub>,
    security: Option<RawSecurity>,
    apps: Option<BTreeMap<String, RawApp>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHub {
    max_script_bytes: Option<i64>,
    max_output_bytes: Option<i64>,
    max_line_bytes: Option<i64>,
    default_timeout_s: Option<i64>,
    max_timeout_s: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSecurity {
    allow_dangerous_scripts: Option<bool>,
    audit_script_text: Option<bool>,
    extra_write_roots: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawApp {
    enabled: Option<bool>,
    mode: Option<String>,
    port: Option<i64>,
    uno_port: Option<i64>,
    bridge_port: Option<i64>,
}

fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |last| last.chars().count())
        + 1;
    (line, column)
}

/// Transforme l'erreur de l'analyseur en une erreur qui porte une position et une sorte, et
/// rien du fichier : son propre message cite le texte autour de la faute.
fn reject(text: &str, error: &toml::de::Error) -> ConfigError {
    let message = error.message();
    let kind = if message.starts_with("unknown field") {
        Rejection::UnknownKey
    } else if message.starts_with("invalid type") {
        Rejection::WrongType
    } else if message.starts_with("invalid value") {
        Rejection::InvalidValue
    } else {
        Rejection::Malformed
    };
    ConfigError::Rejected {
        kind,
        position: error.span().map(|span| position(text, span.start)),
    }
}

fn bounded_integer(
    value: Option<i64>,
    default: i64,
    minimum: i64,
    maximum: Option<i64>,
    key: &str,
) -> Result<i64, ConfigError> {
    let value = value.unwrap_or(default);
    if value < minimum || maximum.is_some_and(|max| value > max) {
        let bound = match maximum {
            Some(max) => format!("between {minimum} and {max}"),
            None => format!("at least {minimum}"),
        };
        return Err(invalid(format!("{key} must be {bound}")));
    }
    Ok(value)
}

fn size(value: Option<i64>, default: usize, key: &str) -> Result<usize, ConfigError> {
    let default = i64::try_from(default).unwrap_or(i64::MAX);
    let checked = bounded_integer(value, default, MIN_BYTE_LIMIT, None, key)?;
    usize::try_from(checked).map_err(|_| invalid(format!("{key} is too large")))
}

fn seconds(value: Option<i64>, default: u32, key: &str) -> Result<u32, ConfigError> {
    let checked = bounded_integer(value, i64::from(default), MIN_SECONDS, None, key)?;
    u32::try_from(checked).map_err(|_| invalid(format!("{key} is too large")))
}

fn port(value: Option<i64>, default: Option<u16>, key: &str) -> Result<Option<u16>, ConfigError> {
    match value {
        None => Ok(default),
        Some(given) => {
            let checked = bounded_integer(Some(given), given, PORT_MIN, Some(PORT_MAX), key)?;
            u16::try_from(checked)
                .map(Some)
                .map_err(|_| invalid(format!("{key} must be between {PORT_MIN} and {PORT_MAX}")))
        }
    }
}

impl RawConfig {
    fn validate(self) -> Result<Config, ConfigError> {
        Ok(Config {
            limits: validate_limits(self.hub)?,
            security: validate_security(self.security)?,
            apps: validate_apps(self.apps.unwrap_or_default())?,
        })
    }
}

fn validate_limits(raw: Option<RawHub>) -> Result<Limits, ConfigError> {
    let Some(raw) = raw else {
        return Ok(Limits::default());
    };
    let base = Limits::default();
    let limits = Limits {
        max_script_bytes: size(
            raw.max_script_bytes,
            base.max_script_bytes,
            "hub.max_script_bytes",
        )?,
        max_output_bytes: size(
            raw.max_output_bytes,
            base.max_output_bytes,
            "hub.max_output_bytes",
        )?,
        max_line_bytes: size(
            raw.max_line_bytes,
            base.max_line_bytes,
            "hub.max_line_bytes",
        )?,
        default_timeout_s: seconds(
            raw.default_timeout_s,
            base.default_timeout_s,
            "hub.default_timeout_s",
        )?,
        max_timeout_s: seconds(raw.max_timeout_s, base.max_timeout_s, "hub.max_timeout_s")?,
    };
    if limits.max_timeout_s < limits.default_timeout_s {
        return Err(invalid(
            "hub.max_timeout_s must not be smaller than hub.default_timeout_s",
        ));
    }
    Ok(limits)
}

fn validate_security(raw: Option<RawSecurity>) -> Result<SecuritySettings, ConfigError> {
    let Some(raw) = raw else {
        return Ok(SecuritySettings::default());
    };
    let roots = raw.extra_write_roots.unwrap_or_default();
    if roots.iter().any(String::is_empty) {
        return Err(invalid(
            "security.extra_write_roots must be a list of non-empty strings",
        ));
    }
    if roots.iter().any(|root| !Path::new(root).is_absolute()) {
        return Err(invalid(
            "security.extra_write_roots entries must be absolute paths",
        ));
    }
    Ok(SecuritySettings {
        allow_dangerous_scripts: raw.allow_dangerous_scripts.unwrap_or(false),
        audit_script_text: raw.audit_script_text.unwrap_or(false),
        extra_write_roots: roots,
    })
}

fn validate_apps(mut raw: BTreeMap<String, RawApp>) -> Result<Vec<AppSettings>, ConfigError> {
    if raw
        .keys()
        .any(|name| !APP_DEFAULTS.iter().any(|known| known.name == name))
    {
        return Err(invalid(format!(
            "unknown application in [apps]; known applications: {}",
            known_applications()
        )));
    }
    let mut apps = Vec::new();
    for defaults in APP_DEFAULTS {
        let name = defaults.name;
        let given = raw.remove(name);
        let (enabled, mode, port_value, uno_value, bridge_value) = match given {
            Some(table) => (
                table.enabled,
                table.mode,
                table.port,
                table.uno_port,
                table.bridge_port,
            ),
            None => (None, None, None, None, None),
        };
        let mode = match mode {
            None => defaults.mode,
            Some(text) => Mode::parse(&text).ok_or_else(|| {
                invalid(format!("apps.{name}.mode must be one of: tcp, uno, cli"))
            })?,
        };
        apps.push(AppSettings {
            name,
            enabled: enabled.unwrap_or(false),
            mode,
            port: port(port_value, defaults.port, &format!("apps.{name}.port"))?,
            uno_port: port(
                uno_value,
                defaults.uno_port,
                &format!("apps.{name}.uno_port"),
            )?,
            bridge_port: port(
                bridge_value,
                defaults.bridge_port,
                &format!("apps.{name}.bridge_port"),
            )?,
        });
    }
    Ok(apps)
}

// --------------------------------------------------------------------------
// Emplacements
// --------------------------------------------------------------------------

/// Les trois répertoires de base du Hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub runtime_dir: Option<PathBuf>,
}

impl Paths {
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    #[must_use]
    pub fn token_file(&self) -> PathBuf {
        self.config_dir.join("token")
    }

    #[must_use]
    pub fn audit_file(&self) -> PathBuf {
        self.state_dir.join("audit.jsonl")
    }

    /// Le répertoire volatil, ou un refus quand le système n'en a donné aucun : les fichiers
    /// volatils ne sont jamais écrits ailleurs.
    ///
    /// # Erreurs
    /// [`ConfigError::Environment`] quand `XDG_RUNTIME_DIR` n'est pas utilisable.
    pub fn require_runtime_dir(&self) -> Result<&Path, ConfigError> {
        self.runtime_dir.as_deref().ok_or_else(|| {
            ConfigError::Environment(if cfg!(windows) {
                "LOCALAPPDATA is not set; refusing to write volatile files elsewhere".to_owned()
            } else {
                "XDG_RUNTIME_DIR is not set; refusing to write volatile files elsewhere".to_owned()
            })
        })
    }
}

fn absolute_override(variable: &str, value: &OsString) -> Result<PathBuf, ConfigError> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(ConfigError::Environment(format!(
            "{variable} must be an absolute path"
        )))
    }
}

/// Résout les trois répertoires de base (règles XDG, avec les surcharges `LIBREMODULAR_*`
/// employées pour les tests et les dispositions inhabituelles). `lookup` lit une variable.
///
/// # Erreurs
/// Une surcharge relative, ou aucun répertoire personnel utilisable quand il en faut un.
#[cfg(unix)]
pub fn resolve_paths(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Paths, ConfigError> {
    let non_empty = |name: &str| lookup(name).filter(|value| !value.is_empty());
    let home = || -> Result<PathBuf, ConfigError> {
        let value = non_empty("HOME").ok_or_else(|| {
            ConfigError::Environment("cannot determine the home directory".to_owned())
        })?;
        let path = PathBuf::from(value);
        if path.is_absolute() {
            Ok(path)
        } else {
            Err(ConfigError::Environment(
                "the home directory must be an absolute path".to_owned(),
            ))
        }
    };
    let pick = |override_name: &str,
                xdg_name: &str,
                fallback: &dyn Fn() -> Result<Option<PathBuf>, ConfigError>|
     -> Result<Option<PathBuf>, ConfigError> {
        if let Some(value) = non_empty(override_name) {
            return absolute_override(override_name, &value).map(Some);
        }
        if let Some(value) = non_empty(xdg_name) {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                return Ok(Some(path));
            }
        }
        fallback()
    };
    let config_base = pick("LIBREMODULAR_CONFIG_DIR", "XDG_CONFIG_HOME", &|| {
        home().map(|h| Some(h.join(".config")))
    })?;
    let state_base = pick("LIBREMODULAR_STATE_DIR", "XDG_STATE_HOME", &|| {
        home().map(|h| Some(h.join(".local").join("state")))
    })?;
    let runtime_base = pick("LIBREMODULAR_RUNTIME_DIR", "XDG_RUNTIME_DIR", &|| Ok(None))?;
    let (Some(config_base), Some(state_base)) = (config_base, state_base) else {
        return Err(ConfigError::Environment(
            "cannot determine the base directories".to_owned(),
        ));
    };
    Ok(Paths {
        config_dir: config_base.join(PACKAGE_DIR_NAME),
        state_dir: state_base.join(PACKAGE_DIR_NAME),
        runtime_dir: runtime_base.map(|base| base.join(PACKAGE_DIR_NAME)),
    })
}

/// Résout les trois répertoires de base sous Windows : `%LOCALAPPDATA%\libremodular-mcp\` et ses
/// sous-répertoires `config`, `state` et `run`. Les surcharges `LIBREMODULAR_*` valent comme ailleurs.
///
/// # Erreurs
/// Une surcharge relative, ou aucun `LOCALAPPDATA` utilisable quand il en faut un.
#[cfg(windows)]
pub fn resolve_paths(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Paths, ConfigError> {
    let non_empty = |name: &str| lookup(name).filter(|value| !value.is_empty());
    let local = || -> Result<PathBuf, ConfigError> {
        let value = non_empty("LOCALAPPDATA").ok_or_else(|| {
            ConfigError::Environment(
                "cannot determine the local application data directory".to_owned(),
            )
        })?;
        let path = PathBuf::from(value);
        if path.is_absolute() {
            Ok(path.join(PACKAGE_DIR_NAME))
        } else {
            Err(ConfigError::Environment(
                "LOCALAPPDATA must be an absolute path".to_owned(),
            ))
        }
    };
    let pick = |override_name: &str, folder: &str| -> Result<PathBuf, ConfigError> {
        match non_empty(override_name) {
            Some(value) => {
                absolute_override(override_name, &value).map(|p| p.join(PACKAGE_DIR_NAME))
            }
            None => local().map(|base| base.join(folder)),
        }
    };
    Ok(Paths {
        config_dir: pick("LIBREMODULAR_CONFIG_DIR", "config")?,
        state_dir: pick("LIBREMODULAR_STATE_DIR", "state")?,
        runtime_dir: pick("LIBREMODULAR_RUNTIME_DIR", "run").ok(),
    })
}

/// [`resolve_paths`] sur l'environnement du processus en cours.
///
/// # Erreurs
/// Voir [`resolve_paths`].
pub fn resolve_paths_from_process() -> Result<Paths, ConfigError> {
    resolve_paths(|name| std::env::var_os(name))
}

/// Le répertoire personnel, quand `HOME` contient un chemin absolu. Le garde-fou de script
/// s'en sert pour développer un `~` initial dans un chemin littéral.
#[must_use]
pub fn home_dir(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    lookup(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// [`home_dir`] sur l'environnement du processus en cours.
#[must_use]
pub fn home_dir_from_process() -> Option<PathBuf> {
    home_dir(|name| std::env::var_os(name))
}

/// Les valeurs des variables nommées dans l'environnement du Hub, pour les processus qu'un
/// module démarre. Les variables absentes sont laissées de côté.
#[must_use]
pub fn environment_values(names: &[&str]) -> Vec<(OsString, OsString)> {
    names
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (OsString::from(name), value)))
        .collect()
}

/// Toutes les variables de l'environnement du processus en cours. Le lanceur décide de ce que
/// chaque programme qu'il démarre reçoit : il travaille donc sur cet instantané plutôt que
/// sur l'environnement lui-même.
#[must_use]
pub fn environment_snapshot() -> Vec<(OsString, OsString)> {
    std::env::vars_os().collect()
}
