//! Points d'entrée en ligne de commande : `libremodular-mcp` et `libremodular-launch`.
//!
//! Pourquoi ce module existe
//! L'opérateur a besoin d'un moyen explicite et sûr de créer la configuration et le jeton
//! du pont avec les bonnes permissions, d'un serveur qui refuse de démarrer quand une
//! condition de sécurité ne peut pas être vérifiée, et d'un lanceur dont la liste
//! d'arguments est fixée par le code et jamais construite à partir d'un texte écrit par
//! le modèle. C'est aussi le seul module qui écrit sur la sortie standard, hors le
//! protocole MCP lui-même.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, SystemTime};

use rmcp::ServiceExt;
use tokio::signal::unix::{SignalKind, signal};

use crate::audit::{AuditLog, AuditUnavailable};
use crate::config::{self, APP_DEFAULTS, ConfigError, Limits, Paths};
use crate::registry::Registry;
use crate::security::{
    ControlAccess, SecurityError, create_token, ensure_private_dir, harden_process, load_token,
    open_control_file,
};
use crate::server::{HubContext, HubServer, build_server};

#[derive(Parser)]
#[command(
    name = "libremodular-mcp",
    version,
    about = "LibreModular-MCP: let a language model drive desktop applications."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Sert MCP sur l'entrée et la sortie standard (c'est aussi la valeur par défaut)
    Serve,
    /// Crée le répertoire de configuration, une configuration d'exemple et le jeton
    Init {
        /// Remplace le jeton existant
        #[arg(long)]
        rotate: bool,
    },
}

/// Configuration d'exemple commentée ; chaque application démarre désactivée.
#[must_use]
pub fn example_config_text() -> String {
    let limits = Limits::default();
    let mut lines = vec![
        "# LibreModular-MCP configuration. Absent keys keep the strict defaults.".to_owned(),
        String::new(),
        "[hub]".to_owned(),
        format!("# max_script_bytes = {}", limits.max_script_bytes),
        format!("# max_output_bytes = {}", limits.max_output_bytes),
        format!("# max_line_bytes = {}", limits.max_line_bytes),
        format!("# default_timeout_s = {}", limits.default_timeout_s),
        format!("# max_timeout_s = {}", limits.max_timeout_s),
        String::new(),
        "[security]".to_owned(),
        "# Lifts the script guard. The audit journal then records: guard: bypassed.".to_owned(),
        "# allow_dangerous_scripts = false".to_owned(),
        "# Write the script text into the audit journal.".to_owned(),
        "# audit_script_text = false".to_owned(),
        "# Absolute directories scripts may write to, besides their own work directory.".to_owned(),
        "# extra_write_roots = []".to_owned(),
        String::new(),
    ];
    for defaults in APP_DEFAULTS {
        lines.push(format!("[apps.{}]", defaults.name));
        lines.push("enabled = false".to_owned());
        lines.push(format!("mode = \"{}\"", defaults.mode.as_str()));
        if let Some(port) = defaults.port {
            lines.push(format!("port = {port}"));
        }
        if let Some(port) = defaults.uno_port {
            lines.push(format!("uno_port = {port}"));
        }
        if let Some(port) = defaults.bridge_port {
            lines.push(format!("bridge_port = {port}"));
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Ce qui peut mal tourner dans `init`.
#[derive(Debug)]
enum InitError {
    Config(ConfigError),
    Security(SecurityError),
    Output(std::io::Error),
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::Config(error) => error.fmt(f),
            InitError::Security(error) => error.fmt(f),
            InitError::Output(error) => write!(f, "cannot write the report: {error}"),
        }
    }
}

impl From<ConfigError> for InitError {
    fn from(error: ConfigError) -> Self {
        InitError::Config(error)
    }
}

impl From<SecurityError> for InitError {
    fn from(error: SecurityError) -> Self {
        InitError::Security(error)
    }
}

impl From<std::io::Error> for InitError {
    fn from(error: std::io::Error) -> Self {
        InitError::Output(error)
    }
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn init_steps(paths: &Paths, rotate: bool, out: &mut dyn Write) -> Result<(), InitError> {
    harden_process()?;
    ensure_private_dir(&paths.config_dir)?;
    ensure_private_dir(&paths.state_dir)?;
    writeln!(out, "directory ready: {}", paths.config_dir.display())?;
    writeln!(out, "directory ready: {}", paths.state_dir.display())?;

    match open_control_file(&paths.config_file(), ControlAccess::CreateNew) {
        Ok(mut file) => {
            file.write_all(example_config_text().as_bytes())
                .map_err(|e| {
                    InitError::Security(SecurityError::Io {
                        action: "write",
                        path: paths.config_file(),
                        source: e,
                    })
                })?;
            writeln!(
                out,
                "configuration created: {}",
                paths.config_file().display()
            )?;
        }
        Err(SecurityError::AlreadyExists { .. }) => {
            writeln!(
                out,
                "configuration exists, unchanged: {}",
                paths.config_file().display()
            )?;
        }
        Err(error) => return Err(error.into()),
    }

    let token_file = paths.token_file();
    if exists(&token_file) {
        if rotate {
            create_token(&token_file, true)?;
            writeln!(out, "token rotated: {}", token_file.display())?;
            writeln!(
                out,
                "restart every bridge: they still hold the previous token"
            )?;
        } else {
            load_token(&token_file)?;
            writeln!(out, "token exists, unchanged: {}", token_file.display())?;
        }
    } else {
        create_token(&token_file, false)?;
        writeln!(out, "token created: {}", token_file.display())?;
    }
    Ok(())
}

/// Crée les répertoires, la configuration d'exemple et le jeton. N'affiche jamais le
/// jeton. Rend le code de sortie du processus.
pub fn run_init(paths: &Paths, rotate: bool, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    match init_steps(paths, rotate, out) {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "error: {error}");
            1
        }
    }
}

/// Exécute la ligne de commande de `libremodular-mcp` et rend son code de sortie.
///
/// `locate` fournit les répertoires de base ; le binaire passe les vrais, et les tests
/// des répertoires temporaires pour ne jamais toucher à la configuration de l'utilisateur.
pub fn run_with(
    args: impl IntoIterator<Item = OsString>,
    out: &mut dyn Write,
    err: &mut dyn Write,
    locate: &dyn Fn() -> Result<Paths, ConfigError>,
) -> u8 {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let rendered = error.render().to_string();
            if error.use_stderr() {
                let _ = write!(err, "{rendered}");
                return 2;
            }
            let _ = write!(out, "{rendered}");
            return 0;
        }
    };
    match cli.command {
        Some(Command::Init { rotate }) => match locate() {
            Ok(paths) => run_init(&paths, rotate, out, err),
            Err(error) => {
                let _ = writeln!(err, "error: {error}");
                1
            }
        },
        Some(Command::Serve) | None => match locate() {
            Ok(paths) => run_serve(&paths, err),
            Err(error) => {
                let _ = writeln!(err, "error: {error}");
                1
            }
        },
    }
}

/// Texte d'aide de la commande, tel que `--help` l'affiche.
#[must_use]
pub fn help_text() -> String {
    Cli::command().render_help().to_string()
}

/// Ce qui empêche le serveur de démarrer.
#[derive(Debug)]
pub enum ServeError {
    Config(ConfigError),
    Security(SecurityError),
    Audit(AuditUnavailable),
    Runtime(std::io::Error),
    Protocol(String),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::Config(error) => error.fmt(f),
            ServeError::Security(error) => error.fmt(f),
            ServeError::Audit(error) => error.fmt(f),
            ServeError::Runtime(error) => write!(f, "cannot start the runtime: {error}"),
            ServeError::Protocol(message) => write!(f, "MCP session failed: {message}"),
        }
    }
}

impl std::error::Error for ServeError {}

impl From<ConfigError> for ServeError {
    fn from(error: ConfigError) -> Self {
        ServeError::Config(error)
    }
}

impl From<SecurityError> for ServeError {
    fn from(error: SecurityError) -> Self {
        ServeError::Security(error)
    }
}

impl From<AuditUnavailable> for ServeError {
    fn from(error: AuditUnavailable) -> Self {
        ServeError::Audit(error)
    }
}

/// Construit le serveur dans l'ordre qui place chaque contrôle avant ce qu'il protège :
/// configuration, durcissement, jeton, journal, répertoire volatil, registre.
///
/// # Erreurs
/// Toute condition de sécurité qui ne peut pas être vérifiée.
pub fn prepare_server(paths: &Paths) -> Result<HubServer, ServeError> {
    let config = config::load(&paths.config_file())?;
    harden_process()?;
    let token = load_token(&paths.token_file())?;
    let audit = AuditLog::new(paths.audit_file(), token.clone(), *config.limits())
        .with_script_text(config.security().audit_script_text);
    audit.probe()?;
    let volatile_dir = paths.require_runtime_dir()?.to_path_buf();
    ensure_private_dir(&volatile_dir)?;
    ensure_private_dir(&volatile_dir.join("work"))?;
    ensure_private_dir(&volatile_dir.join("captures"))?;
    let registry = Registry::from_config(&config, &token);
    let server = build_server(HubContext {
        config,
        registry,
        audit,
        token,
        clock: Arc::new(SystemTime::now),
        volatile_dir,
        home_dir: config::home_dir_from_process(),
    });
    server.context().purge();
    Ok(server)
}

/// Comment la session MCP s'est terminée.
enum Ending {
    /// Le client a fermé la session, ou elle a échoué.
    Session(Result<(), ServeError>),
    /// Le processus a reçu l'ordre de s'arrêter ; le nom du signal.
    Signal(&'static str),
}

/// Sert MCP sur l'entrée et la sortie standard jusqu'à ce que le client la ferme ou que le
/// processus reçoive SIGTERM ou SIGINT. Dans les deux cas les captures sont purgées avant
/// la fin du processus.
fn serve_stdio(server: HubServer) -> Result<(), ServeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ServeError::Runtime)?;
    let session = server.clone();
    let ending = runtime.block_on(async move {
        // On écoute avant le début de la session : une demande d'arrêt à n'importe quel moment
        // de la session emprunte la même sortie.
        let mut terminate = signal(SignalKind::terminate()).map_err(ServeError::Runtime)?;
        let mut interrupt = signal(SignalKind::interrupt()).map_err(ServeError::Runtime)?;
        let mut session = pin!(async move {
            let running = session
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|error| ServeError::Protocol(error.to_string()))?;
            running
                .waiting()
                .await
                .map(drop)
                .map_err(|error| ServeError::Protocol(error.to_string()))
        });
        Ok::<Ending, ServeError>(
            poll_fn(|context| {
                if terminate.poll_recv(context).is_ready() {
                    return Poll::Ready(Ending::Signal("SIGTERM"));
                }
                if interrupt.poll_recv(context).is_ready() {
                    return Poll::Ready(Ending::Signal("SIGINT"));
                }
                session.as_mut().poll(context).map(Ending::Session)
            })
            .await,
        )
    });
    server.shutdown();
    // Une requête qui attend encore un pont ne doit pas garder le processus en vie.
    runtime.shutdown_timeout(Duration::from_secs(1));
    match ending? {
        Ending::Session(outcome) => outcome,
        Ending::Signal(name) => {
            eprintln!("libremodular-mcp: stopped on {name}");
            Ok(())
        }
    }
}

/// Démarre le serveur et rend le code de sortie. Un refus de démarrer est signalé sur le
/// flux d'erreur, jamais avec le jeton.
pub fn run_serve(paths: &Paths, err: &mut dyn Write) -> u8 {
    match prepare_server(paths).and_then(serve_stdio) {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "error: {error}");
            1
        }
    }
}

/// Point d'entrée du binaire `libremodular-mcp`.
#[must_use]
pub fn main_entry() -> ExitCode {
    ExitCode::from(run_with(
        std::env::args_os(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        &config::resolve_paths_from_process,
    ))
}

/// Point d'entrée du binaire `libremodular-launch`.
#[must_use]
pub fn launch_entry() -> ExitCode {
    ExitCode::from(crate::launch::run_in_process(
        std::env::args_os(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    ))
}
