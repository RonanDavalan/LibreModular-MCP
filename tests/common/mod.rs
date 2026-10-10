//! Assistants partagés : jetons manifestement faux, répertoires temporaires, et la mécanique de
//! contre-preuve.
//!
//! Pourquoi ce module existe
//! Les tests ne doivent jamais toucher la vraie configuration ni un vrai jeton, et chaque contrôle de
//! sécurité a besoin d'un moyen de prouver qu'il peut échouer : un contrôle est une fonction qui rend
//! un verdict, elle doit réussir sur l'implémentation réelle et échouer sur une implémentation
//! défectueuse exprès.

#![allow(dead_code)]

pub mod binary;
pub mod fake_bridge;
pub mod hub;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const FAKE_TOKEN: &str = "test-token-0000";
/// Un jeton bien formé (64 caractères hexadécimaux) et manifestement faux.
pub const FILE_TOKEN: &str = "0000000000000000000000000000000000000000000000000000000000000000";
pub const WORK_DIR: &str = "/run/user/1000/libremodular-mcp/work/gimp";

/// Le résultat d'un contrôle : `Err` porte ce qui n'allait pas.
pub type Verdict = Result<(), String>;

#[macro_export]
macro_rules! ensure {
    ($condition:expr, $($message:tt)+) => {
        if !$condition {
            return Err(format!($($message)+));
        }
    };
}

/// Le contrôle doit accepter l'implémentation réelle.
pub fn must_pass(verdict: Verdict) {
    if let Err(message) = verdict {
        panic!("{message}");
    }
}

/// Contre-preuve : un contrôle qui n'échoue jamais sur une implémentation défectueuse ne prouve rien.
pub fn must_fail(verdict: Verdict) {
    assert!(
        verdict.is_err(),
        "the check did not detect the defective implementation"
    );
}

/// Un nouveau répertoire vide, pour qu'un contrôle puisse s'exécuter contre plusieurs implémentations.
pub fn fresh(base: &Path) -> PathBuf {
    let path = base.join(format!("case-{}", unique()));
    fs::create_dir(&path).expect("create a fresh directory");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("make it private");
    path
}

/// Un répertoire temporaire privé quel que soit le umask du processus de test : le Hub refuse de
/// poser ses fichiers ailleurs.
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
