//! The `cli` module of Inkscape: one Python process per script, started from a fixed argument list.
//!
//! Why this module exists
//! Inkscape keeps no scripting session to join, so each script runs in a process
//! of its own. That process must not inherit the Hub's environment or token, must
//! not go through a shell, and must end with all of its children when its time is up.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use serde_json::{Map, Value, json};

use crate::config::Limits;
use crate::modules::{ExecReply, Health, Module, ModuleError, StateReply};

/// The interpreter that runs the scripts, unless the constructor is given another.
pub const DEFAULT_PYTHON: &str = "/usr/bin/python3";
/// Where `inkex` lives on Debian, put on `PYTHONPATH`.
pub const DEFAULT_EXTENSIONS: &str = "/usr/share/inkscape/extensions";
/// The Inkscape executable, used for its version and for captures.
pub const DEFAULT_INKSCAPE: &str = "/usr/bin/inkscape";
/// The working document, inside the work directory.
pub const DOCUMENT_NAME: &str = "current.svg";
/// The file the script is written to, inside the work directory.
pub const SCRIPT_NAME: &str = "script.py";
/// The only variables a started process inherits from the Hub.
pub const INHERITED_VARIABLES: [&str; 4] = ["PATH", "HOME", "LANG", "LC_ALL"];
/// The address space of a script process, by default.
pub const DEFAULT_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Open files a script process may hold.
pub const OPEN_FILES_LIMIT: u64 = 256;
/// How long a capture may take.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `inkscape --version` may take.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// The prefix of the line a script prints last to return a value.
pub const RESULT_PREFIX: &str = "LMM_RESULT: ";
/// The prefix of the line the bootstrap writes last on the error stream.
pub const ERROR_PREFIX: &str = "LMM_ERROR: ";

const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long the output readers may take once the process group is gone.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);
const READ_CHUNK: usize = 64 * 1024;

/// The document created at first use: a valid SVG with one empty layer.
pub const EMPTY_DOCUMENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" width="210mm" height="297mm" viewBox="0 0 210 297">
  <g inkscape:groupmode="layer" id="layer1" inkscape:label="Layer 1"/>
</svg>
"#;

/// Runs a script: applies the limits, then compiles the script as
/// `<libremodular>` and reports an exception on the last line of the error stream.
/// Arguments: script file, CPU seconds, address space bytes, open files.
pub const SCRIPT_BOOTSTRAP: &str = r#"import sys
sys.path[:] = [entry for entry in sys.path if entry not in ("", ".")]
import json
import resource
import traceback


def report(kind, message, trace):
    sys.stdout.flush()
    line = json.dumps({"type": kind, "message": message, "traceback": trace})
    sys.stderr.write("\nLMM_ERROR: " + line + "\n")
    sys.stderr.flush()


def cap(limit, value):
    soft, hard = resource.getrlimit(limit)
    if hard != resource.RLIM_INFINITY:
        value = min(value, hard)
    resource.setrlimit(limit, (value, value))


try:
    target = sys.argv[1]
    cap(resource.RLIMIT_CORE, 0)
    cap(resource.RLIMIT_CPU, int(sys.argv[2]))
    cap(resource.RLIMIT_AS, int(sys.argv[3]))
    cap(resource.RLIMIT_NOFILE, int(sys.argv[4]))
    with open(target, encoding="utf-8") as handle:
        source = handle.read()
except Exception as error:
    report("LimitError", "the process could not be prepared: %s" % error, "")
    sys.exit(3)
sys.argv = [target]
namespace = {"__name__": "__main__"}
try:
    exec(compile(source, "<libremodular>", "exec", dont_inherit=True), namespace)
except BaseException as error:
    frames = error.__traceback__
    while frames is not None and frames.tb_frame.f_code.co_filename != "<libremodular>":
        frames = frames.tb_next
    try:
        message = str(error)
    except Exception:
        message = "<the exception could not be shown>"
    trace = "".join(traceback.format_exception(type(error), error, frames))
    report(type(error).__name__, message, trace)
    sys.exit(1)
"#;

/// Reads the working document: applies the limits, refuses any entity
/// declaration, never resolves or fetches anything, and prints the state.
/// Arguments: document, CPU seconds, address space bytes, open files.
pub const STATE_BOOTSTRAP: &str = r#"import sys
sys.path[:] = [entry for entry in sys.path if entry not in ("", ".")]
import json
import resource
from xml.etree import ElementTree
from xml.parsers import expat

SVG = "{http://www.w3.org/2000/svg}"
INKSCAPE = "{http://www.inkscape.org/namespaces/inkscape}"


def report(kind, message):
    line = json.dumps({"type": kind, "message": message, "traceback": ""})
    sys.stderr.write("\nLMM_ERROR: " + line + "\n")
    sys.stderr.flush()


def cap(limit, value):
    soft, hard = resource.getrlimit(limit)
    if hard != resource.RLIM_INFINITY:
        value = min(value, hard)
    resource.setrlimit(limit, (value, value))


class Refused(Exception):
    pass


def refuse(*arguments):
    raise Refused("the document declares entities")


try:
    document = sys.argv[1]
    cap(resource.RLIMIT_CORE, 0)
    cap(resource.RLIMIT_CPU, int(sys.argv[2]))
    cap(resource.RLIMIT_AS, int(sys.argv[3]))
    cap(resource.RLIMIT_NOFILE, int(sys.argv[4]))
    with open(document, "rb") as handle:
        data = handle.read()
except Exception as error:
    report("LimitError", "the process could not be prepared: %s" % error)
    sys.exit(3)
try:
    scanner = expat.ParserCreate()
    scanner.SetParamEntityParsing(expat.XML_PARAM_ENTITY_PARSING_NEVER)
    scanner.EntityDeclHandler = refuse
    scanner.UnparsedEntityDeclHandler = refuse
    scanner.ExternalEntityRefHandler = refuse
    scanner.Parse(data, True)
    root = ElementTree.fromstring(data)
except Refused as error:
    report("DocumentRefused", str(error))
    sys.exit(1)
except Exception as error:
    report(type(error).__name__, str(error))
    sys.exit(1)
layers = [
    {"id": node.get("id"), "label": node.get(INKSCAPE + "label")}
    for node in root.iter(SVG + "g")
    if node.get(INKSCAPE + "groupmode") == "layer"
]
state = {
    "width": root.get("width"),
    "height": root.get("height"),
    "element_count": sum(1 for _ in root.iter()),
    "layers": layers,
}
print(json.dumps(state))
"#;

/// Where the module finds the programs it starts. Fixed at construction: neither
/// a script nor the configuration can change them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InkscapePaths {
    pub python: PathBuf,
    pub extensions: PathBuf,
    pub inkscape: PathBuf,
}

impl Default for InkscapePaths {
    fn default() -> Self {
        InkscapePaths {
            python: PathBuf::from(DEFAULT_PYTHON),
            extensions: PathBuf::from(DEFAULT_EXTENSIONS),
            inkscape: PathBuf::from(DEFAULT_INKSCAPE),
        }
    }
}

/// The file operations the module needs. The Hub provides them, with its own
/// checks: the module never opens a file itself.
pub trait WorkFiles: Send + Sync {
    /// Create the document holding `initial` when it does not exist; refuse
    /// anything but a private regular file of the user.
    ///
    /// # Errors
    /// Why the document cannot be used.
    fn ensure_document(&self, path: &Path, initial: &[u8]) -> Result<(), String>;

    /// Write the script to a private file created for it.
    ///
    /// # Errors
    /// Why the script cannot be written.
    fn write_script(&self, path: &Path, text: &[u8]) -> Result<(), String>;
}

/// A process to start, described completely before anything runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
    /// The whole environment of the process: nothing else is inherited.
    pub env: Vec<(OsString, OsString)>,
}

/// What a finished process left.
#[derive(Debug)]
pub struct Finished {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Why a process produced no outcome.
#[derive(Debug)]
pub enum RunError {
    /// The program could not be started.
    Start(io::Error),
    /// The process could not be waited for.
    Wait(io::Error),
    /// The time ran out; the whole process group was killed.
    TimedOut,
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Start(error) => write!(f, "cannot start the process: {error}"),
            RunError::Wait(error) => write!(f, "cannot wait for the process: {error}"),
            RunError::TimedOut => f.write_str("the process did not finish in time and was killed"),
        }
    }
}

impl std::error::Error for RunError {}

/// The environment of every process the module starts: the inherited variables
/// that belong to [`INHERITED_VARIABLES`], then the ones the module sets.
#[must_use]
pub fn process_environment(
    inherited: &[(OsString, OsString)],
    extensions: &Path,
    document: Option<(&Path, &Path)>,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = inherited
        .iter()
        .filter(|(name, _)| INHERITED_VARIABLES.iter().any(|allowed| name == allowed))
        .cloned()
        .collect();
    env.push(("PYTHONPATH".into(), extensions.as_os_str().to_owned()));
    if let Some((document, work_dir)) = document {
        env.push(("LMM_DOCUMENT".into(), document.as_os_str().to_owned()));
        env.push(("LMM_WORK_DIR".into(), work_dir.as_os_str().to_owned()));
    }
    env
}

fn limit_arguments(cpu_seconds: u32, memory_bytes: u64) -> [OsString; 3] {
    [
        cpu_seconds.max(1).to_string().into(),
        memory_bytes.to_string().into(),
        OPEN_FILES_LIMIT.to_string().into(),
    ]
}

/// `python3 -c <bootstrap> <script file> <limits>`: the script text itself is
/// never on the command line.
#[must_use]
pub fn script_command(
    paths: &InkscapePaths,
    env: Vec<(OsString, OsString)>,
    work_dir: &Path,
    script_file: &Path,
    cpu_seconds: u32,
    memory_bytes: u64,
) -> ProcessSpec {
    let mut args: Vec<OsString> = vec![
        "-c".into(),
        SCRIPT_BOOTSTRAP.into(),
        script_file.as_os_str().to_owned(),
    ];
    args.extend(limit_arguments(cpu_seconds, memory_bytes));
    ProcessSpec {
        program: paths.python.clone(),
        args,
        current_dir: work_dir.to_path_buf(),
        env,
    }
}

/// `python3 -c <state bootstrap> <document> <limits>`.
#[must_use]
pub fn state_command(
    paths: &InkscapePaths,
    env: Vec<(OsString, OsString)>,
    work_dir: &Path,
    document: &Path,
    cpu_seconds: u32,
    memory_bytes: u64,
) -> ProcessSpec {
    let mut args: Vec<OsString> = vec![
        "-c".into(),
        STATE_BOOTSTRAP.into(),
        document.as_os_str().to_owned(),
    ];
    args.extend(limit_arguments(cpu_seconds, memory_bytes));
    ProcessSpec {
        program: paths.python.clone(),
        args,
        current_dir: work_dir.to_path_buf(),
        env,
    }
}

/// `inkscape --export-type=png --export-filename=<capture> <document>`.
#[must_use]
pub fn capture_command(
    paths: &InkscapePaths,
    env: Vec<(OsString, OsString)>,
    work_dir: &Path,
    document: &Path,
    capture: &Path,
) -> ProcessSpec {
    let mut target = OsString::from("--export-filename=");
    target.push(capture.as_os_str());
    ProcessSpec {
        program: paths.inkscape.clone(),
        args: vec![
            "--export-type=png".into(),
            target,
            document.as_os_str().to_owned(),
        ],
        current_dir: work_dir.to_path_buf(),
        env,
    }
}

/// `inkscape --version`, from the root directory.
#[must_use]
pub fn version_command(paths: &InkscapePaths, env: Vec<(OsString, OsString)>) -> ProcessSpec {
    ProcessSpec {
        program: paths.inkscape.clone(),
        args: vec!["--version".into()],
        current_dir: PathBuf::from("/"),
        env,
    }
}

/// Read a pipe to its end, keeping at most `limit` bytes: the rest is read and
/// dropped, so a talkative process never blocks on a full pipe.
fn drain(mut pipe: impl Read + Send + 'static, limit: usize) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = vec![0_u8; READ_CHUNK];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    let room = limit.saturating_sub(kept.len());
                    if let Some(part) = chunk.get(..count.min(room)) {
                        kept.extend_from_slice(part);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = sender.send(kept);
    });
    receiver
}

fn collected(reader: Option<Receiver<Vec<u8>>>) -> Vec<u8> {
    // A process that left the group may still hold the pipe: its output is not waited for.
    reader
        .and_then(|receiver| receiver.recv_timeout(OUTPUT_GRACE).ok())
        .unwrap_or_default()
}

/// Start `spec` in a new process group, without shell and with exactly its
/// environment, and wait at most `timeout`. Whatever happens, every process
/// still in the group is killed before the call returns.
///
/// # Errors
/// [`RunError`] when the program cannot start or the time runs out.
pub fn run(spec: &ProcessSpec, timeout: Duration, max_output: usize) -> Result<Finished, RunError> {
    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.current_dir)
        .env_clear()
        .envs(spec.env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(RunError::Start)?;
    let group = Pid::from_child(&child);
    let stdout = child.stdout.take().map(|pipe| drain(pipe, max_output));
    let stderr = child.stderr.take().map(|pipe| drain(pipe, max_output));
    let deadline = Instant::now() + timeout;
    let finished = loop {
        // NOWAIT leaves the leader unreaped: its process group id cannot be
        // given to another process before the group is killed below.
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        match waitid(WaitId::Pid(group), options) {
            Ok(Some(_)) => break true,
            Ok(None) | Err(Errno::INTR) => {}
            Err(_) => break true,
        }
        if Instant::now() >= deadline {
            break false;
        }
        thread::sleep(POLL_INTERVAL);
    };
    let _ = kill_process_group(group, Signal::KILL);
    let status = child.wait().map_err(RunError::Wait)?;
    let stdout = collected(stdout);
    let stderr = collected(stderr);
    if !finished {
        return Err(RunError::TimedOut);
    }
    Ok(Finished {
        status,
        stdout,
        stderr,
    })
}

fn signal_name(number: i32) -> &'static str {
    match number {
        6 => " (SIGABRT)",
        9 => " (SIGKILL)",
        11 => " (SIGSEGV)",
        15 => " (SIGTERM)",
        24 => " (SIGXCPU)",
        25 => " (SIGXFSZ)",
        _ => "",
    }
}

/// How a process ended, without inventing a traceback.
#[must_use]
pub fn describe_status(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        format!("the process exited with code {code}")
    } else if let Some(number) = status.signal() {
        format!(
            "the process was ended by signal {number}{}",
            signal_name(number)
        )
    } else {
        "the process ended without an exit code".to_owned()
    }
}

/// Split the last line off `text` when it starts with `prefix`: the text
/// without that line, and what follows the prefix.
fn take_last_line<'a>(text: &'a str, prefix: &str) -> Option<(&'a str, &'a str)> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let (before, last) = match body.rfind('\n') {
        Some(at) => (body.get(..=at)?, body.get(at + 1..)?),
        None => ("", body),
    };
    last.strip_prefix(prefix).map(|rest| (before, rest))
}

/// `stdout` without its `LMM_RESULT` line, and the value it carried.
#[must_use]
pub fn split_result(stdout: &str) -> (String, Value) {
    if let Some((before, rest)) = take_last_line(stdout, RESULT_PREFIX)
        && let Ok(value) = serde_json::from_str::<Value>(rest)
    {
        return (before.to_owned(), value);
    }
    (stdout.to_owned(), Value::Null)
}

fn text_of(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// `stderr` without the bootstrap's `LMM_ERROR` line (and the line break the
/// bootstrap put before it), and the error it carried.
#[must_use]
pub fn split_error(stderr: &str) -> (String, Option<Value>) {
    if let Some((before, rest)) = take_last_line(stderr, ERROR_PREFIX)
        && let Ok(Value::Object(error)) = serde_json::from_str::<Value>(rest)
    {
        let before = before.strip_suffix('\n').unwrap_or(before);
        let error = json!({
            "type": text_of(&error, "type"),
            "message": text_of(&error, "message"),
            "traceback": text_of(&error, "traceback"),
        });
        return (before.to_owned(), Some(error));
    }
    (stderr.to_owned(), None)
}

/// The reply to a script from what its process left.
#[must_use]
pub fn script_reply(finished: &Finished) -> ExecReply {
    let (stdout, result) = split_result(&String::from_utf8_lossy(&finished.stdout));
    let (stderr, error) = split_error(&String::from_utf8_lossy(&finished.stderr));
    let error = match error {
        Some(error) => Some(error),
        None if !finished.status.success() => Some(json!({
            "type": "ProcessError",
            "message": describe_status(finished.status),
            "traceback": "",
        })),
        None => None,
    };
    ExecReply {
        ok: error.is_none(),
        stdout,
        stderr,
        result: if error.is_none() { result } else { Value::Null },
        error: error.unwrap_or(Value::Null),
    }
}

/// The version `inkscape --version` printed, such as `1.2.2`.
fn parse_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next() == Some("Inkscape"))
            .then(|| words.next().map(str::to_owned))
            .flatten()
    })
}

/// The Inkscape module: no persistent process, one process per request.
pub struct InkscapeModule {
    paths: InkscapePaths,
    limits: Limits,
    memory_bytes: u64,
    inherited: Vec<(OsString, OsString)>,
    files: Arc<dyn WorkFiles>,
    busy: Mutex<()>,
    work_dir: Mutex<Option<PathBuf>>,
}

impl InkscapeModule {
    /// `inherited` holds the Hub's values of [`INHERITED_VARIABLES`]; anything
    /// else in it is ignored. `files` performs the file operations.
    #[must_use]
    pub fn new(
        paths: InkscapePaths,
        limits: Limits,
        inherited: Vec<(OsString, OsString)>,
        files: Arc<dyn WorkFiles>,
    ) -> Self {
        InkscapeModule {
            paths,
            limits,
            memory_bytes: DEFAULT_MEMORY_BYTES,
            inherited,
            files,
            busy: Mutex::new(()),
            work_dir: Mutex::new(None),
        }
    }

    /// The same module with another address space limit for its processes.
    #[must_use]
    pub fn with_memory_limit(mut self, bytes: u64) -> Self {
        self.memory_bytes = bytes;
        self
    }

    fn environment(&self, document: Option<(&Path, &Path)>) -> Vec<(OsString, OsString)> {
        process_environment(&self.inherited, &self.paths.extensions, document)
    }

    fn version(&self) -> Option<String> {
        let spec = version_command(&self.paths, self.environment(None));
        let finished = run(&spec, VERSION_TIMEOUT, self.limits.max_output_bytes).ok()?;
        if !finished.status.success() {
            return None;
        }
        parse_version(&String::from_utf8_lossy(&finished.stdout))
            .or_else(|| parse_version(&String::from_utf8_lossy(&finished.stderr)))
    }

    fn remember(&self, work_dir: &Path) {
        if let Ok(mut known) = self.work_dir.lock() {
            *known = Some(work_dir.to_path_buf());
        }
    }

    fn known_work_dir(&self) -> Option<PathBuf> {
        self.work_dir.lock().ok().and_then(|known| known.clone())
    }

    fn document(&self, work_dir: &Path) -> Result<PathBuf, ModuleError> {
        let document = work_dir.join(DOCUMENT_NAME);
        self.files
            .ensure_document(&document, EMPTY_DOCUMENT.as_bytes())
            .map_err(|why| {
                ModuleError::Unavailable(format!("the working document cannot be used: {why}"))
            })?;
        Ok(document)
    }

    fn read_state(
        &self,
        work_dir: &Path,
        warnings: &mut Vec<String>,
    ) -> Result<(PathBuf, Value), ModuleError> {
        let document = self.document(work_dir)?;
        let spec = state_command(
            &self.paths,
            self.environment(Some((&document, work_dir))),
            work_dir,
            &document,
            self.limits.default_timeout_s,
            self.memory_bytes,
        );
        let timeout = Duration::from_secs(u64::from(self.limits.default_timeout_s));
        let finished = run(&spec, timeout, self.limits.max_output_bytes).map_err(run_error)?;
        let (_, error) = split_error(&String::from_utf8_lossy(&finished.stderr));
        let parsed = serde_json::from_slice::<Value>(&finished.stdout).ok();
        let state = match (finished.status.success(), parsed, error) {
            (true, Some(state @ Value::Object(_)), _) => state,
            (_, _, Some(error)) => {
                let field = |key: &str| {
                    error
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                warnings.push(format!(
                    "the document was not read: {}: {}",
                    field("type"),
                    field("message")
                ));
                json!({})
            }
            _ => {
                warnings.push(format!(
                    "the document was not read: {}",
                    describe_status(finished.status)
                ));
                json!({})
            }
        };
        Ok((document, state))
    }

    fn capture(
        &self,
        work_dir: &Path,
        document: &Path,
        capture: &Path,
        warnings: &mut Vec<String>,
    ) -> Option<PathBuf> {
        let spec = capture_command(
            &self.paths,
            self.environment(Some((document, work_dir))),
            work_dir,
            document,
            capture,
        );
        match run(&spec, CAPTURE_TIMEOUT, self.limits.max_output_bytes) {
            Ok(finished) if finished.status.success() => Some(capture.to_path_buf()),
            Ok(finished) => {
                warnings.push(format!(
                    "no capture: inkscape failed, {}",
                    describe_status(finished.status)
                ));
                None
            }
            Err(error) => {
                warnings.push(format!("no capture: {error}"));
                None
            }
        }
    }
}

fn run_error(error: RunError) -> ModuleError {
    match error {
        RunError::TimedOut => ModuleError::Timeout,
        other => ModuleError::Unavailable(other.to_string()),
    }
}

impl Module for InkscapeModule {
    fn health(&self) -> Health {
        if !self.paths.python.is_file() {
            return Health::default();
        }
        match self.version() {
            Some(version) => Health {
                connected: true,
                app_version: Some(version),
                bridge_version: None,
            },
            None => Health::default(),
        }
    }

    fn execute(
        &self,
        script: &str,
        timeout_s: u32,
        work_dir: &Path,
    ) -> Result<ExecReply, ModuleError> {
        let _running = match self.busy.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Err(ModuleError::Busy),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        };
        self.remember(work_dir);
        let document = self.document(work_dir)?;
        let script_file = work_dir.join(SCRIPT_NAME);
        self.files
            .write_script(&script_file, script.as_bytes())
            .map_err(|why| {
                ModuleError::Unavailable(format!("the script cannot be written: {why}"))
            })?;
        let spec = script_command(
            &self.paths,
            self.environment(Some((&document, work_dir))),
            work_dir,
            &script_file,
            timeout_s,
            self.memory_bytes,
        );
        let outcome = run(
            &spec,
            Duration::from_secs(u64::from(timeout_s)),
            self.limits.max_output_bytes,
        );
        // The script stays on disk only while it runs.
        let _ = std::fs::remove_file(&script_file);
        outcome
            .map(|finished| script_reply(&finished))
            .map_err(run_error)
    }

    fn state(&self, capture_path: &Path) -> Result<StateReply, ModuleError> {
        let _running = match self.busy.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Err(ModuleError::Busy),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        };
        let app_version = self.version();
        let Some(work_dir) = self.known_work_dir() else {
            return Ok(StateReply {
                state: json!({}),
                screenshot: None,
                warnings: vec![
                    "no document yet: the working document is created by the first script"
                        .to_owned(),
                ],
                app_version,
                bridge_version: None,
            });
        };
        let mut warnings = Vec::new();
        let (document, state) = self.read_state(&work_dir, &mut warnings)?;
        let screenshot = self.capture(&work_dir, &document, capture_path, &mut warnings);
        Ok(StateReply {
            state,
            screenshot,
            warnings,
            app_version,
            bridge_version: None,
        })
    }

    /// No process outlives a request: there is nothing to release, and an
    /// Inkscape the operator opened is never touched.
    fn close(&self) {}
}
