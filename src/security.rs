//! Primitives de sécurité du côté du Hub : jeton, fichiers privés, chemins, garde-fou de script, bornes.
//!
//! Pourquoi ce module existe
//! Un script s'exécute avec les droits de l'utilisateur dans une application, et tout ce que le
//! Hub rend quitte la machine. Garder chaque contrôle dans un seul module permet à un relecteur
//! de lire toute la surface de sécurité d'un coup, et empêche tout autre module de laisser
//! pousser en douce une copie plus faible d'un contrôle.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ffi::OsString;
use std::fmt;
#[cfg(unix)]
use std::fs::DirBuilder;
use std::fs::{self, File};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ruff_python_ast::token::TokenKind;
use ruff_python_ast::visitor::source_order::{SourceOrderVisitor, TraversalSignal};
use ruff_python_ast::{
    AnyNodeRef, ConversionFlag, Expr, ExprContext, InterpolatedStringElement, Number, Operator,
};
use ruff_python_parser::{Mode, lexer::lex, parse_module};
use ruff_text_size::{Ranged, TextSize};
#[cfg(unix)]
use rustix::fs::{Mode as FileMode, OFlags};
#[cfg(unix)]
use rustix::io::Errno;
#[cfg(unix)]
use rustix::process::{Resource, Rlimit};

#[cfg(windows)]
use crate::windows_api;

/// Marqueur qui termine toute chaîne coupée pour tenir dans une borne de taille.
pub const TRUNCATION_MARKER: &str = "[truncated]";
/// Nombre de caractères hexadécimaux du jeton du pont.
pub const TOKEN_HEX_LENGTH: usize = 64;

const TOKEN_READ_CAP: u64 = 4096;
const MAX_SYMLINK_HOPS: usize = 40;
const PRIVATE_FILE_MODE: u32 = 0o600;
const PRIVATE_DIR_MODE: u32 = 0o700;
#[cfg(unix)]
const GROUP_AND_OTHER_BITS: u32 = 0o077;
#[cfg(unix)]
const GROUP_AND_OTHER_WRITE_BITS: u32 = 0o022;

// --------------------------------------------------------------------------
// Erreurs
// --------------------------------------------------------------------------

/// Une condition de sécurité a échoué ou n'a pas pu être vérifiée.
///
/// Aucune variante ne porte jamais le contenu d'un fichier ni le jeton : seulement des chemins,
/// des modes et des tailles, pour qu'un message puisse être montré à l'opérateur sans fuite.
#[derive(Debug)]
pub enum SecurityError {
    Hardening(&'static str),
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    NotFound {
        path: PathBuf,
    },
    AlreadyExists {
        path: PathBuf,
    },
    SymlinkRefused {
        path: PathBuf,
    },
    NotRegularFile {
        path: PathBuf,
    },
    NotPlainDirectory {
        path: PathBuf,
    },
    ForeignOwner {
        path: PathBuf,
    },
    LooseMode {
        path: PathBuf,
        mode: u32,
        expected: u32,
    },
    WritableByOthers {
        path: PathBuf,
        mode: u32,
    },
    LooseAcl {
        path: PathBuf,
    },
    TooLarge {
        path: PathBuf,
        limit: u64,
    },
    TokenFormat,
    TokenEmpty,
    TokenExists {
        path: PathBuf,
    },
    RandomUnavailable,
    PathEscapes {
        root: PathBuf,
        candidate: PathBuf,
    },
    InvalidPath {
        path: PathBuf,
    },
    UnknownTarget {
        active: Vec<String>,
    },
    ScriptTooLarge {
        size: usize,
        limit: usize,
    },
    LimitTooSmall {
        limit: usize,
    },
}

impl SecurityError {
    /// Dit si l'erreur signifie que le fichier n'existe pas (par opposition à être peu sûr).
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, SecurityError::NotFound { .. })
    }
}

impl fmt::Display for SecurityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecurityError::Hardening(what) => write!(f, "{what}"),
            SecurityError::Io {
                action,
                path,
                source,
            } => write!(f, "cannot {action} {}: {source}", path.display()),
            SecurityError::NotFound { path } => write!(f, "{} does not exist", path.display()),
            SecurityError::AlreadyExists { path } => {
                write!(f, "{} already exists", path.display())
            }
            SecurityError::SymlinkRefused { path } => {
                write!(f, "refusing to follow a symbolic link: {}", path.display())
            }
            SecurityError::NotRegularFile { path } => {
                write!(f, "not a regular file: {}", path.display())
            }
            SecurityError::NotPlainDirectory { path } => {
                write!(f, "not a plain directory: {}", path.display())
            }
            SecurityError::ForeignOwner { path } => {
                write!(f, "{} is not owned by the current user", path.display())
            }
            SecurityError::LooseMode {
                path,
                mode,
                expected,
            } => write!(
                f,
                "{} has mode {mode:04o}; expected {expected:04o} (chmod {expected:o})",
                path.display()
            ),
            SecurityError::WritableByOthers { path, mode } => write!(
                f,
                "{} has mode {mode:04o} and can be changed by the group or by others \
                 (chmod go-w)",
                path.display()
            ),
            SecurityError::LooseAcl { path } => write!(
                f,
                "{} can be used by an account other than yours (icacls <path> /inheritance:r /grant:r %USERNAME%:F)",
                path.display()
            ),
            SecurityError::TooLarge { path, limit } => {
                write!(f, "{} is larger than {limit} bytes", path.display())
            }
            SecurityError::TokenFormat => write!(
                f,
                "the token file must hold {TOKEN_HEX_LENGTH} hexadecimal characters; \
                 run 'libremodular-mcp init --rotate'"
            ),
            SecurityError::TokenEmpty => write!(f, "output cannot be finalized without the token"),
            SecurityError::TokenExists { path } => write!(
                f,
                "{} already exists; use rotate to replace the token",
                path.display()
            ),
            SecurityError::RandomUnavailable => {
                write!(f, "the system random source is unavailable")
            }
            SecurityError::PathEscapes { root, candidate } => write!(
                f,
                "path escapes {}: {}",
                root.display(),
                candidate.display()
            ),
            SecurityError::InvalidPath { path } => write!(f, "invalid path: {}", path.display()),
            SecurityError::UnknownTarget { active } => {
                let listing = if active.is_empty() {
                    "none".to_owned()
                } else {
                    active.join(", ")
                };
                write!(
                    f,
                    "unknown or inactive target_app; active applications: {listing}"
                )
            }
            SecurityError::ScriptTooLarge { size, limit } => {
                write!(f, "script is {size} bytes; the limit is {limit} bytes")
            }
            SecurityError::LimitTooSmall { limit } => write!(
                f,
                "a limit of {limit} bytes is smaller than the truncation marker"
            ),
        }
    }
}

impl std::error::Error for SecurityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SecurityError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> SecurityError {
    SecurityError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

// --------------------------------------------------------------------------
// Durcissement du processus
// --------------------------------------------------------------------------

/// Désactive les vidages mémoire et restreint le masque de création de fichiers.
///
/// Doit s'exécuter avant la lecture du jeton, pour qu'un plantage ne puisse pas l'écrire sur disque.
///
/// # Erreurs
/// Échoue fermé quand la limite des vidages mémoire ne peut pas être abaissée.
#[cfg(unix)]
pub fn harden_process() -> Result<(), SecurityError> {
    rustix::process::setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )
    .map_err(|_| SecurityError::Hardening("cannot disable core dumps"))?;
    rustix::process::umask(FileMode::from_raw_mode(GROUP_AND_OTHER_BITS));
    Ok(())
}

/// Sous Windows, ni vidage ni rapport d'erreur interactif ne naît du processus.
///
/// # Erreurs
/// Aucune : l'appel ne peut pas échouer.
#[cfg(windows)]
pub fn harden_process() -> Result<(), SecurityError> {
    windows_api::harden();
    Ok(())
}

// --------------------------------------------------------------------------
// Fichiers et répertoires
// --------------------------------------------------------------------------

#[cfg(unix)]
fn current_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Sous Windows un propriétaire n'est pas un nombre : `1` désigne l'utilisateur courant, `0` `SYSTEM` et les
/// administrateurs, tout autre compte n'est jamais égal à l'un ni à l'autre.
#[cfg(windows)]
fn current_uid() -> u32 {
    CURRENT_OWNER
}

#[cfg(windows)]
const CURRENT_OWNER: u32 = 1;
#[cfg(windows)]
const OTHER_OWNER: u32 = u32::MAX;

#[cfg(unix)]
fn owner_id(_file: &File, info: &fs::Metadata) -> io::Result<u32> {
    Ok(info.uid())
}

#[cfg(windows)]
fn owner_id(file: &File, _info: &fs::Metadata) -> io::Result<u32> {
    Ok(match windows_api::owner_of(file)? {
        windows_api::Owner::CurrentUser => CURRENT_OWNER,
        windows_api::Owner::Administrators => ROOT_UID,
        windows_api::Owner::Other => OTHER_OWNER,
    })
}

/// Refuse un fichier ou un répertoire ouvert que le groupe ou les autres peuvent employer.
#[cfg(unix)]
fn check_private(
    _file: &File,
    info: &fs::Metadata,
    path: &Path,
    expected_mode: u32,
) -> Result<(), SecurityError> {
    if info.mode() & GROUP_AND_OTHER_BITS != 0 {
        return Err(SecurityError::LooseMode {
            path: path.to_path_buf(),
            mode: info.mode() & 0o7777,
            expected: expected_mode,
        });
    }
    Ok(())
}

/// Refuse un fichier ou un répertoire ouvert dont la liste de contrôle d'accès sert un autre compte.
#[cfg(windows)]
fn check_private(
    file: &File,
    _info: &fs::Metadata,
    path: &Path,
    _expected_mode: u32,
) -> Result<(), SecurityError> {
    let shared = windows_api::grants_to_others(file, windows_api::ANY_RIGHT)
        .map_err(|e| io_error("inspect", path, e))?;
    if shared {
        return Err(SecurityError::LooseAcl {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Refuse un fichier de confiance qu'un autre que son propriétaire peut modifier.
#[cfg(unix)]
fn check_not_writable_by_others(
    _file: &File,
    info: &fs::Metadata,
    path: &Path,
) -> Result<(), SecurityError> {
    if info.mode() & GROUP_AND_OTHER_WRITE_BITS != 0 {
        return Err(SecurityError::WritableByOthers {
            path: path.to_path_buf(),
            mode: info.mode() & 0o7777,
        });
    }
    Ok(())
}

#[cfg(windows)]
fn check_not_writable_by_others(
    file: &File,
    _info: &fs::Metadata,
    path: &Path,
) -> Result<(), SecurityError> {
    let writable = windows_api::grants_to_others(file, windows_api::MODIFY_RIGHTS)
        .map_err(|e| io_error("inspect", path, e))?;
    if writable {
        return Err(SecurityError::LooseAcl {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Comment un fichier de contrôle est ouvert. La troncature n'est pas une option : un fichier de
/// contrôle n'est jamais que lu, complété ou créé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAccess {
    Read,
    AppendOrCreate,
    CreateNew,
}

impl ControlAccess {
    #[cfg(unix)]
    fn flags(self) -> OFlags {
        match self {
            ControlAccess::Read => OFlags::RDONLY,
            ControlAccess::AppendOrCreate => OFlags::WRONLY | OFlags::APPEND | OFlags::CREATE,
            ControlAccess::CreateNew => OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
        }
    }
}

/// Ouvre sans suivre un lien symbolique final ; refuse tout sauf un fichier ordinaire.
///
/// # Erreurs
/// Refuse les liens, les fichiers non ordinaires et tout échec de l'ouverture elle-même.
#[cfg(unix)]
pub fn open_regular_nofollow(path: &Path, access: ControlAccess) -> Result<File, SecurityError> {
    let flags = access.flags() | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let fd =
        rustix::fs::open(path, flags, FileMode::RUSR | FileMode::WUSR).map_err(
            |errno| match errno {
                Errno::LOOP => SecurityError::SymlinkRefused {
                    path: path.to_path_buf(),
                },
                Errno::NOENT => SecurityError::NotFound {
                    path: path.to_path_buf(),
                },
                Errno::EXIST => SecurityError::AlreadyExists {
                    path: path.to_path_buf(),
                },
                other => io_error("open", path, io::Error::from(other)),
            },
        )?;
    let file = File::from(fd);
    let info = file.metadata().map_err(|e| io_error("inspect", path, e))?;
    if !info.file_type().is_file() {
        return Err(SecurityError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    Ok(file)
}

/// Ouvre sans suivre un point d'analyse final (lien symbolique ou jonction) ; refuse tout sauf un fichier
/// ordinaire. Un fichier créé naît avec une liste de contrôle d'accès privée à l'utilisateur.
///
/// # Erreurs
/// Refuse les liens, les fichiers non ordinaires et tout échec de l'ouverture elle-même.
#[cfg(windows)]
pub fn open_regular_nofollow(path: &Path, access: ControlAccess) -> Result<File, SecurityError> {
    let kind = match access {
        ControlAccess::Read => windows_api::Access::Read,
        ControlAccess::AppendOrCreate => windows_api::Access::AppendOrCreate,
        ControlAccess::CreateNew => windows_api::Access::CreateNew,
    };
    let file = windows_api::open_file(path, kind).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => SecurityError::NotFound {
            path: path.to_path_buf(),
        },
        io::ErrorKind::AlreadyExists => SecurityError::AlreadyExists {
            path: path.to_path_buf(),
        },
        _ => io_error("open", path, error),
    })?;
    let info = file.metadata().map_err(|e| io_error("inspect", path, e))?;
    if is_link_like(&info) {
        return Err(SecurityError::SymlinkRefused {
            path: path.to_path_buf(),
        });
    }
    if !info.file_type().is_file() {
        return Err(SecurityError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    Ok(file)
}

/// Dit si l'entrée est un point d'analyse : lien symbolique ou jonction.
#[cfg(windows)]
fn is_link_like(info: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as WindowsMetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    info.file_attributes() & REPARSE_POINT != 0
}

/// Ouvre un jeton, un journal ou un fichier de capture après contrôle du propriétaire et des permissions.
///
/// Le fichier doit appartenir à l'utilisateur courant et ne porter aucun bit de groupe ni des autres.
///
/// # Erreurs
/// Tout échec de [`open_regular_nofollow`], un propriétaire étranger, ou un mode trop ouvert.
pub fn open_control_file(path: &Path, access: ControlAccess) -> Result<File, SecurityError> {
    open_control_file_as(path, access, current_uid())
}

/// Comme [`open_control_file`] avec un propriétaire attendu explicite.
///
/// # Erreurs
/// Voir [`open_control_file`].
pub fn open_control_file_as(
    path: &Path,
    access: ControlAccess,
    expected_uid: u32,
) -> Result<File, SecurityError> {
    let file = open_regular_nofollow(path, access)?;
    let info = file.metadata().map_err(|e| io_error("inspect", path, e))?;
    if owner_id(&file, &info).map_err(|e| io_error("inspect", path, e))? != expected_uid {
        return Err(SecurityError::ForeignOwner {
            path: path.to_path_buf(),
        });
    }
    check_private(&file, &info, path, PRIVATE_FILE_MODE)?;
    Ok(file)
}

/// Crée un fichier privé contenant `initial` quand rien ne porte encore ce nom ; quand quelque chose
/// le porte, ce doit être un fichier ordinaire privé de l'utilisateur courant. Un lien symbolique
/// est refusé, jamais suivi.
///
/// # Erreurs
/// Un lien, un fichier non ordinaire, un propriétaire étranger, un mode trop ouvert, ou un échec d'E/S.
pub fn ensure_private_file(path: &Path, initial: &[u8]) -> Result<(), SecurityError> {
    match open_control_file(path, ControlAccess::CreateNew) {
        Ok(mut file) => file
            .write_all(initial)
            .map_err(|e| io_error("write", path, e)),
        Err(SecurityError::AlreadyExists { .. }) => {
            open_control_file(path, ControlAccess::Read).map(drop)
        }
        Err(error) => Err(error),
    }
}

/// Écrit `bytes` dans un fichier privé créé pour l'occasion : ce qui portait ce nom est d'abord
/// retiré (un lien lui-même, jamais sa cible), puis le fichier est créé en exclusivité en mode 600,
/// de sorte qu'un lien posé entre-temps est refusé.
///
/// # Erreurs
/// Un répertoire de ce nom, ou tout échec de suppression, de création ou d'écriture.
pub fn replace_private_file(path: &Path, bytes: &[u8]) -> Result<(), SecurityError> {
    match fs::symlink_metadata(path) {
        Ok(info) if info.is_dir() => {
            return Err(SecurityError::NotRegularFile {
                path: path.to_path_buf(),
            });
        }
        Ok(_) => fs::remove_file(path).map_err(|e| io_error("remove", path, e))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error("inspect", path, e)),
    }
    let mut file = open_control_file(path, ControlAccess::CreateNew)?;
    file.write_all(bytes)
        .map_err(|e| io_error("write", path, e))
}

/// Lit un fichier de contrôle (jeton, configuration) en une seule lecture bornée.
///
/// L'unique point d'entrée de tout fichier auquel le Hub fait confiance pour piloter son
/// comportement : fichier ordinaire, sans lien symbolique, appartenant à l'utilisateur courant,
/// sans bit de groupe ni des autres, d'au plus `max_bytes`.
///
/// # Erreurs
/// [`SecurityError::NotFound`] quand le fichier est absent ; toute autre variante signifie que
/// le fichier existe mais ne peut pas être cru.
pub fn read_control_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, SecurityError> {
    read_control_file_as(path, max_bytes, current_uid())
}

/// Comme [`read_control_file`] avec un propriétaire attendu explicite.
///
/// # Erreurs
/// Voir [`read_control_file`].
pub fn read_control_file_as(
    path: &Path,
    max_bytes: u64,
    expected_uid: u32,
) -> Result<Vec<u8>, SecurityError> {
    let file = open_control_file_as(path, ControlAccess::Read, expected_uid)?;
    let mut data = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| io_error("read", path, e))?;
    if u64::try_from(data.len()).map_or(true, |len| len > max_bytes) {
        return Err(SecurityError::TooLarge {
            path: path.to_path_buf(),
            limit: max_bytes,
        });
    }
    Ok(data)
}

/// Le propriétaire des fichiers qu'un gestionnaire de paquets installe.
const ROOT_UID: u32 = 0;

/// Contrôle un fichier que le Hub s'apprête à confier à un autre programme pour exécution : un
/// chemin absolu vers un fichier ordinaire, pas un lien, appartenant à l'utilisateur courant ou à
/// `root` et non inscriptible par le groupe ni par les autres. Son contenu n'est pas lu.
///
/// `root` est admis parce que le pont installé par le paquet lui appartient : le fichier d'un
/// autre utilisateur reste refusé, et un fichier de `root` que ni le groupe ni les autres ne
/// peuvent écrire n'est remplaçable que par `root`.
///
/// Sa lecture reste permise à tous : un tel fichier est du code, pas un secret.
///
/// # Erreurs
/// Un chemin relatif, un lien, un fichier non ordinaire, un propriétaire étranger, ou un mode qui
/// permet au groupe ou aux autres de modifier le fichier.
pub fn check_trusted_file(path: &Path) -> Result<(), SecurityError> {
    check_trusted_file_among(path, &[current_uid(), ROOT_UID])
}

/// Comme [`check_trusted_file`] avec un propriétaire attendu explicite.
///
/// # Erreurs
/// Voir [`check_trusted_file`].
pub fn check_trusted_file_as(path: &Path, expected_uid: u32) -> Result<(), SecurityError> {
    check_trusted_file_among(path, &[expected_uid])
}

/// Comme [`check_trusted_file`] avec la liste des propriétaires admis.
///
/// # Erreurs
/// Voir [`check_trusted_file`].
pub fn check_trusted_file_among(path: &Path, owners: &[u32]) -> Result<(), SecurityError> {
    if !path.is_absolute() {
        return Err(SecurityError::InvalidPath {
            path: path.to_path_buf(),
        });
    }
    let file = open_regular_nofollow(path, ControlAccess::Read)?;
    let info = file.metadata().map_err(|e| io_error("inspect", path, e))?;
    if !owners.contains(&owner_id(&file, &info).map_err(|e| io_error("inspect", path, e))?) {
        return Err(SecurityError::ForeignOwner {
            path: path.to_path_buf(),
        });
    }
    check_not_writable_by_others(&file, &info, path)?;
    Ok(())
}

/// Lit un fichier que le système fournit, comme un fichier sous `/proc` : un fichier ordinaire qui
/// n'est pas un lien, lu en une seule lecture bornée. Contrairement à un fichier de contrôle, il
/// n'est pas à nous de le protéger : son propriétaire et son mode ne sont donc pas contrôlés.
///
/// # Erreurs
/// [`SecurityError::NotFound`] quand il a disparu (un processus terminé), un lien, un fichier non
/// ordinaire, un échec de lecture, ou plus de `max_bytes` de contenu.
pub fn read_system_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, SecurityError> {
    let file = open_regular_nofollow(path, ControlAccess::Read)?;
    let mut data = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| io_error("read", path, e))?;
    if u64::try_from(data.len()).map_or(true, |len| len > max_bytes) {
        return Err(SecurityError::TooLarge {
            path: path.to_path_buf(),
            limit: max_bytes,
        });
    }
    Ok(data)
}

/// Crée le répertoire en mode 700, ou vérifie qu'un répertoire existant est privé.
///
/// # Erreurs
/// Refuse les liens, les non-répertoires, les propriétaires étrangers et les permissions de groupe ou des autres.
pub fn ensure_private_dir(path: &Path) -> Result<(), SecurityError> {
    ensure_private_dir_as(path, current_uid())
}

/// Comme [`ensure_private_dir`] avec un propriétaire attendu explicite.
///
/// # Erreurs
/// Voir [`ensure_private_dir`].
#[cfg(unix)]
pub fn ensure_private_dir_as(path: &Path, expected_uid: u32) -> Result<(), SecurityError> {
    let info = match fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                DirBuilder::new()
                    .recursive(true)
                    .mode(PRIVATE_DIR_MODE)
                    .create(parent)
                    .map_err(|e| io_error("create directory", parent, e))?;
            }
            match DirBuilder::new().mode(PRIVATE_DIR_MODE).create(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_error("create directory", path, e)),
            }
            fs::symlink_metadata(path).map_err(|e| io_error("inspect", path, e))?
        }
        Err(e) => return Err(io_error("inspect", path, e)),
    };
    if info.file_type().is_symlink() || !info.file_type().is_dir() {
        return Err(SecurityError::NotPlainDirectory {
            path: path.to_path_buf(),
        });
    }
    if info.uid() != expected_uid {
        return Err(SecurityError::ForeignOwner {
            path: path.to_path_buf(),
        });
    }
    if info.mode() & GROUP_AND_OTHER_BITS != 0 {
        return Err(SecurityError::LooseMode {
            path: path.to_path_buf(),
            mode: info.mode() & 0o7777,
            expected: PRIVATE_DIR_MODE,
        });
    }
    Ok(())
}

/// Comme [`ensure_private_dir`] avec un propriétaire attendu explicite.
///
/// # Erreurs
/// Voir [`ensure_private_dir`].
#[cfg(windows)]
pub fn ensure_private_dir_as(path: &Path, expected_uid: u32) -> Result<(), SecurityError> {
    let info = match fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                create_private_dirs_under(parent)?;
            }
            match windows_api::create_private_dir(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_error("create directory", path, e)),
            }
            fs::symlink_metadata(path).map_err(|e| io_error("inspect", path, e))?
        }
        Err(e) => return Err(io_error("inspect", path, e)),
    };
    if is_link_like(&info) || !info.file_type().is_dir() {
        return Err(SecurityError::NotPlainDirectory {
            path: path.to_path_buf(),
        });
    }
    let handle = windows_api::open_directory(path).map_err(|e| io_error("inspect", path, e))?;
    if owner_id(&handle, &info).map_err(|e| io_error("inspect", path, e))? != expected_uid {
        return Err(SecurityError::ForeignOwner {
            path: path.to_path_buf(),
        });
    }
    check_private(&handle, &info, path, PRIVATE_DIR_MODE)
}

/// Crée les répertoires manquants de `path`, chacun avec une liste de contrôle d'accès privée.
#[cfg(windows)]
fn create_private_dirs_under(path: &Path) -> Result<(), SecurityError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        create_private_dirs_under(parent)?;
    }
    match windows_api::create_private_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(io_error("create directory", path, e)),
    }
}

// --------------------------------------------------------------------------
// Jeton
// --------------------------------------------------------------------------

/// Le secret partagé avec les ponts.
///
/// Il n'a pas de `Display` et sa sortie `Debug` masque la valeur, pour que journaliser une
/// structure qui le contient ne puisse pas le divulguer.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// Enveloppe un secret non vide. [`load_token`] est le seul moyen d'obtenir le jeton que les ponts
    /// attendent ; ce constructeur existe pour que des composants puissent être construits avec un
    /// secret quelconque.
    ///
    /// # Erreurs
    /// Une valeur vide correspondrait partout, elle est donc refusée.
    pub fn new(value: impl Into<String>) -> Result<Token, SecurityError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SecurityError::TokenEmpty);
        }
        Ok(Token(value))
    }

    /// Le secret lui-même. Les appelants ne doivent jamais le journaliser ni le rendre.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token([redacted])")
    }
}

fn is_valid_token_text(text: &[u8]) -> bool {
    text.len() == TOKEN_HEX_LENGTH && text.iter().all(u8::is_ascii_hexdigit)
}

/// Lit et valide le jeton du pont (64 caractères hexadécimaux).
///
/// # Erreurs
/// Tout fichier de jeton peu sûr ou mal formé. Le message ne contient jamais le jeton.
pub fn load_token(path: &Path) -> Result<Token, SecurityError> {
    load_token_as(path, current_uid())
}

/// Comme [`load_token`] avec un propriétaire attendu explicite.
///
/// # Erreurs
/// Voir [`load_token`].
pub fn load_token_as(path: &Path, expected_uid: u32) -> Result<Token, SecurityError> {
    harden_process()?;
    let mut data = read_control_file_as(path, TOKEN_READ_CAP, expected_uid)?;
    while data.last() == Some(&b'\n') {
        data.pop();
    }
    if !is_valid_token_text(&data) {
        return Err(SecurityError::TokenFormat);
    }
    let text = String::from_utf8(data).map_err(|_| SecurityError::TokenFormat)?;
    Token::new(text)
}

fn random_hex(byte_count: usize) -> Result<String, SecurityError> {
    let mut bytes = vec![0_u8; byte_count];
    getrandom::fill(&mut bytes).map_err(|_| SecurityError::RandomUnavailable)?;
    let mut text = String::with_capacity(byte_count.saturating_mul(2));
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    Ok(text)
}

/// Crée le fichier du jeton (en exclusivité, mode 600) dans un répertoire privé.
///
/// Refuse de remplacer un jeton existant sauf si `rotate` est vrai.
///
/// # Erreurs
/// Un jeton existant sans `rotate`, un répertoire peu sûr, ou un échec d'E/S.
pub fn create_token(path: &Path, rotate: bool) -> Result<(), SecurityError> {
    harden_process()?;
    let parent = path.parent().ok_or_else(|| SecurityError::InvalidPath {
        path: path.to_path_buf(),
    })?;
    ensure_private_dir(parent)?;
    let data = random_hex(TOKEN_HEX_LENGTH / 2)?;
    if !rotate {
        let mut file = match open_control_file(path, ControlAccess::CreateNew) {
            Err(SecurityError::AlreadyExists { path }) => {
                return Err(SecurityError::TokenExists { path });
            }
            other => other?,
        };
        file.write_all(data.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| io_error("write the token file", path, e))?;
        return Ok(());
    }
    let name =
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| SecurityError::InvalidPath {
                path: path.to_path_buf(),
            })?;
    let staging = parent.join(format!(".{name}.{}.new", random_hex(8)?));
    let mut file = open_control_file(&staging, ControlAccess::CreateNew)?;
    let outcome = file
        .write_all(data.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&staging, path));
    drop(file);
    if let Err(e) = outcome {
        // Au mieux : un fichier intermédiaire laissé derrière est privé et ne contient aucun jeton vivant.
        let _ = fs::remove_file(&staging);
        return Err(io_error("write the token file", path, e));
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Chemins, cibles, tailles
// --------------------------------------------------------------------------

/// La racine d'un chemin : `/` sous Unix, le lecteur ou le partage et le séparateur sous Windows.
#[cfg(unix)]
fn root_of(_path: &Path) -> PathBuf {
    PathBuf::from("/")
}

#[cfg(windows)]
fn root_of(path: &Path) -> PathBuf {
    path.components()
        .take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
        .collect()
}

fn realpath(path: &Path) -> Result<PathBuf, SecurityError> {
    let mut pending: VecDeque<OsString> = VecDeque::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => pending.push_back(name.to_os_string()),
            Component::ParentDir => pending.push_back(OsString::from("..")),
            _ => {}
        }
    }
    let mut resolved = root_of(path);
    let mut hops = 0_usize;
    while let Some(part) = pending.pop_front() {
        if part == ".." {
            resolved.pop();
            continue;
        }
        let candidate = resolved.join(&part);
        let is_link = fs::symlink_metadata(&candidate)
            .map(|info| info.file_type().is_symlink())
            .unwrap_or(false);
        if !is_link {
            resolved = candidate;
            continue;
        }
        hops += 1;
        if hops > MAX_SYMLINK_HOPS {
            return Err(SecurityError::InvalidPath {
                path: path.to_path_buf(),
            });
        }
        let target = fs::read_link(&candidate).map_err(|e| io_error("resolve", &candidate, e))?;
        if target.is_absolute() {
            resolved = root_of(&target);
        }
        for component in target.components().rev() {
            match component {
                Component::Normal(name) => pending.push_front(name.to_os_string()),
                Component::ParentDir => pending.push_front(OsString::from("..")),
                _ => {}
            }
        }
    }
    Ok(resolved)
}

/// Résout les liens symboliques et exige que le résultat reste dans `root`.
///
/// Les composants qui n'existent pas encore sont gardés tels quels, si bien que le contrôle
/// fonctionne aussi pour un fichier sur le point d'être créé.
///
/// # Erreurs
/// [`SecurityError::PathEscapes`] quand le chemin résolu sort de `root`.
pub fn resolve_under(root: &Path, candidate: &Path) -> Result<PathBuf, SecurityError> {
    if !root.is_absolute() {
        return Err(SecurityError::InvalidPath {
            path: root.to_path_buf(),
        });
    }
    let root_real = realpath(root)?;
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root_real.join(candidate)
    };
    let real = realpath(&joined)?;
    if real.starts_with(&root_real) {
        Ok(real)
    } else {
        Err(SecurityError::PathEscapes {
            root: root_real,
            candidate: candidate.to_path_buf(),
        })
    }
}

/// N'accepte qu'une correspondance exacte, sensible à la casse, avec le nom d'une application active.
///
/// # Erreurs
/// [`SecurityError::UnknownTarget`], qui liste les applications actives.
pub fn check_target<'a>(
    name: &str,
    active: impl IntoIterator<Item = &'a str>,
) -> Result<(), SecurityError> {
    let active: Vec<&str> = active.into_iter().collect();
    if active.contains(&name) {
        return Ok(());
    }
    let mut listing: Vec<String> = active.into_iter().map(str::to_owned).collect();
    listing.sort();
    Err(SecurityError::UnknownTarget { active: listing })
}

/// Rend la taille du script en octets, ou refuse quand elle dépasse la limite.
///
/// # Erreurs
/// [`SecurityError::ScriptTooLarge`].
pub fn check_script_size(script: &str, max_script_bytes: usize) -> Result<usize, SecurityError> {
    let size = script.len();
    if size > max_script_bytes {
        return Err(SecurityError::ScriptTooLarge {
            size,
            limit: max_script_bytes,
        });
    }
    Ok(size)
}

/// Coupe `text` à au plus `limit` octets UTF-8, terminé par le marqueur de troncature.
///
/// Un caractère n'est jamais coupé en deux. Rend le texte et dit s'il a été coupé.
///
/// # Erreurs
/// [`SecurityError::LimitTooSmall`] quand la limite ne peut même pas contenir le marqueur.
pub fn bounded(text: &str, limit: usize) -> Result<(String, bool), SecurityError> {
    if text.len() <= limit {
        return Ok((text.to_owned(), false));
    }
    let room = limit
        .checked_sub(TRUNCATION_MARKER.len())
        .ok_or(SecurityError::LimitTooSmall { limit })?;
    let mut end = room;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = text.get(..end).unwrap_or("");
    Ok((format!("{head}{TRUNCATION_MARKER}"), true))
}

// --------------------------------------------------------------------------
// Captures
// --------------------------------------------------------------------------

/// Une capture plus grande que cela est refusée et supprimée.
pub const MAX_CAPTURE_BYTES: u64 = 20 * 1024 * 1024;
/// Au plus ce nombre de captures est gardé ; les plus anciennes partent d'abord.
pub const MAX_CAPTURES: usize = 50;
/// Une capture plus ancienne que cela est supprimée.
pub const MAX_CAPTURE_AGE: Duration = Duration::from_secs(3600);

/// Le chemin où le Hub demande à un pont d'écrire une capture : `<app>-<timestamp>.png` directement
/// dans le répertoire des captures. `stamp` ne doit contenir que des caractères sûrs dans un nom de fichier.
///
/// # Erreurs
/// [`SecurityError::InvalidPath`] quand le nom ne resterait pas un composant unique.
pub fn capture_path_for(
    captures_dir: &Path,
    app: &str,
    stamp: &str,
) -> Result<PathBuf, SecurityError> {
    let name = format!("{app}-{stamp}.png");
    let safe = !app.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.');
    if !safe {
        return Err(SecurityError::InvalidPath {
            path: PathBuf::from(name),
        });
    }
    Ok(captures_dir.join(name))
}

/// Accepte une capture signalée par un pont : un fichier ordinaire de l'utilisateur courant, atteint
/// directement sans lien symbolique, dans le répertoire des captures une fois résolu, d'au plus
/// [`MAX_CAPTURE_BYTES`]. Son mode est ramené à 600.
///
/// # Erreurs
/// Tout le reste ; l'appelant l'écarte alors avec [`discard_capture`].
pub fn accept_capture(captures_dir: &Path, reported: &Path) -> Result<PathBuf, SecurityError> {
    if !reported.is_absolute() {
        return Err(SecurityError::InvalidPath {
            path: reported.to_path_buf(),
        });
    }
    let resolved = resolve_under(captures_dir, reported)?;
    let file = open_regular_nofollow(reported, ControlAccess::Read)?;
    let info = file
        .metadata()
        .map_err(|e| io_error("inspect", reported, e))?;
    if owner_id(&file, &info).map_err(|e| io_error("inspect", reported, e))? != current_uid() {
        return Err(SecurityError::ForeignOwner {
            path: reported.to_path_buf(),
        });
    }
    if info.len() > MAX_CAPTURE_BYTES {
        return Err(SecurityError::TooLarge {
            path: reported.to_path_buf(),
            limit: MAX_CAPTURE_BYTES,
        });
    }
    restrict_capture(&file, &info, reported)?;
    Ok(resolved)
}

/// Ramène une capture au mode 600.
#[cfg(unix)]
fn restrict_capture(
    file: &File,
    info: &fs::Metadata,
    reported: &Path,
) -> Result<(), SecurityError> {
    if info.mode() & GROUP_AND_OTHER_BITS != 0 {
        // Modifié par le descripteur ouvert, pour qu'un chemin échangé ne puisse pas le rediriger.
        file.set_permissions(fs::Permissions::from_mode(PRIVATE_FILE_MODE))
            .map_err(|e| io_error("restrict", reported, e))?;
    }
    Ok(())
}

/// Sous Windows une capture produite dans le répertoire privé en hérite la liste ; une autre est refusée.
#[cfg(windows)]
fn restrict_capture(
    file: &File,
    info: &fs::Metadata,
    reported: &Path,
) -> Result<(), SecurityError> {
    check_private(file, info, reported, PRIVATE_FILE_MODE)
}

/// Supprime une capture refusée, mais seulement quand c'est une entrée directe du répertoire des
/// captures : un pont ne doit jamais pouvoir faire supprimer au Hub un fichier ailleurs. Un lien
/// symbolique est supprimé lui-même, jamais sa cible.
///
/// Rend si quelque chose a été supprimé.
#[must_use]
pub fn discard_capture(captures_dir: &Path, reported: &Path) -> bool {
    let direct_child = reported.parent() == Some(captures_dir)
        && matches!(
            reported.components().next_back(),
            Some(Component::Normal(_))
        );
    if !direct_child {
        return false;
    }
    match fs::symlink_metadata(reported) {
        Ok(info) if !info.is_dir() => fs::remove_file(reported).is_ok(),
        _ => false,
    }
}

/// Supprime les captures plus anciennes que [`MAX_CAPTURE_AGE`] à l'instant `now`, puis les plus
/// anciennes au-delà de [`MAX_CAPTURES`]. Les répertoires qu'il contient sont laissés. Un répertoire
/// absent n'a rien à purger.
///
/// Rend le nombre d'entrées supprimées.
///
/// # Erreurs
/// Le répertoire existe mais n'est pas un répertoire privé de l'utilisateur courant, ou ne peut pas être listé.
pub fn purge_captures(captures_dir: &Path, now: SystemTime) -> Result<usize, SecurityError> {
    match fs::symlink_metadata(captures_dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io_error("inspect", captures_dir, e)),
        Ok(_) => ensure_private_dir(captures_dir)?,
    }
    let listing = fs::read_dir(captures_dir).map_err(|e| io_error("list", captures_dir, e))?;
    let mut kept: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut removed = 0_usize;
    for entry in listing {
        let entry = entry.map_err(|e| io_error("list", captures_dir, e))?;
        let path = entry.path();
        let Ok(info) = fs::symlink_metadata(&path) else {
            continue;
        };
        if info.is_dir() {
            continue;
        }
        let modified = info.modified().unwrap_or(UNIX_EPOCH);
        let too_old = now
            .duration_since(modified)
            .is_ok_and(|age| age > MAX_CAPTURE_AGE);
        if too_old {
            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        } else {
            kept.push((modified, path));
        }
    }
    kept.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in kept.iter().skip(MAX_CAPTURES) {
        if fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

// --------------------------------------------------------------------------
// Garde-fou de script
// --------------------------------------------------------------------------

/// La sorte de danger à laquelle appartient un constat. L'identifiant est une valeur du produit :
/// il apparaît dans les messages de refus et dans le journal d'audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    FileDeletion,
    ProcessLaunch,
    NetworkAccess,
    WriteOutsideWorkDir,
    AnalysisBypass,
}

impl Category {
    /// L'identifiant montré au modèle et écrit dans le journal d'audit.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Category::FileDeletion => "G-1",
            Category::ProcessLaunch => "G-2",
            Category::NetworkAccess => "G-3",
            Category::WriteOutsideWorkDir => "G-4",
            Category::AnalysisBypass => "G-5",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Category::FileDeletion => "file deletion",
            Category::ProcessLaunch => "process launch",
            Category::NetworkAccess => "network access",
            Category::WriteOutsideWorkDir => "write outside the work directory",
            Category::AnalysisBypass => "analysis bypass",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Une construction dangereuse trouvée dans un script. La ligne 0 désigne le script entier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub category: Category,
    pub construct: String,
    pub line: usize,
}

/// Le résultat de l'analyse d'un script : un refus quand il y a des constats.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardReport {
    pub findings: Vec<Finding>,
    pub warnings: Vec<String>,
}

impl GuardReport {
    #[must_use]
    pub fn refused(&self) -> bool {
        !self.findings.is_empty()
    }

    /// Les catégories distinctes trouvées, dans l'ordre des identifiants.
    #[must_use]
    pub fn categories(&self) -> Vec<Category> {
        let unique: BTreeSet<Category> = self.findings.iter().map(|f| f.category).collect();
        unique.into_iter().collect()
    }

    fn refusal(construct: &str) -> GuardReport {
        GuardReport {
            findings: vec![Finding {
                category: Category::AnalysisBypass,
                construct: construct.to_owned(),
                line: 0,
            }],
            warnings: Vec::new(),
        }
    }
}

/// Ce que le garde-fou doit savoir de l'endroit où un script peut écrire.
#[derive(Debug, Clone, Copy)]
pub struct GuardPolicy<'a> {
    /// Le répertoire de travail par application, aussi disponible pour les scripts sous `LMM_WORK_DIR`.
    pub work_dir: &'a str,
    /// D'autres répertoires absolus où les scripts peuvent écrire.
    pub extra_write_roots: &'a [String],
    /// Le répertoire personnel qui sert à développer un `~` initial, quand il est connu.
    pub home_dir: Option<&'a str>,
}

const MAX_NAME_DEPTH: usize = 12;
const MAX_REFUSAL_LINES: usize = 10;
/// CPython lui-même refuse plus que ce nombre de crochets imbriqués : aucun script valide n'est
/// donc refusé par cette borne.
const MAX_BRACKET_NESTING: usize = 200;
const ANALYSIS_BUDGET: u64 = 2_000_000;
/// Pile du fil d'analyse : un plancher, plus ce que l'arbre syntaxique le plus profond possible exige.
/// L'imbrication ne peut pas dépasser un niveau par octet de source, et l'usage mesuré reste sous
/// 128 octets par niveau : cela garde une large marge.
const GUARD_STACK_BASE: usize = 16 << 20;
const GUARD_STACK_PER_BYTE: usize = 512;
const GUARD_STACK_MAX: usize = 1 << 30;

const PATH_CLASS_NAMES: [&str; 6] = [
    "Path",
    "PosixPath",
    "WindowsPath",
    "PurePath",
    "PurePosixPath",
    "PureWindowsPath",
];
const WRITE_FLAG_NAMES: [&str; 5] = ["O_WRONLY", "O_RDWR", "O_CREAT", "O_APPEND", "O_TRUNC"];
const BARE_BUILTINS: [&str; 8] = [
    "eval",
    "exec",
    "compile",
    "__import__",
    "__builtins__",
    "open",
    "getattr",
    "str",
];

#[cfg(unix)]
fn write_flag_mask() -> i64 {
    let flags = OFlags::WRONLY | OFlags::RDWR | OFlags::CREATE | OFlags::APPEND | OFlags::TRUNC;
    i64::from(flags.bits())
}

/// Les drapeaux d'écriture de `os.open` tels que Python les définit sous Windows (`O_WRONLY`, `O_RDWR`,
/// `O_APPEND`, `O_CREAT`, `O_TRUNC`).
#[cfg(windows)]
fn write_flag_mask() -> i64 {
    0x0001 | 0x0002 | 0x0008 | 0x0100 | 0x0200
}

fn is_path_class(name: &str) -> bool {
    name.strip_prefix("pathlib.")
        .is_some_and(|class| PATH_CLASS_NAMES.contains(&class))
}

/// Pour `pathlib.Path.write_text`, rend `write_text`.
fn path_class_member(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("pathlib.")?;
    let (class, member) = rest.split_once('.')?;
    PATH_CLASS_NAMES.contains(&class).then_some(member)
}

fn in_modules(name: &str, modules: &[&str]) -> bool {
    modules.iter().any(|module| {
        name == *module
            || name
                .strip_prefix(module)
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

/// La catégorie du garde-fou d'un nom pleinement qualifié, s'il en a une.
fn classify(name: &str) -> Option<Category> {
    if matches!(
        name,
        "os.remove" | "os.unlink" | "os.rmdir" | "os.removedirs" | "shutil.rmtree"
    ) || matches!(path_class_member(name), Some("unlink" | "rmdir"))
    {
        return Some(Category::FileDeletion);
    }
    if matches!(
        name,
        "os.system" | "os.popen" | "os.posix_spawn" | "os.posix_spawnp"
    ) || in_modules(name, &["subprocess", "pty"])
    {
        return Some(Category::ProcessLaunch);
    }
    if let Some(rest) = name.strip_prefix("os.")
        && !rest.contains('.')
        && (rest.starts_with("exec") || rest.starts_with("spawn"))
    {
        return Some(Category::ProcessLaunch);
    }
    if in_modules(
        name,
        &[
            "socket",
            "urllib",
            "http.client",
            "ftplib",
            "smtplib",
            "requests",
        ],
    ) {
        return Some(Category::NetworkAccess);
    }
    if matches!(
        name,
        "eval" | "exec" | "compile" | "__import__" | "__builtins__"
    ) || in_modules(name, &["importlib", "ctypes"])
    {
        return Some(Category::AnalysisBypass);
    }
    None
}

fn normalize(name: String) -> String {
    for prefix in ["builtins.", "__builtins__."] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest.to_owned();
        }
    }
    name
}

// -- arithmétique des chemins POSIX sur des chaînes littérales ---------------

fn posix_join(parts: &[String]) -> String {
    let mut joined = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index == 0 || part.starts_with('/') {
            joined.clone_from(part);
        } else if joined.is_empty() || joined.ends_with('/') {
            joined.push_str(part);
        } else {
            joined.push('/');
            joined.push_str(part);
        }
    }
    joined
}

/// Dit si le texte d'un chemin littéral est absolu : `/` ou, sous Windows, `\`, une lettre de lecteur ou
/// un chemin UNC.
fn is_absolute_literal(literal: &str) -> bool {
    literal.starts_with('/')
        || (cfg!(windows) && (literal.starts_with('\\') || Path::new(literal).is_absolute()))
}

fn posix_dirname(path: &str) -> String {
    let head = match path.rfind('/') {
        Some(index) => path.get(..=index).unwrap_or(""),
        None => "",
    };
    if !head.is_empty() && !head.chars().all(|c| c == '/') {
        return head.trim_end_matches('/').to_owned();
    }
    head.to_owned()
}

fn posix_normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let leading = if path.starts_with("//") && !path.starts_with("///") {
        2
    } else {
        usize::from(path.starts_with('/'))
    };
    let mut kept: Vec<&str> = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part != ".." || (leading == 0 && kept.is_empty()) || kept.last() == Some(&"..") {
            kept.push(part);
        } else {
            kept.pop();
        }
    }
    let result = format!("{}{}", "/".repeat(leading), kept.join("/"));
    if result.is_empty() {
        ".".to_owned()
    } else {
        result
    }
}

// -- parcours de l'arbre syntaxique ------------------------------------------

struct ChildCollector<'a> {
    children: Vec<AnyNodeRef<'a>>,
}

impl<'a> SourceOrderVisitor<'a> for ChildCollector<'a> {
    fn enter_node(&mut self, node: AnyNodeRef<'a>) -> TraversalSignal {
        self.children.push(node);
        TraversalSignal::Skip
    }
}

/// Chaque nœud sous `root`, racine comprise, sans récursion : le parcours tient sa propre pile, si
/// bien qu'un script très imbriqué ne peut pas épuiser la pile du fil ici.
fn collect_nodes(root: AnyNodeRef<'_>) -> Vec<AnyNodeRef<'_>> {
    let mut all = Vec::new();
    let mut pending = vec![root];
    let mut collector = ChildCollector {
        children: Vec::new(),
    };
    while let Some(node) = pending.pop() {
        all.push(node);
        node.visit_source_order(&mut collector);
        pending.append(&mut collector.children);
    }
    all
}

fn node_address(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Name(name) => Some(std::ptr::from_ref(name) as usize),
        Expr::Attribute(attribute) => Some(std::ptr::from_ref(attribute) as usize),
        _ => None,
    }
}

fn line_starts(script: &str) -> Vec<usize> {
    let bytes = script.as_bytes();
    let mut starts = vec![0_usize];
    let mut index = 0_usize;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\n' => starts.push(index + 1),
            b'\r' => {
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
                starts.push(index + 1);
            }
            _ => {}
        }
        index += 1;
    }
    starts
}

fn call_arg<'a>(
    call: &'a ruff_python_ast::ExprCall,
    index: usize,
    keyword: &str,
) -> Option<&'a Expr> {
    let args = &call.arguments.args;
    if args.len() > index && !args.iter().take(index + 1).any(Expr::is_starred_expr) {
        return args.get(index);
    }
    call.arguments
        .keywords
        .iter()
        .find(|kw| kw.arg.as_ref().is_some_and(|name| name.as_str() == keyword))
        .map(|kw| &kw.value)
}

enum OpenMode {
    Read,
    Write,
    Unknown,
}

fn open_mode(mode: Option<&Expr>) -> OpenMode {
    match mode {
        None => OpenMode::Read,
        Some(Expr::StringLiteral(text)) => {
            if text.value.to_str().chars().any(|c| "wax+".contains(c)) {
                OpenMode::Write
            } else {
                OpenMode::Read
            }
        }
        Some(_) => OpenMode::Unknown,
    }
}

struct Analyzer<'a> {
    work_dir: String,
    roots: Vec<String>,
    home: Option<String>,
    line_starts: Vec<usize>,
    imports: HashMap<&'a str, Vec<String>>,
    star_modules: Vec<String>,
    assigned: HashMap<&'a str, Vec<&'a Expr>>,
    bindings: HashMap<&'a str, usize>,
    findings: RefCell<BTreeSet<(usize, Category, String)>>,
    warnings: RefCell<Vec<String>>,
    name_cache: RefCell<HashMap<(&'a str, usize), Vec<String>>>,
    budget: Cell<u64>,
    exhausted: Cell<bool>,
}

impl<'a> Analyzer<'a> {
    fn new(policy: &GuardPolicy<'_>, script: &str) -> Self {
        let mut roots = vec![policy.work_dir.to_owned()];
        roots.extend(policy.extra_write_roots.iter().cloned());
        Analyzer {
            work_dir: policy.work_dir.to_owned(),
            roots,
            home: policy.home_dir.map(str::to_owned),
            line_starts: line_starts(script),
            imports: HashMap::new(),
            star_modules: Vec::new(),
            assigned: HashMap::new(),
            bindings: HashMap::new(),
            findings: RefCell::new(BTreeSet::new()),
            warnings: RefCell::new(Vec::new()),
            name_cache: RefCell::new(HashMap::new()),
            budget: Cell::new(ANALYSIS_BUDGET),
            exhausted: Cell::new(false),
        }
    }

    // -- comptabilité ------------------------------------------------------------

    fn line(&self, offset: TextSize) -> usize {
        let offset = usize::from(offset);
        self.line_starts.partition_point(|start| *start <= offset)
    }

    /// Dépense une unité du budget de travail. Un script qui fait exploser la résolution des noms
    /// l'épuise et est refusé plutôt qu'analysé sans fin.
    fn tick(&self) -> bool {
        let left = self.budget.get();
        if left == 0 {
            self.exhausted.set(true);
            return false;
        }
        self.budget.set(left - 1);
        true
    }

    fn add(&self, category: Category, construct: String, line: usize) {
        self.findings
            .borrow_mut()
            .insert((line, category, construct));
    }

    fn warn(&self, line: usize, message: &str) {
        let text = format!("line {line}: {message}");
        let mut warnings = self.warnings.borrow_mut();
        if !warnings.contains(&text) {
            warnings.push(text);
        }
    }

    fn bind(&mut self, name: &'a str) {
        *self.bindings.entry(name).or_insert(0) += 1;
    }

    /// Développe un `~` initial avec le répertoire personnel, en gardant le reste du texte tel quel.
    /// Les formes `~user` sont laissées : elles exigent la base des utilisateurs.
    fn expand_user(&self, text: &str) -> String {
        let rest = text.strip_prefix('~').filter(|rest| {
            rest.is_empty() || rest.starts_with('/') || (cfg!(windows) && rest.starts_with('\\'))
        });
        match (rest, &self.home) {
            (Some(rest), Some(home)) => {
                let joined = format!("{}{rest}", home.trim_end_matches('/'));
                if joined.is_empty() {
                    "/".to_owned()
                } else {
                    joined
                }
            }
            _ => text.to_owned(),
        }
    }

    // -- collecte des symboles ---------------------------------------------------

    fn collect(&mut self, nodes: &[AnyNodeRef<'a>]) {
        for node in nodes {
            match *node {
                AnyNodeRef::StmtImport(stmt) => {
                    for alias in &stmt.names {
                        if let Some(asname) = &alias.asname {
                            self.imports
                                .entry(asname.as_str())
                                .or_default()
                                .push(alias.name.as_str().to_owned());
                            self.bind(asname.as_str());
                        } else {
                            let top = alias.name.as_str().split('.').next().unwrap_or("");
                            self.imports.entry(top).or_default().push(top.to_owned());
                            self.bind(top);
                        }
                    }
                }
                AnyNodeRef::StmtImportFrom(stmt) => {
                    let module = if stmt.level == 0 {
                        stmt.module.as_ref().map(|m| m.as_str())
                    } else {
                        None
                    };
                    for alias in &stmt.names {
                        if alias.name.as_str() == "*" {
                            if let Some(module) = module {
                                self.star_modules.push(module.to_owned());
                            }
                            continue;
                        }
                        let bound = alias.asname.as_ref().unwrap_or(&alias.name).as_str();
                        if let Some(module) = module {
                            self.imports
                                .entry(bound)
                                .or_default()
                                .push(format!("{module}.{}", alias.name.as_str()));
                        }
                        self.bind(bound);
                    }
                }
                AnyNodeRef::StmtAssign(stmt) => {
                    for target in &stmt.targets {
                        if let Expr::Name(name) = target {
                            self.assigned
                                .entry(name.id.as_str())
                                .or_default()
                                .push(&stmt.value);
                        }
                    }
                }
                AnyNodeRef::StmtAnnAssign(stmt) => {
                    if let (Expr::Name(name), Some(value)) = (&*stmt.target, &stmt.value) {
                        self.assigned
                            .entry(name.id.as_str())
                            .or_default()
                            .push(value);
                    }
                }
                AnyNodeRef::ExprName(name) => {
                    if matches!(name.ctx, ExprContext::Store | ExprContext::Del) {
                        self.bind(name.id.as_str());
                    }
                }
                AnyNodeRef::StmtFunctionDef(stmt) => self.bind(stmt.name.as_str()),
                AnyNodeRef::StmtClassDef(stmt) => self.bind(stmt.name.as_str()),
                AnyNodeRef::Parameter(parameter) => self.bind(parameter.name.as_str()),
                AnyNodeRef::ExceptHandlerExceptHandler(handler) => {
                    if let Some(name) = &handler.name {
                        self.bind(name.as_str());
                    }
                }
                AnyNodeRef::PatternMatchAs(pattern) => {
                    if let Some(name) = &pattern.name {
                        self.bind(name.as_str());
                    }
                }
                AnyNodeRef::PatternMatchStar(pattern) => {
                    if let Some(name) = &pattern.name {
                        self.bind(name.as_str());
                    }
                }
                AnyNodeRef::PatternMatchMapping(pattern) => {
                    if let Some(rest) = &pattern.rest {
                        self.bind(rest.as_str());
                    }
                }
                _ => {}
            }
        }
    }

    // -- résolution des noms -----------------------------------------------------

    fn qualify(&self, expr: &'a Expr, depth: usize, seen: &[&'a str]) -> Vec<String> {
        match expr {
            Expr::Name(name) => self.qualify_name(name.id.as_str(), depth, seen),
            Expr::Attribute(attribute) => self.qualify_attribute(attribute, depth, seen),
            Expr::Call(call) => self.qualify_call(call, depth, seen),
            _ => Vec::new(),
        }
    }

    fn qualify_name(&self, name: &'a str, depth: usize, seen: &[&'a str]) -> Vec<String> {
        // Un nom rencontré seul se résout toujours de la même façon : il est donc résolu une seule fois,
        // et un script qui réaffecte un nom des milliers de fois reste peu coûteux.
        let cacheable = seen.is_empty();
        if cacheable && let Some(hit) = self.name_cache.borrow().get(&(name, depth)) {
            return hit.clone();
        }
        let resolved = self.resolve_name(name, depth, seen);
        if cacheable && !self.exhausted.get() {
            self.name_cache
                .borrow_mut()
                .insert((name, depth), resolved.clone());
        }
        resolved
    }

    fn resolve_name(&self, name: &'a str, depth: usize, seen: &[&'a str]) -> Vec<String> {
        if depth > MAX_NAME_DEPTH || !self.tick() || seen.contains(&name) {
            return Vec::new();
        }
        let mut found: Vec<String> = self.imports.get(name).cloned().unwrap_or_default();
        if let Some(values) = self.assigned.get(name) {
            let mut seen_here = seen.to_vec();
            seen_here.push(name);
            for value in values {
                if self.exhausted.get() {
                    break;
                }
                found.extend(self.qualify(value, depth + 1, &seen_here));
            }
        }
        if found.is_empty() {
            if self.bindings.contains_key(name) {
                return Vec::new();
            }
            found = self
                .star_modules
                .iter()
                .map(|module| format!("{module}.{name}"))
                .collect();
            found.push(name.to_owned());
        }
        found.into_iter().map(normalize).collect()
    }

    fn qualify_attribute(
        &self,
        attribute: &'a ruff_python_ast::ExprAttribute,
        depth: usize,
        seen: &[&'a str],
    ) -> Vec<String> {
        if depth > MAX_NAME_DEPTH || !self.tick() {
            return Vec::new();
        }
        self.qualify(&attribute.value, depth + 1, seen)
            .into_iter()
            .map(|base| normalize(format!("{base}.{}", attribute.attr.as_str())))
            .collect()
    }

    fn qualify_call(
        &self,
        call: &'a ruff_python_ast::ExprCall,
        depth: usize,
        seen: &[&'a str],
    ) -> Vec<String> {
        if depth > MAX_NAME_DEPTH || !self.tick() {
            return Vec::new();
        }
        let Expr::Name(func) = &*call.func else {
            return Vec::new();
        };
        if func.id.as_str() != "getattr"
            || self.bindings.contains_key("getattr")
            || call.arguments.args.len() < 2
        {
            return Vec::new();
        }
        let (Some(target), Some(attr)) = (call.arguments.args.first(), call.arguments.args.get(1))
        else {
            return Vec::new();
        };
        let bases = self.qualify(target, depth + 1, seen);
        if let Expr::StringLiteral(text) = attr {
            return bases
                .into_iter()
                .map(|base| normalize(format!("{base}.{}", text.value.to_str())))
                .collect();
        }
        if bases
            .iter()
            .any(|base| classify(&format!("{base}.x")).is_some() || classify(base).is_some())
        {
            self.warn(
                self.line(call.start()),
                "dynamic attribute access on a guarded module",
            );
        }
        Vec::new()
    }

    fn single_value(&self, name: &str) -> Option<&'a Expr> {
        let values = self.assigned.get(name)?;
        match (values.as_slice(), self.bindings.get(name)) {
            ([value], Some(1)) => Some(value),
            _ => None,
        }
    }

    // -- chemins littéraux -------------------------------------------------------

    /// Le chemin auquel une expression s'évalue toujours, quand on peut le savoir statiquement.
    fn literal_path(&self, expr: &'a Expr, depth: usize) -> Option<String> {
        if depth > MAX_NAME_DEPTH || !self.tick() {
            return None;
        }
        let sub = depth + 1;
        match expr {
            Expr::StringLiteral(text) => Some(text.value.to_str().to_owned()),
            Expr::Name(name) => {
                let id = name.id.as_str();
                if let Some(value) = self.single_value(id) {
                    return self.literal_path(value, sub);
                }
                if id == "LMM_WORK_DIR" && !self.bindings.contains_key(id) {
                    return Some(self.work_dir.clone());
                }
                None
            }
            Expr::FString(fstring) => {
                let mut out = String::new();
                for part in fstring.value.iter() {
                    match part {
                        ruff_python_ast::FStringPartRef::Literal(text) => out.push_str(text),
                        ruff_python_ast::FStringPartRef::FString(inner) => {
                            for element in inner.elements.iter() {
                                match element {
                                    InterpolatedStringElement::Literal(text) => {
                                        out.push_str(&text.value);
                                    }
                                    InterpolatedStringElement::Interpolation(slot) => {
                                        if slot.conversion != ConversionFlag::None
                                            || slot.format_spec.is_some()
                                            || slot.debug_text.is_some()
                                        {
                                            return None;
                                        }
                                        out.push_str(&self.literal_path(&slot.expression, sub)?);
                                    }
                                }
                            }
                        }
                    }
                }
                Some(out)
            }
            Expr::BinOp(binary) if matches!(binary.op, Operator::Add | Operator::Div) => {
                let left = self.literal_path(&binary.left, sub)?;
                let right = self.literal_path(&binary.right, sub)?;
                if matches!(binary.op, Operator::Add) {
                    Some(format!("{left}{right}"))
                } else {
                    Some(posix_join(&[left, right]))
                }
            }
            Expr::Attribute(attribute) if attribute.attr.as_str() == "parent" => {
                let inner = self.literal_path(&attribute.value, sub)?;
                Some(posix_dirname(&inner))
            }
            Expr::Call(call) => self.literal_call(call, sub),
            _ => None,
        }
    }

    fn literal_call(&self, call: &'a ruff_python_ast::ExprCall, depth: usize) -> Option<String> {
        if !call.arguments.keywords.is_empty() {
            return None;
        }
        let names = self.qualify(&call.func, 0, &[]);
        if let Expr::Attribute(method) = &*call.func {
            let attr = method.attr.as_str();
            if matches!(attr, "expanduser" | "resolve" | "absolute")
                && call.arguments.args.is_empty()
                && let Some(inner) = self.literal_path(&method.value, depth)
            {
                return Some(if attr == "expanduser" {
                    self.expand_user(&inner)
                } else {
                    inner
                });
            }
            if attr == "joinpath" {
                let base = self.literal_path(&method.value, depth)?;
                let mut parts = vec![base];
                for arg in &call.arguments.args {
                    parts.push(self.literal_path(arg, depth)?);
                }
                return Some(posix_join(&parts));
            }
        }
        let [name] = names.as_slice() else {
            return None;
        };
        let mut strs = Vec::new();
        for arg in &call.arguments.args {
            strs.push(self.literal_path(arg, depth)?);
        }
        if is_path_class(name) {
            return Some(if strs.is_empty() {
                ".".to_owned()
            } else {
                posix_join(&strs)
            });
        }
        if let Some(member) = path_class_member(name)
            && member == "home"
        {
            return Some(self.expand_user("~"));
        }
        if matches!(name.as_str(), "os.path.join" | "posixpath.join") && !strs.is_empty() {
            return Some(posix_join(&strs));
        }
        let [single] = strs.as_slice() else {
            return None;
        };
        match name.as_str() {
            "os.path.dirname" => Some(posix_dirname(single)),
            "os.path.expanduser" => Some(self.expand_user(single)),
            "os.path.normpath" => Some(posix_normpath(single)),
            "str" | "os.fspath" => Some(single.clone()),
            _ => None,
        }
    }

    /// Dit si l'expression est, pour autant qu'on puisse le dire, un objet chemin de pathlib.
    fn is_path_expr(&self, expr: &'a Expr, depth: usize) -> bool {
        if depth > MAX_NAME_DEPTH || !self.tick() {
            return false;
        }
        let sub = depth + 1;
        match expr {
            Expr::Call(call) => {
                if self.qualify(&call.func, 0, &[]).iter().any(|q| {
                    is_path_class(q)
                        || path_class_member(q).is_some_and(|m| m == "home" || m == "cwd")
                }) {
                    return true;
                }
                match &*call.func {
                    Expr::Attribute(method) => {
                        matches!(
                            method.attr.as_str(),
                            "resolve"
                                | "absolute"
                                | "expanduser"
                                | "joinpath"
                                | "with_name"
                                | "with_suffix"
                                | "with_stem"
                        ) && self.is_path_expr(&method.value, sub)
                    }
                    _ => false,
                }
            }
            Expr::BinOp(binary) if matches!(binary.op, Operator::Div) => {
                self.is_path_expr(&binary.left, sub) || self.is_path_expr(&binary.right, sub)
            }
            Expr::Attribute(attribute) if attribute.attr.as_str() == "parent" => {
                self.is_path_expr(&attribute.value, sub)
            }
            Expr::Name(name) => {
                let Some(values) = self.assigned.get(name.id.as_str()) else {
                    return false;
                };
                for value in values {
                    if self.exhausted.get() {
                        return false;
                    }
                    if self.is_path_expr(value, sub) {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    // -- contrôles d'écriture ----------------------------------------------------

    fn check_write_path(&self, expr: &'a Expr, line: usize, construct: &str) {
        let Some(literal) = self.literal_path(expr, 0) else {
            self.warn(
                line,
                &format!("path not statically verifiable ({construct})"),
            );
            return;
        };
        let literal = self.expand_user(&literal);
        if !is_absolute_literal(&literal) {
            self.warn(
                line,
                &format!(
                    "relative path not statically verifiable ({construct}); \
                     build paths from LMM_WORK_DIR"
                ),
            );
            return;
        }
        let inside_a_root = self
            .roots
            .iter()
            .any(|root| resolve_under(Path::new(root), Path::new(&literal)).is_ok());
        if !inside_a_root {
            self.add(
                Category::WriteOutsideWorkDir,
                format!("{construct} to {literal}"),
                line,
            );
        }
    }

    fn check_open(
        &self,
        line: usize,
        path: Option<&'a Expr>,
        mode: Option<&'a Expr>,
        construct: &str,
    ) {
        let Some(path) = path else {
            return;
        };
        match open_mode(mode) {
            OpenMode::Read => {}
            OpenMode::Unknown => {
                self.warn(
                    line,
                    &format!("open mode not statically verifiable ({construct})"),
                );
            }
            OpenMode::Write => self.check_write_path(path, line, construct),
        }
    }

    fn check_os_open(&self, call: &'a ruff_python_ast::ExprCall) {
        let (Some(path), Some(flags)) = (call_arg(call, 0, "path"), call_arg(call, 1, "flags"))
        else {
            return;
        };
        let mask = write_flag_mask();
        let writes = collect_nodes(AnyNodeRef::from(flags))
            .iter()
            .any(|node| match *node {
                AnyNodeRef::ExprAttribute(attribute) => {
                    WRITE_FLAG_NAMES.contains(&attribute.attr.as_str())
                }
                AnyNodeRef::ExprName(name) => WRITE_FLAG_NAMES.contains(&name.id.as_str()),
                AnyNodeRef::ExprNumberLiteral(number) => match &number.value {
                    Number::Int(value) => value.as_i64().is_none_or(|v| v & mask != 0),
                    _ => false,
                },
                _ => false,
            });
        if writes {
            self.check_write_path(path, self.line(call.start()), "os.open");
        }
    }

    fn check_call(&self, call: &'a ruff_python_ast::ExprCall) {
        let line = self.line(call.start());
        let names: BTreeSet<String> = self.qualify(&call.func, 0, &[]).into_iter().collect();
        for name in &names {
            let name = name.as_str();
            if matches!(name, "open" | "io.open" | "codecs.open") {
                let keyword = if name == "codecs.open" {
                    "filename"
                } else {
                    "file"
                };
                self.check_open(
                    line,
                    call_arg(call, 0, keyword),
                    call_arg(call, 1, "mode"),
                    "open()",
                );
            } else if name == "os.open" {
                self.check_os_open(call);
            } else if name == "shutil.move" {
                for (index, key) in [(0, "src"), (1, "dst")] {
                    if let Some(arg) = call_arg(call, index, key) {
                        self.check_write_path(arg, line, "shutil.move");
                    }
                }
            } else if name.starts_with("shutil.copy") && name != "shutil.copyfileobj" {
                if let Some(arg) = call_arg(call, 1, "dst") {
                    self.check_write_path(arg, line, name);
                }
            } else if let Some(member @ ("write_text" | "write_bytes")) = path_class_member(name) {
                if let Some(target) = call_arg(call, 0, "self") {
                    self.check_write_path(target, line, member);
                }
            } else if path_class_member(name) == Some("open")
                && let Some(target) = call_arg(call, 0, "self")
            {
                self.check_open(line, Some(target), call_arg(call, 1, "mode"), "Path.open()");
            }
        }
        if let Expr::Attribute(method) = &*call.func
            && self.is_path_expr(&method.value, 0)
        {
            match method.attr.as_str() {
                attr @ ("write_text" | "write_bytes") => {
                    self.check_write_path(&method.value, line, attr);
                }
                "open" => {
                    self.check_open(
                        line,
                        Some(&method.value),
                        call_arg(call, 0, "mode"),
                        "Path.open()",
                    );
                }
                _ => {}
            }
        }
    }

    // -- visiteurs ---------------------------------------------------------------

    fn check_module(&self, module: &str, construct: String, line: usize) {
        if let Some(category) = classify(module) {
            self.add(category, construct, line);
        }
    }

    fn on_import(&self, stmt: &'a ruff_python_ast::StmtImport) {
        let line = self.line(stmt.start());
        for alias in &stmt.names {
            let name = alias.name.as_str();
            self.check_module(name, format!("import {name}"), line);
        }
    }

    fn on_import_from(&self, stmt: &'a ruff_python_ast::StmtImportFrom) {
        let Some(module) = stmt.module.as_ref().map(|m| m.as_str()) else {
            return;
        };
        if stmt.level != 0 {
            return;
        }
        let line = self.line(stmt.start());
        self.check_module(module, format!("from {module} import ..."), line);
        for alias in &stmt.names {
            if alias.name.as_str() != "*" {
                let full = format!("{module}.{}", alias.name.as_str());
                self.check_module(&full, normalize(full.clone()), line);
            }
        }
    }

    fn on_name(&self, name: &'a ruff_python_ast::ExprName) {
        if !matches!(name.ctx, ExprContext::Load) {
            return;
        }
        let id = name.id.as_str();
        let line = self.line(name.start());
        for qualified in self.qualify_name(id, 0, &[]) {
            if qualified == id
                && !self.imports.contains_key(id)
                && !BARE_BUILTINS.contains(&qualified.as_str())
            {
                continue;
            }
            if let Some(category) = classify(&qualified) {
                self.add(category, qualified, line);
            }
        }
    }

    fn on_attribute(&self, attribute: &'a ruff_python_ast::ExprAttribute) {
        if !matches!(attribute.ctx, ExprContext::Load) {
            return;
        }
        let line = self.line(attribute.start());
        for qualified in self.qualify_attribute(attribute, 0, &[]) {
            if let Some(category) = classify(&qualified) {
                self.add(category, qualified, line);
            }
        }
        let name = attribute.attr.as_str();
        if matches!(name, "unlink" | "rmdir") && self.is_path_expr(&attribute.value, 0) {
            self.add(Category::FileDeletion, format!("Path.{name}"), line);
        }
    }

    fn on_call(&self, call: &'a ruff_python_ast::ExprCall) {
        let line = self.line(call.start());
        for qualified in self.qualify_call(call, 0, &[]) {
            if let Some(category) = classify(&qualified) {
                self.add(category, qualified, line);
            }
        }
        self.check_call(call);
    }

    /// Visite chaque nœud. Les parties internes d'une chaîne pointée simple sont sautées : la plus
    /// externe est classée une seule fois, avec son nom pleinement qualifié.
    fn run(&self, nodes: &[AnyNodeRef<'a>]) {
        let mut purity: HashMap<usize, bool> = HashMap::new();
        let mut inner: BTreeSet<usize> = BTreeSet::new();
        for node in nodes {
            if let AnyNodeRef::ExprAttribute(attribute) = *node
                && is_pure_chain(attribute, &mut purity)
                && let Some(address) = node_address(&attribute.value)
            {
                inner.insert(address);
            }
        }
        for node in nodes {
            match *node {
                AnyNodeRef::StmtImport(stmt) => self.on_import(stmt),
                AnyNodeRef::StmtImportFrom(stmt) => self.on_import_from(stmt),
                AnyNodeRef::ExprName(name) => {
                    if !inner.contains(&(std::ptr::from_ref(name) as usize)) {
                        self.on_name(name);
                    }
                }
                AnyNodeRef::ExprAttribute(attribute) => {
                    if !inner.contains(&(std::ptr::from_ref(attribute) as usize)) {
                        self.on_attribute(attribute);
                    }
                }
                AnyNodeRef::ExprCall(call) => self.on_call(call),
                _ => {}
            }
        }
    }
}

/// Dit si une chaîne d'attributs se termine par un nom simple (`a.b.c`), mémoïsé pour qu'une très
/// longue chaîne coûte un passage au lieu d'un passage par maillon.
fn is_pure_chain(
    attribute: &ruff_python_ast::ExprAttribute,
    memo: &mut HashMap<usize, bool>,
) -> bool {
    let mut path = vec![std::ptr::from_ref(attribute) as usize];
    let mut current: &Expr = &attribute.value;
    let result = loop {
        match current {
            Expr::Name(_) => break true,
            Expr::Attribute(next) => {
                let key = std::ptr::from_ref(next) as usize;
                if let Some(&known) = memo.get(&key) {
                    break known;
                }
                path.push(key);
                current = &next.value;
            }
            _ => break false,
        }
    };
    for key in path {
        memo.insert(key, result);
    }
    result
}

/// Plus grande imbrication de crochets d'un script, d'après les jetons, sans récursion.
/// `None` quand l'analyseur lexical ne termine pas dans une borne que respecte tout flux de jetons
/// de cette longueur.
fn bracket_depth(script: &str) -> Option<usize> {
    let mut lexer = lex(script, Mode::Module);
    let mut depth = 0_usize;
    let mut deepest = 0_usize;
    let limit = script.len().saturating_mul(2).saturating_add(1024);
    for _ in 0..limit {
        match lexer.next_token() {
            TokenKind::Lpar | TokenKind::Lsqb | TokenKind::Lbrace => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace => {
                depth = depth.saturating_sub(1);
            }
            TokenKind::EndOfFile => return Some(deepest),
            _ => {}
        }
    }
    None
}

fn analyse(script: &str, policy: &GuardPolicy<'_>) -> GuardReport {
    match bracket_depth(script) {
        Some(depth) if depth <= MAX_BRACKET_NESTING => {}
        Some(_) => return GuardReport::refusal("nesting too deep to analyse"),
        None => return GuardReport::refusal("analysis did not finish"),
    }
    let Ok(parsed) = parse_module(script) else {
        return GuardReport {
            findings: Vec::new(),
            warnings: vec!["syntax error not analysed".to_owned()],
        };
    };
    let nodes = collect_nodes(AnyNodeRef::from(parsed.syntax()));
    let mut analyzer = Analyzer::new(policy, script);
    analyzer.collect(&nodes);
    analyzer.run(&nodes);
    if analyzer.exhausted.get() {
        return GuardReport::refusal("name resolution too complex to analyse");
    }
    let findings = analyzer
        .findings
        .take()
        .into_iter()
        .map(|(line, category, construct)| Finding {
            category,
            construct,
            line,
        })
        .collect();
    let warnings = analyzer.warnings.take();
    GuardReport { findings, warnings }
}

fn guard_stack_size(script_len: usize) -> Option<usize> {
    script_len
        .checked_mul(GUARD_STACK_PER_BYTE)?
        .checked_add(GUARD_STACK_BASE)
        .filter(|size| *size <= GUARD_STACK_MAX)
}

/// Exécute `analysis` dans son propre fil, avec une pile dimensionnée pour un script de
/// `script_len` octets, et transforme chacune de ses façons d'échouer en refus.
///
/// Une panique est attrapée, une pile qui ne peut pas être allouée est un refus, et un script trop
/// grand pour toute pile sûre est refusé sans être tenté. Prendre l'analyse en paramètre permet aux
/// tests de prouver qu'une panique ne s'échappe jamais.
#[must_use]
pub fn run_isolated<F>(script_len: usize, analysis: F) -> GuardReport
where
    F: FnOnce() -> GuardReport + Send,
{
    let Some(stack_size) = guard_stack_size(script_len) else {
        return GuardReport::refusal("script too large to analyse");
    };
    let outcome = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("script-guard".to_owned())
            .stack_size(stack_size)
            .spawn_scoped(scope, analysis)
            .map(std::thread::ScopedJoinHandle::join)
    });
    match outcome {
        Ok(Ok(report)) => report,
        Ok(Err(_)) => GuardReport::refusal("analysis failed"),
        Err(_) => GuardReport::refusal("analysis could not start"),
    }
}

/// Analyse statiquement un script Python ; rien n'est exécuté.
///
/// Quoi qu'il arrive pendant l'analyse, la réponse est un refus. Un script qui ne s'analyse pas
/// n'est pas refusé : l'application signalera sa propre erreur de syntaxe.
#[must_use]
pub fn guard_script(script: &str, policy: &GuardPolicy<'_>) -> GuardReport {
    run_isolated(script.len(), || analyse(script, policy))
}

/// Message pour un script refusé : catégorie, construction, ligne, et comment l'autoriser.
#[must_use]
pub fn format_refusal(report: &GuardReport) -> String {
    let mut lines =
        vec!["Script refused by the safety guard before reaching the application:".to_owned()];
    for finding in report.findings.iter().take(MAX_REFUSAL_LINES) {
        let place = if finding.line == 0 {
            String::new()
        } else {
            format!(" at line {}", finding.line)
        };
        lines.push(format!(
            "  {} ({}){place}: {}",
            finding.category.id(),
            finding.category.label(),
            finding.construct
        ));
    }
    let hidden = report.findings.len().saturating_sub(MAX_REFUSAL_LINES);
    if hidden > 0 {
        lines.push(format!("  ... and {hidden} more"));
    }
    lines.push(
        "The guard is a safety net, not a sandbox. Write files under LMM_WORK_DIR or a directory \
         listed in extra_write_roots. To allow these constructs the operator sets \
         allow_dangerous_scripts = true in the [security] section of config.toml; \
         the audit log then records guard: bypassed."
            .to_owned(),
    );
    lines.join("\n")
}
