//! The launcher of LibreOffice and its bridge: `libremodular-launch libreoffice`.
//!
//! Why this module exists
//! LibreOffice only listens for UNO when it was started with an `--accept`
//! option, and its bridge is a separate process that has to find the token and the
//! ports. Doing it by hand invites a token on a command line and an attachment to
//! an instance that holds unsaved work. Here the argument lists are fixed by code,
//! the token travels as the path of its file and never as its content, and nothing
//! is started next to a LibreOffice the operator already runs.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rustix::process::{Pid, Signal, kill_process};

use crate::config::{self, AppSettings, Config, ConfigError, Mode, Paths};
use crate::protocol::BRIDGE_ADDRESS;
use crate::security::{
    SecurityError, check_trusted_file, ensure_private_dir, read_control_file, read_system_file,
    replace_private_file,
};

/// The LibreOffice executable, unless the caller gives another.
pub const DEFAULT_SOFFICE: &str = "/usr/bin/soffice";
/// The interpreter that runs the bridge: the system one, which has `uno`.
pub const DEFAULT_PYTHON: &str = "/usr/bin/python3";
/// Where the processes of the system are listed.
pub const DEFAULT_PROC: &str = "/proc";
/// Put on `LD_LIBRARY_PATH` so that `import uno` finds LibreOffice's libraries.
pub const LIBREOFFICE_LIBRARIES: &str = "/usr/lib/libreoffice/program";
/// Put on `PYTHONPATH` so that the system interpreter finds `uno`.
pub const PYTHON_PACKAGES: &str = "/usr/lib/python3/dist-packages";
/// The only variables the bridge inherits from the launcher.
pub const BRIDGE_INHERITED: [&str; 4] = ["PATH", "HOME", "LANG", "LC_ALL"];
/// The variables of this program's own: no application has a use for them, so none
/// is handed to LibreOffice.
pub const OWN_VARIABLES_PREFIX: &str = "LIBREMODULAR_";
/// The name of the process a running LibreOffice has.
pub const SOFFICE_PROCESS: &str = "soffice.bin";
/// The directory, under the volatile one, that holds what a launch leaves.
pub const LAUNCH_DIR: &str = "launch";
/// The file that records the processes of the last launch.
pub const RECORD_FILE: &str = "libreoffice.pids";
/// The applications whose bridge the operator starts by hand.
pub const START_BY_HAND: [&str; 4] = ["gimp", "krita", "blender", "inkscape"];
/// Exit code of a launch that failed while doing its work.
pub const EXIT_FAILED: u8 = 1;
/// Exit code of a refusal: a wrong usage, or something the operator must change.
pub const EXIT_REFUSED: u8 = 2;

const TARGET_PREFIX: &str = "libreoffice-";
const RECORD_LIMIT: u64 = 4096;
const PROC_FILE_LIMIT: u64 = 64 * 1024;

const USAGE: &str = "usage:\n  \
    libremodular-launch libreoffice --bridge-file <absolute path of uno_bridge.py>\n  \
    libremodular-launch libreoffice --stop\n\
    The bridges of the other applications are started by hand: see docs/modules/<name>.md\n";

/// Where the programs are, and where the processes of the system are listed.
/// Fixed by the caller: nothing a script or the model says can change them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programs {
    pub soffice: PathBuf,
    pub python: PathBuf,
    pub proc_dir: PathBuf,
}

impl Default for Programs {
    fn default() -> Self {
        Programs {
            soffice: PathBuf::from(DEFAULT_SOFFICE),
            python: PathBuf::from(DEFAULT_PYTHON),
            proc_dir: PathBuf::from(DEFAULT_PROC),
        }
    }
}

/// What a launch needs, all decided before anything starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub uno_port: u16,
    pub bridge_port: u16,
    pub bridge_file: PathBuf,
    /// The file that holds the token. The token itself never leaves it.
    pub token_file: PathBuf,
}

/// A process to start, described completely before anything runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
    /// The whole environment of the process: nothing else is inherited.
    pub env: Vec<(OsString, OsString)>,
}

/// Why a launch or a stop did not happen.
#[derive(Debug)]
pub enum LaunchError {
    /// No target of LibreOffice is enabled in the mode `uno`.
    NoTarget,
    /// Enabled targets name different values for one key.
    PortsDisagree {
        key: &'static str,
        values: Vec<(&'static str, Option<u16>)>,
    },
    /// The two ports are the same.
    PortsClash { port: u16 },
    /// The bridge file cannot be trusted.
    BridgeFile(SecurityError),
    /// An instance of LibreOffice of the user already runs.
    AlreadyRunning { pids: Vec<u32>, uno_port: u16 },
    /// A bridge started by an earlier launch still runs.
    BridgeStillRunning { pid: u32 },
    /// The configuration cannot be read or is not usable.
    Config(ConfigError),
    /// A file or a directory of the launcher cannot be used.
    Files(SecurityError),
    /// The record of an earlier launch does not make sense.
    MalformedRecord(PathBuf),
    /// The processes of the system cannot be listed.
    Processes(io::Error),
    /// A program could not be started.
    Start { program: PathBuf, source: io::Error },
    /// The bridge did not start after LibreOffice did.
    BridgeNotStarted {
        soffice_pid: u32,
        source: Box<LaunchError>,
    },
    /// A signal could not be sent.
    Signal { pid: u32, source: io::Error },
    /// The report could not be written.
    Output(io::Error),
}

impl LaunchError {
    /// The exit code that goes with the error.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            LaunchError::NoTarget
            | LaunchError::PortsDisagree { .. }
            | LaunchError::PortsClash { .. }
            | LaunchError::BridgeFile(_)
            | LaunchError::AlreadyRunning { .. }
            | LaunchError::BridgeStillRunning { .. } => EXIT_REFUSED,
            _ => EXIT_FAILED,
        }
    }
}

fn pid_list(pids: &[u32]) -> String {
    pids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::NoTarget => f.write_str(
                "no LibreOffice target is enabled: set enabled = true under \
                 [apps.libreoffice-writer], [apps.libreoffice-calc] or \
                 [apps.libreoffice-impress] in the configuration",
            ),
            LaunchError::PortsDisagree { key, values } => {
                let listing = values
                    .iter()
                    .map(|(target, port)| match port {
                        Some(port) => format!("apps.{target}.{key} = {port}"),
                        None => format!("apps.{target}.{key} is not set"),
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(
                    f,
                    "the enabled LibreOffice targets disagree on {key}: {listing}; \
                     one LibreOffice and one bridge serve all of them, so give them the same value"
                )
            }
            LaunchError::PortsClash { port } => write!(
                f,
                "uno_port and bridge_port are both {port}: LibreOffice and the bridge \
                 cannot listen on the same port"
            ),
            LaunchError::BridgeFile(error) => write!(
                f,
                "the bridge file is refused: {error}; pass the absolute path of \
                 bridges/generated/uno_bridge.py with --bridge-file"
            ),
            LaunchError::AlreadyRunning { pids, uno_port } => write!(
                f,
                "LibreOffice is already running (process {}). The launcher does not attach to \
                 an instance that may hold unsaved work and never closes one it did not start. \
                 Close LibreOffice and run the launcher again, or start it yourself with the \
                 option --accept=\"socket,host={BRIDGE_ADDRESS},port={uno_port};urp;\" \
                 (see docs/modules/libreoffice.md) and start the bridge by hand.",
                pid_list(pids)
            ),
            LaunchError::BridgeStillRunning { pid } => write!(
                f,
                "the bridge of an earlier launch is still running (process {pid}); \
                 run 'libremodular-launch libreoffice --stop' first"
            ),
            LaunchError::Config(error) => write!(f, "{error}"),
            LaunchError::Files(error) => write!(f, "{error}"),
            LaunchError::MalformedRecord(path) => write!(
                f,
                "the record of an earlier launch ({}) is not in the expected form; \
                 nothing was signalled. Check the processes yourself, then delete the file",
                path.display()
            ),
            LaunchError::Processes(error) => {
                write!(f, "cannot list the processes of the system: {error}")
            }
            LaunchError::Start { program, source } => {
                write!(f, "cannot start {}: {source}", program.display())
            }
            LaunchError::BridgeNotStarted {
                soffice_pid,
                source,
            } => write!(
                f,
                "{source}. LibreOffice was started (process {soffice_pid}) and is left running: \
                 close it yourself"
            ),
            LaunchError::Signal { pid, source } => {
                write!(f, "cannot signal process {pid}: {source}")
            }
            LaunchError::Output(error) => write!(f, "cannot write the report: {error}"),
        }
    }
}

impl std::error::Error for LaunchError {}

// --------------------------------------------------------------------------
// The commands: pure, nothing starts here
// --------------------------------------------------------------------------

/// The option that makes LibreOffice listen for UNO, on the loopback address only.
#[must_use]
pub fn accept_argument(uno_port: u16) -> OsString {
    OsString::from(format!(
        "--accept=socket,host={BRIDGE_ADDRESS},port={uno_port};urp;"
    ))
}

/// LibreOffice with its window. A window needs the display, the session bus and
/// the user's settings, and the operator would give it the same by starting it
/// from a terminal: it receives the environment of the launcher, apart from the
/// variables of this program's own ([`OWN_VARIABLES_PREFIX`]).
#[must_use]
pub fn soffice_spawn(
    programs: &Programs,
    plan: &Plan,
    environment: &[(OsString, OsString)],
) -> Spawn {
    Spawn {
        program: programs.soffice.clone(),
        args: vec![
            accept_argument(plan.uno_port),
            "--norestore".into(),
            "--nologo".into(),
            "--nodefault".into(),
        ],
        current_dir: PathBuf::from("/"),
        env: environment
            .iter()
            .filter(|(name, _)| !name.as_bytes().starts_with(OWN_VARIABLES_PREFIX.as_bytes()))
            .cloned()
            .collect(),
    }
}

/// The environment of the bridge: four inherited variables, the two that let the
/// system interpreter find `uno`, the two ports and the path of the token file.
#[must_use]
pub fn bridge_environment(
    environment: &[(OsString, OsString)],
    plan: &Plan,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = environment
        .iter()
        .filter(|(name, _)| BRIDGE_INHERITED.iter().any(|allowed| name == allowed))
        .cloned()
        .collect();
    env.push(("LD_LIBRARY_PATH".into(), LIBREOFFICE_LIBRARIES.into()));
    env.push(("PYTHONPATH".into(), PYTHON_PACKAGES.into()));
    env.push((
        "LIBREMODULAR_UNO_PORT".into(),
        plan.uno_port.to_string().into(),
    ));
    env.push((
        "LIBREMODULAR_BRIDGE_PORT".into(),
        plan.bridge_port.to_string().into(),
    ));
    env.push((
        "LIBREMODULAR_TOKEN_PATH".into(),
        plan.token_file.as_os_str().to_owned(),
    ));
    env
}

/// `python3 <bridge file>`.
#[must_use]
pub fn bridge_spawn(
    programs: &Programs,
    plan: &Plan,
    environment: &[(OsString, OsString)],
) -> Spawn {
    Spawn {
        program: programs.python.clone(),
        args: vec![plan.bridge_file.as_os_str().to_owned()],
        current_dir: PathBuf::from("/"),
        env: bridge_environment(environment, plan),
    }
}

// --------------------------------------------------------------------------
// The plan, from the configuration
// --------------------------------------------------------------------------

fn libreoffice_targets(config: &Config) -> Vec<&AppSettings> {
    config
        .apps()
        .iter()
        .filter(|app| app.enabled && app.mode == Mode::Uno && app.name.starts_with(TARGET_PREFIX))
        .collect()
}

/// The one value an enabled target names for `key`, or the disagreement.
fn agreed_port(
    targets: &[&AppSettings],
    key: &'static str,
    read: fn(&AppSettings) -> Option<u16>,
) -> Result<u16, LaunchError> {
    let values: Vec<(&'static str, Option<u16>)> = targets
        .iter()
        .map(|target| (target.name, read(target)))
        .collect();
    let first = values.first().and_then(|(_, port)| *port);
    match first {
        Some(port) if values.iter().all(|(_, other)| *other == Some(port)) => Ok(port),
        _ => Err(LaunchError::PortsDisagree { key, values }),
    }
}

/// The ports of the enabled targets of LibreOffice, which must all agree.
///
/// # Errors
/// No target enabled, disagreeing or missing ports, or the two ports equal.
pub fn plan_from_config(
    config: &Config,
    bridge_file: PathBuf,
    token_file: PathBuf,
) -> Result<Plan, LaunchError> {
    let targets = libreoffice_targets(config);
    if targets.is_empty() {
        return Err(LaunchError::NoTarget);
    }
    let uno_port = agreed_port(&targets, "uno_port", |app| app.uno_port)?;
    let bridge_port = agreed_port(&targets, "bridge_port", |app| app.bridge_port)?;
    if uno_port == bridge_port {
        return Err(LaunchError::PortsClash { port: uno_port });
    }
    Ok(Plan {
        uno_port,
        bridge_port,
        bridge_file,
        token_file,
    })
}

// --------------------------------------------------------------------------
// The processes of the system
// --------------------------------------------------------------------------

fn current_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// The processes of the current user that are a running LibreOffice, read from
/// the listing of `proc_dir` without starting any command.
///
/// # Errors
/// [`LaunchError::Processes`] when the listing cannot be read: the launch does
/// not go on without knowing.
pub fn running_soffice(proc_dir: &Path, uid: u32) -> Result<Vec<u32>, LaunchError> {
    let mut found = Vec::new();
    for entry in fs::read_dir(proc_dir).map_err(LaunchError::Processes)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|text| text.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(info) = entry.metadata() else { continue };
        if info.uid() != uid {
            continue;
        }
        let Ok(comm) = read_system_file(&entry.path().join("comm"), PROC_FILE_LIMIT) else {
            continue;
        };
        if comm.trim_ascii() == SOFFICE_PROCESS.as_bytes() {
            found.push(pid);
        }
    }
    found.sort_unstable();
    Ok(found)
}

/// Whether `pid` is a process started as `python3 <bridge_file>`: one of its
/// arguments is exactly the bridge file. A process that is gone, one the user
/// cannot read, or any other program is not.
#[must_use]
pub fn is_the_bridge(proc_dir: &Path, pid: u32, bridge_file: &Path) -> bool {
    let cmdline = proc_dir.join(pid.to_string()).join("cmdline");
    let Ok(bytes) = read_system_file(&cmdline, PROC_FILE_LIMIT) else {
        return false;
    };
    bytes
        .split(|byte| *byte == 0)
        .any(|argument| argument == bridge_file.as_os_str().as_bytes())
}

// --------------------------------------------------------------------------
// The record of a launch
// --------------------------------------------------------------------------

/// The processes a launch started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub soffice_pid: Option<u32>,
    pub bridge: Option<(u32, PathBuf)>,
}

impl Record {
    /// One line per process: `soffice <pid>` and `bridge <pid> <bridge file>`.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = String::new();
        if let Some(pid) = self.soffice_pid {
            text.push_str(&format!("soffice {pid}\n"));
        }
        if let Some((pid, file)) = &self.bridge {
            text.push_str(&format!("bridge {pid} {}\n", file.display()));
        }
        text
    }

    /// Read what [`Record::render`] wrote; `None` for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Record> {
        let mut record = Record {
            soffice_pid: None,
            bridge: None,
        };
        for line in text.lines() {
            if let Some(pid) = line.strip_prefix("soffice ") {
                record.soffice_pid = Some(pid.parse().ok()?);
            } else {
                let rest = line.strip_prefix("bridge ")?;
                let (pid, file) = rest.split_once(' ')?;
                if !file.starts_with('/') {
                    return None;
                }
                record.bridge = Some((pid.parse().ok()?, PathBuf::from(file)));
            }
        }
        Some(record)
    }
}

fn record_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(LAUNCH_DIR).join(RECORD_FILE)
}

/// The record of the last launch, if there is one.
fn read_record(path: &Path) -> Result<Option<Record>, LaunchError> {
    match read_control_file(path, RECORD_LIMIT) {
        Ok(bytes) => String::from_utf8(bytes)
            .ok()
            .and_then(|text| Record::parse(&text))
            .map(Some)
            .ok_or_else(|| LaunchError::MalformedRecord(path.to_path_buf())),
        Err(error) if error.is_not_found() => Ok(None),
        Err(error) => Err(LaunchError::Files(error)),
    }
}

// --------------------------------------------------------------------------
// Starting and stopping
// --------------------------------------------------------------------------

/// Start `spawn` in a new process group, without a shell, with exactly its
/// environment and with its input and output sent nowhere. The process outlives
/// the launcher: the handle is dropped without waiting for it.
fn spawn_detached(spawn: &Spawn) -> Result<u32, LaunchError> {
    let child = Command::new(&spawn.program)
        .args(&spawn.args)
        .current_dir(&spawn.current_dir)
        .env_clear()
        .envs(spawn.env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|source| LaunchError::Start {
            program: spawn.program.clone(),
            source,
        })?;
    Ok(child.id())
}

fn say(out: &mut dyn Write, line: &str) -> Result<(), LaunchError> {
    writeln!(out, "{line}").map_err(LaunchError::Output)
}

fn prepare_launch_dir(paths: &Paths) -> Result<PathBuf, LaunchError> {
    let runtime = paths.require_runtime_dir().map_err(LaunchError::Config)?;
    let launch_dir = runtime.join(LAUNCH_DIR);
    ensure_private_dir(runtime)
        .and_then(|()| ensure_private_dir(&launch_dir))
        .map_err(LaunchError::Files)?;
    Ok(record_path(runtime))
}

/// Start LibreOffice and its bridge, then record their processes.
///
/// # Errors
/// A refusal before anything starts (see [`LaunchError::exit_code`]), or a
/// failure while starting.
pub fn launch(
    programs: &Programs,
    paths: &Paths,
    plan: &Plan,
    environment: &[(OsString, OsString)],
    out: &mut dyn Write,
) -> Result<(), LaunchError> {
    check_trusted_file(&plan.bridge_file).map_err(LaunchError::BridgeFile)?;
    let record_file = prepare_launch_dir(paths)?;
    if let Some(Record {
        bridge: Some((pid, file)),
        ..
    }) = read_record(&record_file)?
        && is_the_bridge(&programs.proc_dir, pid, &file)
    {
        return Err(LaunchError::BridgeStillRunning { pid });
    }
    let running = running_soffice(&programs.proc_dir, current_uid())?;
    if !running.is_empty() {
        return Err(LaunchError::AlreadyRunning {
            pids: running,
            uno_port: plan.uno_port,
        });
    }
    let soffice_pid = spawn_detached(&soffice_spawn(programs, plan, environment))?;
    let bridge_pid = match spawn_detached(&bridge_spawn(programs, plan, environment)) {
        Ok(pid) => pid,
        Err(error) => {
            let alone = Record {
                soffice_pid: Some(soffice_pid),
                bridge: None,
            };
            let _ = replace_private_file(&record_file, alone.render().as_bytes());
            return Err(LaunchError::BridgeNotStarted {
                soffice_pid,
                source: Box::new(error),
            });
        }
    };
    let record = Record {
        soffice_pid: Some(soffice_pid),
        bridge: Some((bridge_pid, plan.bridge_file.clone())),
    };
    replace_private_file(&record_file, record.render().as_bytes()).map_err(LaunchError::Files)?;
    say(
        out,
        &format!(
            "LibreOffice started (process {soffice_pid}), listening for UNO on {BRIDGE_ADDRESS}:{}",
            plan.uno_port
        ),
    )?;
    say(
        out,
        &format!(
            "bridge started (process {bridge_pid}), listening on {BRIDGE_ADDRESS}:{}",
            plan.bridge_port
        ),
    )?;
    say(
        out,
        "a script runs on the current document of its target's family, or with doc set to None when none is open",
    )?;
    say(
        out,
        "stop the bridge with 'libremodular-launch libreoffice --stop'; LibreOffice is yours to close",
    )
}

/// Stop the bridge of the last launch, and only it.
///
/// The recorded process is signalled only if it still is the bridge: a number
/// that another program has taken since is never touched. LibreOffice is never
/// closed: it may hold work, and closing it is the operator's gesture.
///
/// # Errors
/// An unreadable or malformed record, or a signal that cannot be sent.
pub fn stop(programs: &Programs, paths: &Paths, out: &mut dyn Write) -> Result<(), LaunchError> {
    let runtime = paths.require_runtime_dir().map_err(LaunchError::Config)?;
    let record_file = record_path(runtime);
    let Some(record) = read_record(&record_file)? else {
        return say(out, "nothing to stop: no launch is recorded");
    };
    match &record.bridge {
        Some((pid, file)) if is_the_bridge(&programs.proc_dir, *pid, file) => {
            let target = i32::try_from(*pid)
                .ok()
                .and_then(Pid::from_raw)
                .ok_or_else(|| LaunchError::MalformedRecord(record_file.clone()))?;
            kill_process(target, Signal::TERM).map_err(|errno| LaunchError::Signal {
                pid: *pid,
                source: io::Error::from(errno),
            })?;
            say(out, &format!("bridge stopped (process {pid})"))?;
        }
        Some((pid, _)) => say(
            out,
            &format!("the bridge (process {pid}) is no longer running: nothing was signalled"),
        )?,
        None => say(out, "no bridge is recorded: nothing was signalled")?,
    }
    match fs::remove_file(&record_file) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(LaunchError::Files(SecurityError::Io {
                action: "remove",
                path: record_file,
                source: error,
            }));
        }
    }
    let soffice = record.soffice_pid.map_or_else(
        || "LibreOffice".to_owned(),
        |pid| format!("LibreOffice (process {pid})"),
    );
    say(
        out,
        &format!("{soffice} was left running: close it yourself when you are done"),
    )
}

// --------------------------------------------------------------------------
// The command line
// --------------------------------------------------------------------------

/// What the arguments ask for.
#[derive(Debug, PartialEq, Eq)]
enum Request {
    Start { bridge_file: PathBuf },
    Stop,
    Help,
    ByHand(&'static str),
    Usage,
}

fn parse_arguments(args: &[OsString]) -> Request {
    let mut rest = args.iter().skip(1);
    let Some(name) = rest.next() else {
        return Request::Usage;
    };
    let name = name.to_str().unwrap_or_default();
    if matches!(name, "--help" | "-h") {
        return Request::Help;
    }
    if let Some(known) = START_BY_HAND.iter().find(|known| **known == name) {
        return Request::ByHand(known);
    }
    if name != "libreoffice" {
        return Request::Usage;
    }
    let options: Vec<&OsString> = rest.collect();
    match options.as_slice() {
        [flag] if *flag == OsStr::new("--stop") => Request::Stop,
        [flag] if *flag == OsStr::new("--help") => Request::Help,
        [flag, file] if *flag == OsStr::new("--bridge-file") => Request::Start {
            bridge_file: PathBuf::from(file),
        },
        [joined] => joined
            .as_bytes()
            .strip_prefix(b"--bridge-file=")
            .filter(|file| !file.is_empty())
            .map_or(Request::Usage, |file| Request::Start {
                bridge_file: PathBuf::from(OsStr::from_bytes(file)),
            }),
        _ => Request::Usage,
    }
}

fn start(
    programs: &Programs,
    paths: &Paths,
    bridge_file: PathBuf,
    environment: &[(OsString, OsString)],
    out: &mut dyn Write,
) -> Result<(), LaunchError> {
    let config = config::load(&paths.config_file()).map_err(LaunchError::Config)?;
    let plan = plan_from_config(&config, bridge_file, paths.token_file())?;
    launch(programs, paths, &plan, environment, out)
}

/// Run the `libremodular-launch` command line and return its exit code.
///
/// `locate` provides the base directories, `programs` the executables and the
/// listing of processes, `environment` what the launcher received: the binary
/// passes the real ones, and tests pass fakes.
pub fn run(
    args: impl IntoIterator<Item = OsString>,
    locate: &dyn Fn() -> Result<Paths, ConfigError>,
    programs: &Programs,
    environment: &[(OsString, OsString)],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let args: Vec<OsString> = args.into_iter().collect();
    let mut located = || {
        locate().map_err(|error| {
            let _ = writeln!(err, "libremodular-launch: error: {error}");
            EXIT_FAILED
        })
    };
    let outcome = match parse_arguments(&args) {
        Request::Help => {
            let _ = write!(out, "{USAGE}");
            return 0;
        }
        Request::Usage => {
            let _ = write!(err, "{USAGE}");
            return EXIT_REFUSED;
        }
        Request::ByHand(name) => {
            let _ = writeln!(
                err,
                "libremodular-launch: {name}: start the bridge by hand, see docs/modules/{name}.md"
            );
            return EXIT_REFUSED;
        }
        Request::Start { bridge_file } => match located() {
            Ok(paths) => start(programs, &paths, bridge_file, environment, out),
            Err(code) => return code,
        },
        Request::Stop => match located() {
            Ok(paths) => stop(programs, &paths, out),
            Err(code) => return code,
        },
    };
    match outcome {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "libremodular-launch: {error}");
            error.exit_code()
        }
    }
}

/// The launcher on the real directories, programs and environment: what the
/// binary runs.
pub fn run_in_process(
    args: impl IntoIterator<Item = OsString>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    run(
        args,
        &config::resolve_paths_from_process,
        &Programs::default(),
        &config::environment_snapshot(),
        out,
        err,
    )
}
