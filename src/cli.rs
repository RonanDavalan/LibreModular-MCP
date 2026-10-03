//! Command line entry points: `libremodular-mcp` and `libremodular-launch`.
//!
//! Why this module exists
//! The operator needs one explicit, safe way to create the configuration and the
//! bridge token with the right permissions, and a launcher whose argument list is
//! fixed by code and never built from text the model wrote. This is also the only
//! module that writes to the standard output.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

use crate::config::{self, APP_DEFAULTS, ConfigError, Limits, Paths};
use crate::security::{
    ControlAccess, SecurityError, create_token, ensure_private_dir, harden_process, load_token,
    open_control_file,
};

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
    /// Create the configuration directory, an example configuration and the token
    Init {
        /// Replace the existing token
        #[arg(long)]
        rotate: bool,
    },
}

/// Commented example configuration; every application starts disabled.
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

/// What can go wrong in `init`.
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

/// Create directories, the example configuration and the token. Never prints the
/// token. Returns the process exit code.
pub fn run_init(paths: &Paths, rotate: bool, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    match init_steps(paths, rotate, out) {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "error: {error}");
            1
        }
    }
}

/// Run the `libremodular-mcp` command line and return its exit code.
///
/// `locate` provides the base directories; the binary passes the real ones, and
/// tests pass temporary ones so that they never touch the user's configuration.
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
        None => {
            let help = Cli::command().render_help().to_string();
            let _ = write!(out, "{help}");
            0
        }
    }
}

/// Entry point of the `libremodular-mcp` binary.
#[must_use]
pub fn main_entry() -> ExitCode {
    ExitCode::from(run_with(
        std::env::args_os(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        &config::resolve_paths_from_process,
    ))
}

/// The launcher supports no application yet.
pub fn launch_run(err: &mut dyn Write) -> u8 {
    let _ = writeln!(err, "libremodular-launch: no application is supported yet");
    2
}

/// Entry point of the `libremodular-launch` binary.
#[must_use]
pub fn launch_entry() -> ExitCode {
    ExitCode::from(launch_run(&mut std::io::stderr()))
}
