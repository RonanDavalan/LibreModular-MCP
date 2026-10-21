//! Le module `cli` d'Inkscape : un processus Python par script, démarré avec une liste d'arguments fixe.
//!
//! Pourquoi ce module existe
//! Inkscape ne garde aucune session de script à rejoindre : chaque script s'exécute donc dans un
//! processus à lui. Ce processus ne doit hériter ni de l'environnement ni du jeton du Hub, ne doit
//! pas passer par un interpréteur de commandes, et doit finir avec tous ses enfants quand son temps est écoulé.

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

/// L'interpréteur qui exécute les scripts, sauf si le constructeur en reçoit un autre.
pub const DEFAULT_PYTHON: &str = "/usr/bin/python3";
/// Où vit `inkex` sous Debian, posé sur `PYTHONPATH`.
pub const DEFAULT_EXTENSIONS: &str = "/usr/share/inkscape/extensions";
/// L'exécutable Inkscape, employé pour sa version et pour les captures.
pub const DEFAULT_INKSCAPE: &str = "/usr/bin/inkscape";
/// Le document de travail, dans le répertoire de travail.
pub const DOCUMENT_NAME: &str = "current.svg";
/// Le fichier où le script est écrit, dans le répertoire de travail.
pub const SCRIPT_NAME: &str = "script.py";
/// Les seules variables qu'un processus démarré hérite du Hub.
pub const INHERITED_VARIABLES: [&str; 4] = ["PATH", "HOME", "LANG", "LC_ALL"];

/// Posé à `1` dans chaque processus : empêche le BLAS de numpy de démarrer un fil par cœur.
pub const THREAD_VARIABLES: [&str; 2] = ["OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS"];
/// L'espace d'adressage d'un processus de script, par défaut.
pub const DEFAULT_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Les fichiers ouverts qu'un processus de script peut tenir.
pub const OPEN_FILES_LIMIT: u64 = 256;
/// Combien de temps une capture peut prendre.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);
/// Combien de temps `inkscape --version` peut prendre.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// Le préfixe de la ligne qu'un script affiche en dernier pour rendre une valeur.
pub const RESULT_PREFIX: &str = "LMM_RESULT: ";
/// Le préfixe de la ligne que l'amorce écrit en dernier sur le flux d'erreur.
pub const ERROR_PREFIX: &str = "LMM_ERROR: ";

const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Combien de temps les lecteurs de sortie peuvent prendre une fois le groupe de processus disparu.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);
const READ_CHUNK: usize = 64 * 1024;

/// Le document créé au premier usage : un SVG valide avec un calque vide.
pub const EMPTY_DOCUMENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" width="210mm" height="297mm" viewBox="0 0 210 297">
  <g inkscape:groupmode="layer" id="layer1" inkscape:label="Layer 1"/>
</svg>
"#;

/// Exécute un script : applique les limites, puis compile le script sous le nom
/// `<libremodular>` et signale une exception sur la dernière ligne du flux d'erreur.
/// Arguments : fichier du script, secondes de processeur, octets d'espace d'adressage, fichiers ouverts.
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

/// Lit le document de travail : applique les limites, refuse toute déclaration d'entité, ne
/// résout ni ne récupère jamais rien, et affiche l'état.
/// Arguments : document, secondes de processeur, octets d'espace d'adressage, fichiers ouverts.
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

/// Où le module trouve les programmes qu'il démarre. Fixé à la construction : ni un script ni la
/// configuration ne peuvent les changer.
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

/// Les opérations sur les fichiers dont le module a besoin. Le Hub les fournit, avec ses propres
/// contrôles : le module n'ouvre jamais un fichier lui-même.
pub trait WorkFiles: Send + Sync {
    /// Crée le document contenant `initial` quand il n'existe pas ; refuse tout sauf un fichier ordinaire
    /// privé de l'utilisateur.
    ///
    /// # Erreurs
    /// Pourquoi le document ne peut pas être utilisé.
    fn ensure_document(&self, path: &Path, initial: &[u8]) -> Result<(), String>;

    /// Écrit le script dans un fichier privé créé pour lui.
    ///
    /// # Erreurs
    /// Pourquoi le script ne peut pas être écrit.
    fn write_script(&self, path: &Path, text: &[u8]) -> Result<(), String>;
}

/// Un processus à démarrer, décrit complètement avant que rien ne s'exécute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
    /// Tout l'environnement du processus : rien d'autre n'est hérité.
    pub env: Vec<(OsString, OsString)>,
}

/// Ce qu'un processus terminé a laissé.
#[derive(Debug)]
pub struct Finished {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Pourquoi un processus n'a produit aucun résultat.
#[derive(Debug)]
pub enum RunError {
    /// Le programme n'a pas pu être démarré.
    Start(io::Error),
    /// Le processus n'a pas pu être attendu.
    Wait(io::Error),
    /// Le temps s'est écoulé ; tout le groupe de processus a été tué.
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

/// L'environnement de chaque processus que le module démarre : les variables héritées qui
/// appartiennent à [`INHERITED_VARIABLES`], puis celles que le module pose.
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
    // `inkex` importe numpy, dont la bibliothèque BLAS démarre un fil par cœur. Sous la limite d'espace
    // d'adressage, trente-deux fils qui réservent chacun leur pile ne finissent jamais de démarrer et
    // l'import se bloque jusqu'au délai.
    for name in THREAD_VARIABLES {
        env.push((name.into(), "1".into()));
    }
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

/// `python3 -c <amorce> <fichier du script> <limites>` : le texte du script n'est jamais sur la
/// ligne de commande.
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

/// `python3 -c <amorce d'état> <document> <limites>`.
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

/// `inkscape --version`, depuis le répertoire racine.
#[must_use]
pub fn version_command(paths: &InkscapePaths, env: Vec<(OsString, OsString)>) -> ProcessSpec {
    ProcessSpec {
        program: paths.inkscape.clone(),
        args: vec!["--version".into()],
        current_dir: PathBuf::from("/"),
        env,
    }
}

/// Lit un tube jusqu'au bout, en gardant au plus `limit` octets et un de plus : le reste est lu et
/// jeté, pour qu'un processus bavard ne bloque jamais sur un tube plein. L'octet en trop fait
/// dépasser la borne au point de sortie du Hub, qui coupe alors avec son marqueur et pose `truncated`.
fn drain(mut pipe: impl Read + Send + 'static, limit: usize) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = vec![0_u8; READ_CHUNK];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    let room = limit.saturating_add(1).saturating_sub(kept.len());
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
    // Un processus qui a quitté le groupe peut encore tenir le tube : sa sortie n'est pas attendue.
    reader
        .and_then(|receiver| receiver.recv_timeout(OUTPUT_GRACE).ok())
        .unwrap_or_default()
}

/// Démarre `spec` dans un nouveau groupe de processus, sans interpréteur de commandes et avec
/// exactement son environnement, et attend au plus `timeout`. Quoi qu'il arrive, tout processus
/// encore dans le groupe est tué avant que l'appel ne rende.
///
/// # Erreurs
/// [`RunError`] quand le programme ne peut pas démarrer ou que le temps s'écoule.
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
        // NOWAIT laisse le chef non récolté : l'identifiant de son groupe de processus ne peut pas être
        // donné à un autre processus avant que le groupe soit tué plus bas.
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

/// Comment un processus s'est terminé, sans inventer de trace d'appels.
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

/// Détache la dernière ligne de `text` quand elle commence par `prefix` : le texte sans cette ligne,
/// et ce qui suit le préfixe.
fn take_last_line<'a>(text: &'a str, prefix: &str) -> Option<(&'a str, &'a str)> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let (before, last) = match body.rfind('\n') {
        Some(at) => (body.get(..=at)?, body.get(at + 1..)?),
        None => ("", body),
    };
    last.strip_prefix(prefix).map(|rest| (before, rest))
}

/// `stdout` sans sa ligne `LMM_RESULT`, et la valeur qu'elle portait.
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

/// `stderr` sans la ligne `LMM_ERROR` de l'amorce (et le saut de ligne que l'amorce a mis avant
/// elle), et l'erreur qu'elle portait.
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

/// La réponse à un script, d'après ce que son processus a laissé.
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

/// La version qu'`inkscape --version` a affichée, par exemple `1.2.2`.
fn parse_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next() == Some("Inkscape"))
            .then(|| words.next().map(str::to_owned))
            .flatten()
    })
}

/// Le module Inkscape : aucun processus persistant, un processus par requête.
pub struct InkscapeModule {
    paths: InkscapePaths,
    limits: Limits,
    memory_bytes: u64,
    inherited: Vec<(OsString, OsString)>,
    files: Arc<dyn WorkFiles>,
    busy: Mutex<()>,
}

impl InkscapeModule {
    /// `inherited` contient les valeurs du Hub pour [`INHERITED_VARIABLES`] ; tout le reste y est
    /// ignoré. `files` effectue les opérations sur les fichiers.
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
        }
    }

    /// Le même module avec une autre limite d'espace d'adressage pour ses processus.
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

/// Dit si quelque chose, un lien compris, porte le nom du document de travail. Un simple `stat` :
/// ce qui s'y trouve est contrôlé par les opérations sur les fichiers du Hub.
fn document_exists(path: &Path) -> bool {
    !matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound
    )
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
        // Le script ne reste sur le disque que pendant son exécution.
        let _ = std::fs::remove_file(&script_file);
        outcome
            .map(|finished| script_reply(&finished))
            .map_err(run_error)
    }

    fn state(&self, capture_path: &Path, work_dir: &Path) -> Result<StateReply, ModuleError> {
        let _running = match self.busy.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Err(ModuleError::Busy),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        };
        let app_version = self.version();
        // Regarder n'est pas créer : le document est fait par le premier script, et un document absent
        // est signalé comme tel plutôt qu'inventé.
        if !document_exists(&work_dir.join(DOCUMENT_NAME)) {
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
        }
        let mut warnings = Vec::new();
        let (document, state) = self.read_state(work_dir, &mut warnings)?;
        let screenshot = self.capture(work_dir, &document, capture_path, &mut warnings);
        Ok(StateReply {
            state,
            screenshot,
            warnings,
            app_version,
            bridge_version: None,
        })
    }

    /// Aucun processus ne survit à une requête : il n'y a rien à libérer, et un Inkscape que
    /// l'opérateur a ouvert n'est jamais touché.
    fn close(&self) {}
}
