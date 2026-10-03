//! Hub-side security primitives: token, private files, paths, script guard, bounds.
//!
//! Why this module exists
//! A script runs with the user's rights inside an application, and everything the
//! Hub returns leaves the machine. Keeping every check in one module means a
//! reviewer can read the whole security surface at once, and no other module can
//! quietly grow a weaker copy of a check.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, DirBuilder, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Component, Path, PathBuf};

use ruff_python_ast::token::TokenKind;
use ruff_python_ast::visitor::source_order::{SourceOrderVisitor, TraversalSignal};
use ruff_python_ast::{
    AnyNodeRef, ConversionFlag, Expr, ExprContext, InterpolatedStringElement, Number, Operator,
};
use ruff_python_parser::{Mode, lexer::lex, parse_module};
use ruff_text_size::{Ranged, TextSize};
use rustix::fs::{Mode as FileMode, OFlags};
use rustix::io::Errno;
use rustix::process::{Resource, Rlimit};

/// Marker that ends every string cut to fit a size bound.
pub const TRUNCATION_MARKER: &str = "[truncated]";
/// Number of hexadecimal characters in the bridge token.
pub const TOKEN_HEX_LENGTH: usize = 64;

const TOKEN_READ_CAP: u64 = 4096;
const MAX_SYMLINK_HOPS: usize = 40;
const PRIVATE_FILE_MODE: u32 = 0o600;
const PRIVATE_DIR_MODE: u32 = 0o700;
const GROUP_AND_OTHER_BITS: u32 = 0o077;

// --------------------------------------------------------------------------
// Errors
// --------------------------------------------------------------------------

/// A security condition failed or could not be verified.
///
/// No variant ever carries the content of a file or the token: only paths, modes
/// and sizes, so that a message can be shown to the operator without leaking.
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
    /// Whether the error means the file does not exist (as opposed to being unsafe).
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
// Process hardening
// --------------------------------------------------------------------------

/// Disable core dumps and restrict the file creation mask.
///
/// Must run before the token is read so that a crash cannot write it to disk.
///
/// # Errors
/// Fails closed when the core-dump limit cannot be lowered.
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

// --------------------------------------------------------------------------
// Files and directories
// --------------------------------------------------------------------------

fn current_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// How a control file is opened. Truncation is not an option: a control file is
/// only ever read, appended to, or created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAccess {
    Read,
    AppendOrCreate,
    CreateNew,
}

impl ControlAccess {
    fn flags(self) -> OFlags {
        match self {
            ControlAccess::Read => OFlags::RDONLY,
            ControlAccess::AppendOrCreate => OFlags::WRONLY | OFlags::APPEND | OFlags::CREATE,
            ControlAccess::CreateNew => OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
        }
    }
}

/// Open without following a final symbolic link; refuse anything but a regular file.
///
/// # Errors
/// Refuses links, non-regular files and any failure of the open itself.
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

/// Open a token, journal or capture file after checking owner and permissions.
///
/// The file must belong to the current user and carry no group or other bit.
///
/// # Errors
/// Any failure of [`open_regular_nofollow`], a foreign owner, or a loose mode.
pub fn open_control_file(path: &Path, access: ControlAccess) -> Result<File, SecurityError> {
    open_control_file_as(path, access, current_uid())
}

/// Same as [`open_control_file`] with an explicit expected owner.
///
/// # Errors
/// See [`open_control_file`].
pub fn open_control_file_as(
    path: &Path,
    access: ControlAccess,
    expected_uid: u32,
) -> Result<File, SecurityError> {
    let file = open_regular_nofollow(path, access)?;
    let info = file.metadata().map_err(|e| io_error("inspect", path, e))?;
    if info.uid() != expected_uid {
        return Err(SecurityError::ForeignOwner {
            path: path.to_path_buf(),
        });
    }
    if info.mode() & GROUP_AND_OTHER_BITS != 0 {
        return Err(SecurityError::LooseMode {
            path: path.to_path_buf(),
            mode: info.mode() & 0o7777,
            expected: PRIVATE_FILE_MODE,
        });
    }
    Ok(file)
}

/// Read a control file (token, configuration) in one bounded read.
///
/// The single entry point for every file the Hub trusts to control its behaviour:
/// regular file, no symbolic link, owned by the current user, no group or other
/// permission bit, at most `max_bytes` long.
///
/// # Errors
/// [`SecurityError::NotFound`] when the file is absent; any other variant means
/// the file exists but cannot be trusted.
pub fn read_control_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, SecurityError> {
    read_control_file_as(path, max_bytes, current_uid())
}

/// Same as [`read_control_file`] with an explicit expected owner.
///
/// # Errors
/// See [`read_control_file`].
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

/// Create the directory with mode 700, or verify an existing one is private.
///
/// # Errors
/// Refuses links, non-directories, foreign owners and group/other permissions.
pub fn ensure_private_dir(path: &Path) -> Result<(), SecurityError> {
    ensure_private_dir_as(path, current_uid())
}

/// Same as [`ensure_private_dir`] with an explicit expected owner.
///
/// # Errors
/// See [`ensure_private_dir`].
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

// --------------------------------------------------------------------------
// Token
// --------------------------------------------------------------------------

/// The secret shared with the bridges.
///
/// It has no `Display` and its `Debug` output hides the value, so that logging a
/// structure that holds it cannot leak it.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// Wrap a non-empty secret. [`load_token`] is the only way to obtain the token
    /// the bridges expect; this constructor exists so that components can be built
    /// with any secret.
    ///
    /// # Errors
    /// An empty value would match everywhere, so it is refused.
    pub fn new(value: impl Into<String>) -> Result<Token, SecurityError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SecurityError::TokenEmpty);
        }
        Ok(Token(value))
    }

    /// The secret itself. Callers must never log it or return it.
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

/// Read and validate the bridge token (64 hexadecimal characters).
///
/// # Errors
/// Any unsafe or malformed token file. The message never contains the token.
pub fn load_token(path: &Path) -> Result<Token, SecurityError> {
    load_token_as(path, current_uid())
}

/// Same as [`load_token`] with an explicit expected owner.
///
/// # Errors
/// See [`load_token`].
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

/// Create the token file (exclusive, mode 600) in a private directory.
///
/// Refuses to replace an existing token unless `rotate` is true.
///
/// # Errors
/// An existing token without `rotate`, an unsafe directory, or an I/O failure.
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
        // Best effort: a staging file left behind is private and holds no live token.
        let _ = fs::remove_file(&staging);
        return Err(io_error("write the token file", path, e));
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Paths, targets, sizes
// --------------------------------------------------------------------------

fn realpath(path: &Path) -> Result<PathBuf, SecurityError> {
    let mut pending: VecDeque<OsString> = VecDeque::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => pending.push_back(name.to_os_string()),
            Component::ParentDir => pending.push_back(OsString::from("..")),
            _ => {}
        }
    }
    let mut resolved = PathBuf::from("/");
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
            resolved = PathBuf::from("/");
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

/// Resolve symbolic links and require the result to stay inside `root`.
///
/// Components that do not exist yet are kept as they are, so the check also works
/// for a file that is about to be created.
///
/// # Errors
/// [`SecurityError::PathEscapes`] when the resolved path leaves `root`.
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

/// Accept only an exact, case-sensitive match with an active application name.
///
/// # Errors
/// [`SecurityError::UnknownTarget`], which lists the active applications.
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

/// Return the script size in bytes, or refuse when it exceeds the limit.
///
/// # Errors
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

/// Cut `text` to at most `limit` UTF-8 bytes, ending with the truncation marker.
///
/// A character is never split. Returns the text and whether it was cut.
///
/// # Errors
/// [`SecurityError::LimitTooSmall`] when the limit cannot even hold the marker.
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
// Script guard
// --------------------------------------------------------------------------

/// The kind of danger a finding belongs to. The identifier is a value of the
/// product: it appears in refusal messages and in the audit journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    FileDeletion,
    ProcessLaunch,
    NetworkAccess,
    WriteOutsideWorkDir,
    AnalysisBypass,
}

impl Category {
    /// The identifier shown to the model and written to the audit journal.
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

/// One dangerous construct found in a script. Line 0 means the whole script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub category: Category,
    pub construct: String,
    pub line: usize,
}

/// The outcome of analysing a script: a refusal when there are findings.
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

    /// The distinct categories found, in identifier order.
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

/// What the guard needs to know about where a script may write.
#[derive(Debug, Clone, Copy)]
pub struct GuardPolicy<'a> {
    /// The per-application work directory, also available to scripts as `LMM_WORK_DIR`.
    pub work_dir: &'a str,
    /// Further absolute directories scripts may write to.
    pub extra_write_roots: &'a [String],
    /// The home directory used to expand a leading `~`, when known.
    pub home_dir: Option<&'a str>,
}

const MAX_NAME_DEPTH: usize = 12;
const MAX_REFUSAL_LINES: usize = 10;
/// CPython itself refuses more than this many nested brackets, so no valid
/// script is refused by this bound.
const MAX_BRACKET_NESTING: usize = 200;
const ANALYSIS_BUDGET: u64 = 2_000_000;
/// Stack for the analysis thread: a floor, plus what the deepest possible
/// syntax tree needs. Nesting cannot exceed one level per source byte, and
/// measured use stays below 128 bytes per level, so this keeps a wide margin.
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

fn write_flag_mask() -> i64 {
    let flags = OFlags::WRONLY | OFlags::RDWR | OFlags::CREATE | OFlags::APPEND | OFlags::TRUNC;
    i64::from(flags.bits())
}

fn is_path_class(name: &str) -> bool {
    name.strip_prefix("pathlib.")
        .is_some_and(|class| PATH_CLASS_NAMES.contains(&class))
}

/// For `pathlib.Path.write_text` returns `write_text`.
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

/// The guard category of a fully qualified name, if it has one.
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

// -- posix path arithmetic on literal strings ---------------------------------

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

// -- syntax tree walking --------------------------------------------------------

struct ChildCollector<'a> {
    children: Vec<AnyNodeRef<'a>>,
}

impl<'a> SourceOrderVisitor<'a> for ChildCollector<'a> {
    fn enter_node(&mut self, node: AnyNodeRef<'a>) -> TraversalSignal {
        self.children.push(node);
        TraversalSignal::Skip
    }
}

/// Every node under `root`, root included, without recursion: the walk keeps its
/// own stack, so a deeply nested script cannot exhaust the thread's stack here.
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

    // -- bookkeeping ------------------------------------------------------------

    fn line(&self, offset: TextSize) -> usize {
        let offset = usize::from(offset);
        self.line_starts.partition_point(|start| *start <= offset)
    }

    /// Spend one unit of the work budget. A script that makes name resolution
    /// explode runs out of it and is refused rather than analysed forever.
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

    fn expand_user(&self, text: &str) -> String {
        let rest = if text == "~" {
            Some("")
        } else {
            text.strip_prefix("~/")
        };
        match (rest, &self.home) {
            (Some(rest), Some(home)) => {
                let home = home.trim_end_matches('/');
                if rest.is_empty() {
                    if home.is_empty() {
                        "/".to_owned()
                    } else {
                        home.to_owned()
                    }
                } else {
                    format!("{home}/{rest}")
                }
            }
            _ => text.to_owned(),
        }
    }

    // -- symbol collection --------------------------------------------------------

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

    // -- name resolution ------------------------------------------------------------

    fn qualify(&self, expr: &'a Expr, depth: usize, seen: &[&'a str]) -> Vec<String> {
        match expr {
            Expr::Name(name) => self.qualify_name(name.id.as_str(), depth, seen),
            Expr::Attribute(attribute) => self.qualify_attribute(attribute, depth, seen),
            Expr::Call(call) => self.qualify_call(call, depth, seen),
            _ => Vec::new(),
        }
    }

    fn qualify_name(&self, name: &'a str, depth: usize, seen: &[&'a str]) -> Vec<String> {
        // A name met on its own always resolves the same way, so it is resolved
        // once: a script that reassigns one name thousands of times stays cheap.
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

    // -- literal paths ---------------------------------------------------------------

    /// The path an expression always evaluates to, when it can be known statically.
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

    /// Whether the expression is, as far as can be told, a pathlib path object.
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

    // -- write checks ---------------------------------------------------------------

    fn check_write_path(&self, expr: &'a Expr, line: usize, construct: &str) {
        let Some(literal) = self.literal_path(expr, 0) else {
            self.warn(
                line,
                &format!("path not statically verifiable ({construct})"),
            );
            return;
        };
        let literal = self.expand_user(&literal);
        if !literal.starts_with('/') {
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

    // -- visitors -------------------------------------------------------------------

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

    /// Visit every node. The inner parts of a plain dotted chain are skipped: the
    /// outermost one is classified once, with its fully qualified name.
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

/// Whether an attribute chain ends in a plain name (`a.b.c`), memoised so that a
/// very long chain costs one pass instead of one pass per link.
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

/// Deepest bracket nesting of a script, from the tokens, without recursion.
/// `None` when the lexer does not finish within a bound that every token stream
/// of this length respects.
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

/// Run `analysis` on its own thread, with a stack sized for a script of
/// `script_len` bytes, and turn every way it can fail into a refusal.
///
/// A panic is caught, a stack that cannot be allocated is a refusal, and a script
/// too large for any safe stack is refused without being attempted. Taking the
/// analysis as a parameter lets tests prove that a panic never escapes.
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

/// Statically analyse a Python script; nothing is executed.
///
/// Whatever goes wrong while analysing, the answer is a refusal. A script that
/// does not parse is not refused: the application will report its own syntax
/// error.
#[must_use]
pub fn guard_script(script: &str, policy: &GuardPolicy<'_>) -> GuardReport {
    run_isolated(script.len(), || analyse(script, policy))
}

/// Message for a refused script: category, construct, line, and how to allow it.
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
