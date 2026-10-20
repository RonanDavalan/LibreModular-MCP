//! Le lanceur de LibreOffice et de son pont : `libremodular-launch libreoffice`.
//!
//! Pourquoi ce module existe
//! LibreOffice n'écoute UNO que s'il a été démarré avec une option `--accept`, et son pont est un
//! processus distinct qui doit trouver le jeton et les ports. Le faire à la main invite à mettre un
//! jeton sur une ligne de commande et à se rattacher à une instance qui contient du travail non
//! enregistré. Ici les listes d'arguments sont fixées par le code, le jeton voyage comme le chemin
//! de son fichier et jamais comme son contenu, et rien n'est démarré à côté d'un LibreOffice que
//! l'opérateur fait déjà tourner.

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

/// L'exécutable LibreOffice, sauf si l'appelant en donne un autre.
pub const DEFAULT_SOFFICE: &str = "/usr/bin/soffice";
/// L'interpréteur qui exécute le pont : celui du système, qui a `uno`.
pub const DEFAULT_PYTHON: &str = "/usr/bin/python3";
/// Où sont listés les processus du système.
pub const DEFAULT_PROC: &str = "/proc";
/// Posé sur `LD_LIBRARY_PATH` pour que `import uno` trouve les bibliothèques de LibreOffice.
pub const LIBREOFFICE_LIBRARIES: &str = "/usr/lib/libreoffice/program";
/// Posé sur `PYTHONPATH` pour que l'interpréteur du système trouve `uno`.
pub const PYTHON_PACKAGES: &str = "/usr/lib/python3/dist-packages";
/// Les seules variables que le pont hérite du lanceur.
pub const BRIDGE_INHERITED: [&str; 4] = ["PATH", "HOME", "LANG", "LC_ALL"];
/// Les variables propres à ce programme : aucune application n'en a l'usage, donc aucune n'est
/// remise à LibreOffice.
pub const OWN_VARIABLES_PREFIX: &str = "LIBREMODULAR_";
/// Le nom du processus d'un LibreOffice en cours d'exécution.
pub const SOFFICE_PROCESS: &str = "soffice.bin";
/// Le répertoire, sous le volatil, qui contient ce qu'un lancement laisse.
pub const LAUNCH_DIR: &str = "launch";
/// Le fichier qui enregistre les processus du dernier lancement.
pub const RECORD_FILE: &str = "libreoffice.pids";
/// Les applications dont l'opérateur démarre le pont à la main.
pub const START_BY_HAND: [&str; 4] = ["gimp", "krita", "blender", "inkscape"];
/// Code de sortie d'un lancement qui a échoué en faisant son travail.
pub const EXIT_FAILED: u8 = 1;
/// Code de sortie d'un refus : un mauvais usage, ou quelque chose que l'opérateur doit changer.
pub const EXIT_REFUSED: u8 = 2;

const TARGET_PREFIX: &str = "libreoffice-";
const RECORD_LIMIT: u64 = 4096;
const PROC_FILE_LIMIT: u64 = 64 * 1024;

const USAGE: &str = "usage:\n  \
    libremodular-launch libreoffice --bridge-file <absolute path of uno_bridge.py>\n  \
    libremodular-launch libreoffice --stop\n\
    The bridges of the other applications are started by hand: see docs/modules/<name>.md\n";

/// Où sont les programmes, et où sont listés les processus du système. Fixé par l'appelant : rien
/// de ce qu'un script ou le modèle dit ne peut les changer.
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

/// Ce dont un lancement a besoin, tout décidé avant que rien ne démarre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub uno_port: u16,
    pub bridge_port: u16,
    pub bridge_file: PathBuf,
    /// Le fichier qui contient le jeton. Le jeton lui-même n'en sort jamais.
    pub token_file: PathBuf,
}

/// Un processus à démarrer, décrit complètement avant que rien ne s'exécute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
    /// Tout l'environnement du processus : rien d'autre n'est hérité.
    pub env: Vec<(OsString, OsString)>,
}

/// Pourquoi un lancement ou un arrêt n'a pas eu lieu.
#[derive(Debug)]
pub enum LaunchError {
    /// Aucune cible de LibreOffice n'est activée dans le mode `uno`.
    NoTarget,
    /// Des cibles activées nomment des valeurs différentes pour une même clé.
    PortsDisagree {
        key: &'static str,
        values: Vec<(&'static str, Option<u16>)>,
    },
    /// Les deux ports sont les mêmes.
    PortsClash { port: u16 },
    /// Le fichier du pont ne peut pas être cru.
    BridgeFile(SecurityError),
    /// Une instance de LibreOffice de l'utilisateur tourne déjà.
    AlreadyRunning { pids: Vec<u32>, uno_port: u16 },
    /// Un pont démarré par un lancement antérieur tourne encore.
    BridgeStillRunning { pid: u32 },
    /// La configuration ne peut pas être lue ou n'est pas utilisable.
    Config(ConfigError),
    /// Un fichier ou un répertoire du lanceur ne peut pas être utilisé.
    Files(SecurityError),
    /// L'enregistrement d'un lancement antérieur n'a pas de sens.
    MalformedRecord(PathBuf),
    /// Les processus du système ne peuvent pas être listés.
    Processes(io::Error),
    /// Un programme n'a pas pu être démarré.
    Start { program: PathBuf, source: io::Error },
    /// Le pont n'a pas démarré après LibreOffice.
    BridgeNotStarted {
        soffice_pid: u32,
        source: Box<LaunchError>,
    },
    /// Un signal n'a pas pu être envoyé.
    Signal { pid: u32, source: io::Error },
    /// Le rapport n'a pas pu être écrit.
    Output(io::Error),
}

impl LaunchError {
    /// Le code de sortie qui va avec l'erreur.
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
                "the bridge file is refused: {error}; pass the absolute path of uno_bridge.py \
                 with --bridge-file: bridges/generated/uno_bridge.py in a clone, \
                 /usr/share/libremodular-mcp/bridges/uno_bridge.py from the Debian package"
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
// Les commandes : pures, rien ne démarre ici
// --------------------------------------------------------------------------

/// L'option qui fait écouter UNO à LibreOffice, sur l'adresse de bouclage seulement.
#[must_use]
pub fn accept_argument(uno_port: u16) -> OsString {
    OsString::from(format!(
        "--accept=socket,host={BRIDGE_ADDRESS},port={uno_port};urp;"
    ))
}

/// LibreOffice avec sa fenêtre. Une fenêtre a besoin de l'affichage, du bus de session et des
/// réglages de l'utilisateur, et l'opérateur lui donnerait la même chose en le démarrant depuis
/// un terminal : il reçoit l'environnement du lanceur, hors les variables propres à ce programme
/// ([`OWN_VARIABLES_PREFIX`]).
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

/// L'environnement du pont : quatre variables héritées, les deux qui laissent l'interpréteur du
/// système trouver `uno`, les deux ports et le chemin du fichier du jeton.
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

/// `python3 <fichier du pont>`.
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
// Le plan, depuis la configuration
// --------------------------------------------------------------------------

fn libreoffice_targets(config: &Config) -> Vec<&AppSettings> {
    config
        .apps()
        .iter()
        .filter(|app| app.enabled && app.mode == Mode::Uno && app.name.starts_with(TARGET_PREFIX))
        .collect()
}

/// L'unique valeur qu'une cible activée nomme pour `key`, ou le désaccord.
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

/// Les ports des cibles activées de LibreOffice, qui doivent tous concorder.
///
/// # Erreurs
/// Aucune cible activée, des ports en désaccord ou manquants, ou les deux ports égaux.
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
// Les processus du système
// --------------------------------------------------------------------------

fn current_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Les processus de l'utilisateur courant qui sont un LibreOffice en cours d'exécution, lus dans
/// le listage de `proc_dir` sans démarrer aucune commande.
///
/// # Erreurs
/// [`LaunchError::Processes`] quand le listage ne peut pas être lu : le lancement ne continue
/// pas sans le savoir.
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

/// Dit si `pid` est un processus démarré comme `python3 <bridge_file>` : l'un de ses arguments
/// est exactement le fichier du pont. Un processus disparu, un que l'utilisateur ne peut pas lire,
/// ou tout autre programme ne l'est pas.
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
// L'enregistrement d'un lancement
// --------------------------------------------------------------------------

/// Les processus qu'un lancement a démarrés.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub soffice_pid: Option<u32>,
    pub bridge: Option<(u32, PathBuf)>,
}

impl Record {
    /// Une ligne par processus : `soffice <pid>` et `bridge <pid> <fichier du pont>`.
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

    /// Lit ce que [`Record::render`] a écrit ; `None` pour tout le reste.
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

/// L'enregistrement du dernier lancement, s'il y en a un.
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
// Démarrer et arrêter
// --------------------------------------------------------------------------

/// Démarre `spawn` dans un nouveau groupe de processus, sans interpréteur de commandes, avec
/// exactement son environnement et son entrée et sa sortie envoyées nulle part. Le processus
/// survit au lanceur : le descripteur est abandonné sans l'attendre.
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

/// Démarre LibreOffice et son pont, puis enregistre leurs processus.
///
/// # Erreurs
/// Un refus avant que rien ne démarre (voir [`LaunchError::exit_code`]), ou un échec au démarrage.
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

/// Arrête le pont du dernier lancement, et lui seul.
///
/// Le processus enregistré n'est signalé que s'il est toujours le pont : un numéro qu'un autre
/// programme a pris depuis n'est jamais touché. LibreOffice n'est jamais fermé : il peut contenir
/// du travail, et le fermer est le geste de l'opérateur.
///
/// # Erreurs
/// Un enregistrement illisible ou mal formé, ou un signal qui ne peut pas être envoyé.
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
// La ligne de commande
// --------------------------------------------------------------------------

/// Ce que les arguments demandent.
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

/// Exécute la ligne de commande de `libremodular-launch` et rend son code de sortie.
///
/// `locate` fournit les répertoires de base, `programs` les exécutables et le listage des
/// processus, `environment` ce que le lanceur a reçu : le binaire passe les vrais, et les tests
/// des faux.
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

/// Le lanceur sur les vrais répertoires, programmes et environnement : ce que le binaire exécute.
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
