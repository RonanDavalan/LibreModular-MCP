//! Shared helpers: obviously fake tokens, temporary directories, and the
//! counter-proof machinery.
//!
//! Why this module exists
//! Tests must never touch the real configuration or a real token, and every
//! security check needs a way to prove it can fail: a check is a function that
//! returns a verdict, it must pass on the real implementation and fail on a
//! deliberately defective one.

#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const FAKE_TOKEN: &str = "test-token-0000";
/// A well-formed (64 hexadecimal characters) and obviously fake token.
pub const FILE_TOKEN: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const WORK_DIR: &str = "/run/user/1000/libremodular-mcp/work/gimp";

/// The result of a check: `Err` carries what was wrong.
pub type Verdict = Result<(), String>;

#[macro_export]
macro_rules! ensure {
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err(format!($($message)+));
        }
    };
}

/// The check must accept the real implementation.
pub fn must_pass(verdict: Verdict) {
    if let Err(message) = verdict {
        panic!("{message}");
    }
}

/// Counter-proof: a check that never fails on a defective implementation proves nothing.
pub fn must_fail(verdict: Verdict) {
    assert!(
        verdict.is_err(),
        "the check did not detect the defective implementation"
    );
}

/// A new empty directory, so one check can run against several implementations.
pub fn fresh(base: &Path) -> PathBuf {
    let path = base.join(format!("case-{}", unique()));
    fs::create_dir(&path).expect("create a fresh directory");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("make it private");
    path
}

/// A temporary directory that is private whatever the umask of the test process
/// is: the Hub refuses to put its files anywhere else.
pub fn private_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temporary directory");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).expect("make it private");
    dir
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

pub fn write_file(path: &Path, mode: u32, text: &str) {
    fs::write(path, text).expect("write a test file");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set the test file mode");
}

pub fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("stat")
        .permissions()
        .mode()
        & 0o7777
}

pub fn current_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}
