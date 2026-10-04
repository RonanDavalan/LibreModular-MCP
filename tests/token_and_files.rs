//! Token, private file and path confinement tests.
//!
//! Why this module exists
//! A weak permission check would silently expose the bridge token.

#[macro_use]
mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use common::{FILE_TOKEN, Verdict, current_uid, fresh, mode_of, must_fail, must_pass, write_file};
use libremodular_mcp::security::{
    ControlAccess, SecurityError, TRUNCATION_MARKER, bounded, check_script_size, check_target,
    create_token, ensure_private_dir, load_token_as, open_control_file, open_regular_nofollow,
    read_control_file, resolve_under,
};

type Loader<'l> = &'l dyn Fn(&Path, u32) -> Result<String, String>;

fn real_load(path: &Path, uid: u32) -> Result<String, String> {
    load_token_as(path, uid)
        .map(|token| token.as_str().to_owned())
        .map_err(|e| e.to_string())
}

fn lenient_load(path: &Path, _uid: u32) -> Result<String, String> {
    fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .map_err(|e| e.to_string())
}

fn check_refuses_loose_modes(load: Loader<'_>, base: &Path) -> Verdict {
    let base = fresh(base);
    for mode in [0o640, 0o644, 0o660, 0o604, 0o606] {
        let path = base.join(format!("token-{mode:o}"));
        write_file(&path, mode, FILE_TOKEN);
        ensure!(
            load(&path, current_uid()).is_err(),
            "mode {mode:o} was accepted"
        );
    }
    Ok(())
}

fn check_accepts_private_token(load: Loader<'_>, base: &Path) -> Verdict {
    let base = fresh(base);
    let path = base.join("token-ok");
    write_file(&path, 0o600, FILE_TOKEN);
    ensure!(
        load(&path, current_uid()).as_deref() == Ok(FILE_TOKEN),
        "a private token was refused or altered"
    );
    Ok(())
}

fn check_refuses_symlink(load: Loader<'_>, base: &Path) -> Verdict {
    let base = fresh(base);
    let real = base.join("real-token");
    write_file(&real, 0o600, FILE_TOKEN);
    let link = base.join("link-token");
    symlink(&real, &link).expect("symlink");
    ensure!(
        load(&link, current_uid()).is_err(),
        "symbolic link was followed"
    );
    Ok(())
}

fn check_refuses_foreign_owner(load: Loader<'_>, base: &Path) -> Verdict {
    let base = fresh(base);
    let path = base.join("token-owner");
    write_file(&path, 0o600, FILE_TOKEN);
    ensure!(
        load(&path, current_uid() + 1).is_err(),
        "foreign owner was accepted"
    );
    Ok(())
}

fn check_refuses_malformed(load: Loader<'_>, base: &Path) -> Verdict {
    let base = fresh(base);
    let too_long = format!("{FILE_TOKEN}00");
    let not_hex = "zz".repeat(32);
    for (index, text) in ["", "abc", not_hex.as_str(), too_long.as_str()]
        .iter()
        .enumerate()
    {
        let path = base.join(format!("token-bad-{index}"));
        write_file(&path, 0o600, text);
        ensure!(
            load(&path, current_uid()).is_err(),
            "malformed token {index} was accepted"
        );
    }
    Ok(())
}

#[test]
fn load_token_refusals_and_their_counter_proof() {
    let checks: [fn(Loader<'_>, &Path) -> Verdict; 4] = [
        check_refuses_loose_modes,
        check_refuses_symlink,
        check_refuses_foreign_owner,
        check_refuses_malformed,
    ];
    let base = tempfile::tempdir().expect("temporary directory");
    for check in checks {
        must_pass(check(&real_load, base.path()));
        must_fail(check(&lenient_load, base.path()));
    }
}

#[test]
fn load_token_accepts_a_private_token() {
    let base = tempfile::tempdir().expect("temporary directory");
    must_pass(check_accepts_private_token(&real_load, base.path()));
}

#[test]
fn load_token_error_never_contains_the_token() {
    let base = tempfile::tempdir().expect("temporary directory");
    let loose = base.path().join("loose");
    write_file(&loose, 0o644, FILE_TOKEN);
    let too_long = base.path().join("too-long");
    write_file(&too_long, 0o600, &format!("{FILE_TOKEN}00"));
    let wrong_owner = base.path().join("wrong-owner");
    write_file(&wrong_owner, 0o600, FILE_TOKEN);
    for (path, uid) in [
        (loose, current_uid()),
        (too_long, current_uid()),
        (wrong_owner, current_uid() + 1),
    ] {
        let error = load_token_as(&path, uid)
            .expect_err("must be refused")
            .to_string();
        assert!(!error.contains(FILE_TOKEN), "{error}");
    }
}

#[test]
fn the_token_type_never_shows_its_value() {
    let token = libremodular_mcp::security::Token::new("test-token-0000").expect("token");
    assert!(!format!("{token:?}").contains("test-token-0000"));
    assert!(libremodular_mcp::security::Token::new("").is_err());
}

// --- create_token --------------------------------------------------------------

type Creator<'c> = &'c dyn Fn(&Path, bool) -> Result<(), String>;

fn real_create(path: &Path, rotate: bool) -> Result<(), String> {
    create_token(path, rotate).map_err(|e| e.to_string())
}

fn naive_create(path: &Path, _rotate: bool) -> Result<(), String> {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let parent = path.parent().expect("parent");
    let _ = fs::create_dir(parent);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    fs::write(path, format!("{n:064x}")).map_err(|e| e.to_string())?;
    write_file(path, 0o644, &format!("{n:064x}"));
    Ok(())
}

fn check_create_does_not_overwrite(create: Creator<'_>, base: &Path) -> Verdict {
    let path = fresh(base).join("cfg").join("token");
    create(&path, false)?;
    let first = fs::read_to_string(&path).expect("token");
    let _ = create(&path, false);
    ensure!(
        fs::read_to_string(&path).expect("token") == first,
        "existing token was overwritten"
    );
    Ok(())
}

fn check_rotate_replaces_with_mode_600(create: Creator<'_>, base: &Path) -> Verdict {
    let path = fresh(base).join("cfg2").join("token");
    create(&path, false)?;
    let first = fs::read_to_string(&path).expect("token");
    create(&path, true)?;
    let second = fs::read_to_string(&path).expect("token");
    ensure!(second != first, "rotation kept the same token");
    ensure!(
        mode_of(&path) == 0o600,
        "token mode is {:o}",
        mode_of(&path)
    );
    let parent = path.parent().expect("parent");
    ensure!(
        mode_of(parent) == 0o700,
        "directory mode is {:o}",
        mode_of(parent)
    );
    ensure!(second.len() == 64, "token length is {}", second.len());
    Ok(())
}

#[test]
fn create_token_never_overwrites_without_rotate() {
    let base = tempfile::tempdir().expect("temporary directory");
    must_pass(check_create_does_not_overwrite(&real_create, base.path()));
    must_fail(check_create_does_not_overwrite(&naive_create, base.path()));
}

#[test]
fn create_token_rotate_replaces_and_stays_private() {
    let base = tempfile::tempdir().expect("temporary directory");
    must_pass(check_rotate_replaces_with_mode_600(
        &real_create,
        base.path(),
    ));
    must_fail(check_rotate_replaces_with_mode_600(
        &naive_create,
        base.path(),
    ));
}

#[test]
fn created_token_loads_back() {
    let base = tempfile::tempdir().expect("temporary directory");
    let path = base.path().join("cfg").join("token");
    create_token(&path, false).expect("create");
    let token = load_token_as(&path, current_uid()).expect("load");
    assert_eq!(token.as_str().len(), 64);
}

#[test]
fn rotation_leaves_no_staging_file_and_keeps_one_token() {
    let base = tempfile::tempdir().expect("temporary directory");
    let path = base.path().join("cfg").join("token");
    create_token(&path, false).expect("create");
    create_token(&path, true).expect("rotate");
    let names: Vec<_> = fs::read_dir(path.parent().expect("parent"))
        .expect("list")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(names, [std::ffi::OsString::from("token")]);
}

#[test]
fn refusing_to_overwrite_says_so_without_the_token() {
    let base = tempfile::tempdir().expect("temporary directory");
    let path = base.path().join("cfg").join("token");
    create_token(&path, false).expect("create");
    let existing = fs::read_to_string(&path).expect("token");
    let error = create_token(&path, false).expect_err("must refuse");
    assert!(matches!(error, SecurityError::TokenExists { .. }));
    assert!(!error.to_string().contains(&existing));
}

#[test]
fn create_token_refuses_a_directory_that_is_not_private() {
    let base = tempfile::tempdir().expect("temporary directory");
    let wide = base.path().join("wide");
    fs::create_dir(&wide).expect("dir");
    fs::set_permissions(&wide, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    assert!(create_token(&wide.join("token"), false).is_err());
    assert!(!wide.join("token").exists());
}

// --- private directories and file opening ----------------------------------------

#[test]
fn ensure_private_dir_refuses_wide_permissions() {
    let base = tempfile::tempdir().expect("temporary directory");
    let wide = base.path().join("wide");
    fs::create_dir(&wide).expect("dir");
    fs::set_permissions(&wide, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
    assert!(matches!(
        ensure_private_dir(&wide),
        Err(SecurityError::LooseMode { .. })
    ));
    let created = base.path().join("new").join("deeper");
    ensure_private_dir(&created).expect("create");
    assert_eq!(mode_of(&created), 0o700);
    assert_eq!(mode_of(created.parent().expect("parent")), 0o700);
}

#[test]
fn ensure_private_dir_refuses_a_symlink_a_file_and_a_foreign_owner() {
    let base = tempfile::tempdir().expect("temporary directory");
    let target = base.path().join("target");
    fs::create_dir(&target).expect("dir");
    fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .expect("chmod");
    let link = base.path().join("link");
    symlink(&target, &link).expect("symlink");
    assert!(matches!(
        ensure_private_dir(&link),
        Err(SecurityError::NotPlainDirectory { .. })
    ));
    let file = base.path().join("file");
    write_file(&file, 0o600, "x");
    assert!(ensure_private_dir(&file).is_err());
    assert!(matches!(
        libremodular_mcp::security::ensure_private_dir_as(&target, current_uid() + 1),
        Err(SecurityError::ForeignOwner { .. })
    ));
}

#[test]
fn open_regular_nofollow_refuses_links_and_directories() {
    let base = tempfile::tempdir().expect("temporary directory");
    let real = base.path().join("f");
    write_file(&real, 0o600, "x");
    let link = base.path().join("l");
    symlink(&real, &link).expect("symlink");
    assert!(matches!(
        open_regular_nofollow(&link, ControlAccess::Read),
        Err(SecurityError::SymlinkRefused { .. })
    ));
    assert!(matches!(
        open_regular_nofollow(base.path(), ControlAccess::Read),
        Err(SecurityError::NotRegularFile { .. })
    ));
    assert!(open_regular_nofollow(&real, ControlAccess::Read).is_ok());
}

#[test]
fn a_dangling_symlink_is_never_created_through() {
    let base = tempfile::tempdir().expect("temporary directory");
    let victim = base.path().join("victim");
    let link = base.path().join("journal");
    symlink(&victim, &link).expect("symlink");
    assert!(open_control_file(&link, ControlAccess::AppendOrCreate).is_err());
    assert!(open_control_file(&link, ControlAccess::CreateNew).is_err());
    assert!(!victim.exists(), "the open created the link target");
}

#[test]
fn control_files_are_created_private_whatever_the_umask_says() {
    let base = tempfile::tempdir().expect("temporary directory");
    let path = base.path().join("created");
    drop(open_control_file(&path, ControlAccess::CreateNew).expect("create"));
    assert_eq!(mode_of(&path), 0o600);
    assert!(matches!(
        open_control_file(&path, ControlAccess::CreateNew),
        Err(SecurityError::AlreadyExists { .. })
    ));
}

// --- read_control_file --------------------------------------------------------------

#[test]
fn read_control_file_returns_the_bytes_of_a_private_file() {
    let base = tempfile::tempdir().expect("temporary directory");
    let path = base.path().join("c");
    write_file(&path, 0o600, "hello");
    assert_eq!(read_control_file(&path, 16).expect("read"), b"hello");
}

#[test]
fn read_control_file_refuses_what_cannot_be_trusted() {
    let base = tempfile::tempdir().expect("temporary directory");
    let big = base.path().join("big");
    write_file(&big, 0o600, &"x".repeat(33));
    assert!(matches!(
        read_control_file(&big, 32),
        Err(SecurityError::TooLarge { .. })
    ));
    let exact = base.path().join("exact");
    write_file(&exact, 0o600, &"x".repeat(32));
    assert!(read_control_file(&exact, 32).is_ok());
    for mode in [0o640, 0o644, 0o660, 0o604] {
        let loose = base.path().join(format!("loose-{mode:o}"));
        write_file(&loose, mode, "x");
        assert!(matches!(
            read_control_file(&loose, 32),
            Err(SecurityError::LooseMode { .. })
        ));
    }
    assert!(matches!(
        read_control_file(base.path(), 32),
        Err(SecurityError::NotRegularFile { .. })
    ));
    let missing = base.path().join("missing");
    assert!(
        read_control_file(&missing, 32)
            .expect_err("missing")
            .is_not_found()
    );
}

// --- resolve_under ---------------------------------------------------------------

type Resolver<'r> = &'r dyn Fn(&Path, &str) -> Result<PathBuf, String>;

fn real_resolve(root: &Path, candidate: &str) -> Result<PathBuf, String> {
    resolve_under(root, Path::new(candidate)).map_err(|e| e.to_string())
}

fn naive_resolve(root: &Path, candidate: &str) -> Result<PathBuf, String> {
    Ok(root.join(candidate))
}

fn check_resolve_confines(resolve: Resolver<'_>, base: &Path) -> Verdict {
    let base = fresh(base);
    let root = base.join("root");
    fs::create_dir(&root).expect("root");
    let outside = base.join("outside");
    fs::create_dir(&outside).expect("outside");
    symlink(&outside, root.join("escape")).expect("symlink");
    write_file(&root.join("ok.txt"), 0o600, "x");
    let expected = fs::canonicalize(root.join("ok.txt")).expect("canonical");
    ensure!(
        resolve(&root, "ok.txt")? == expected,
        "a path inside the root was altered"
    );
    let absolute_outside = outside.join("x").to_string_lossy().into_owned();
    for bad in [
        "../outside/x",
        "escape/x",
        absolute_outside.as_str(),
        "sub/../../outside",
    ] {
        ensure!(resolve(&root, bad).is_err(), "{bad} escaped the root");
    }
    Ok(())
}

#[test]
fn resolve_under_confines_paths_and_has_a_counter_proof() {
    let base = tempfile::tempdir().expect("temporary directory");
    must_pass(check_resolve_confines(&real_resolve, base.path()));
    must_fail(check_resolve_confines(&naive_resolve, base.path()));
}

#[test]
fn resolve_under_keeps_a_not_yet_existing_path_inside_the_root() {
    let base = tempfile::tempdir().expect("temporary directory");
    let root = fs::canonicalize(base.path()).expect("canonical");
    let resolved = resolve_under(&root, &root.join("new").join("file.txt")).expect("inside");
    assert_eq!(resolved, root.join("new").join("file.txt"));
}

#[test]
fn resolve_under_follows_a_chain_of_links_and_refuses_a_loop() {
    let base = tempfile::tempdir().expect("temporary directory");
    let root = fs::canonicalize(base.path()).expect("canonical");
    fs::create_dir(root.join("work")).expect("work");
    fs::create_dir(root.join("elsewhere")).expect("elsewhere");
    symlink(root.join("elsewhere"), root.join("work").join("hop")).expect("hop");
    assert!(resolve_under(&root.join("work"), Path::new("hop/file")).is_err());
    symlink(root.join("work").join("b"), root.join("work").join("a")).expect("a");
    symlink(root.join("work").join("a"), root.join("work").join("b")).expect("b");
    assert!(resolve_under(&root.join("work"), Path::new("a/file")).is_err());
}

#[test]
fn resolve_under_needs_an_absolute_root() {
    assert!(resolve_under(Path::new("relative"), Path::new("x")).is_err());
}

// --- targets, sizes, bounds -----------------------------------------------------------

#[test]
fn check_target_is_exact() {
    let active = ["gimp", "libreoffice-calc"];
    assert!(check_target("gimp", active).is_ok());
    for bad in ["GIMP", " gimp", "gimp ", "gimp\n", "libreoffice", "", "hub"] {
        let error = check_target(bad, active).expect_err(bad).to_string();
        assert!(
            error.contains("gimp") && error.contains("libreoffice-calc"),
            "{error}"
        );
    }
    let error = check_target("gimp", [])
        .expect_err("none active")
        .to_string();
    assert!(error.contains("none"));
}

#[test]
fn check_script_size_uses_bytes() {
    assert_eq!(
        check_script_size(&"a".repeat(1024), 1024).expect("fits"),
        1024
    );
    assert!(matches!(
        check_script_size(&"é".repeat(600), 1024),
        Err(SecurityError::ScriptTooLarge {
            size: 1200,
            limit: 1024
        })
    ));
}

#[test]
fn bounded_marks_truncation_and_respects_the_limit() {
    let (text, cut) = bounded(&"é".repeat(100), 50).expect("bounded");
    assert!(cut && text.ends_with(TRUNCATION_MARKER));
    assert!(text.len() <= 50);
    assert_eq!(
        bounded("short", 50).expect("bounded"),
        ("short".to_owned(), false)
    );
    assert!(matches!(
        bounded("long enough text", 4),
        Err(SecurityError::LimitTooSmall { .. })
    ));
}

#[test]
fn bounded_never_splits_a_character_whatever_the_limit() {
    for text in ["é".repeat(40), "日本語".repeat(20), "a€b".repeat(30)] {
        for limit in TRUNCATION_MARKER.len()..80 {
            let (cut, _) = bounded(&text, limit).expect("bounded");
            assert!(cut.len() <= limit, "{limit}: {} bytes", cut.len());
        }
    }
}

// --- private work files ------------------------------------------------------------

type Replacer<'a> = &'a dyn Fn(&Path, &[u8]) -> Result<(), String>;

/// Writing over a symbolic link must replace the link with a private regular
/// file and leave the link's target untouched.
fn check_replace_never_writes_through_a_link(replace: Replacer<'_>, base: &Path) -> Verdict {
    let dir = fresh(base);
    let outside = dir.join("outside.txt");
    write_file(&outside, 0o644, "untouched");
    let path = dir.join("script.py");
    symlink(&outside, &path).map_err(|e| e.to_string())?;
    replace(&path, b"print('new')")?;
    ensure!(
        fs::read_to_string(&outside).map_err(|e| e.to_string())? == "untouched",
        "the target of the link was written"
    );
    let info = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    ensure!(info.file_type().is_file(), "the link is still there");
    ensure!(mode_of(&path) == 0o600, "mode {:o}", mode_of(&path));
    ensure!(
        fs::read_to_string(&path).map_err(|e| e.to_string())? == "print('new')",
        "the new content is missing"
    );
    Ok(())
}

#[test]
fn replacing_a_private_file_removes_a_link_instead_of_following_it() {
    let base = common::private_tempdir();
    must_pass(check_replace_never_writes_through_a_link(
        &|path, bytes| {
            libremodular_mcp::security::replace_private_file(path, bytes).map_err(|e| e.to_string())
        },
        base.path(),
    ));
    must_fail(check_replace_never_writes_through_a_link(
        &|path, bytes| fs::write(path, bytes).map_err(|e| e.to_string()),
        base.path(),
    ));
}

#[test]
fn ensuring_a_private_file_creates_it_once_and_refuses_links_and_loose_modes() {
    use libremodular_mcp::security::ensure_private_file;
    let base = common::private_tempdir();
    let dir = fresh(base.path());
    let document = dir.join("current.svg");
    ensure_private_file(&document, b"<svg/>").expect("created");
    assert_eq!(mode_of(&document), 0o600);
    ensure_private_file(&document, b"other").expect("kept");
    assert_eq!(fs::read_to_string(&document).expect("read"), "<svg/>");
    let loose = dir.join("loose.svg");
    write_file(&loose, 0o644, "<svg/>");
    assert!(ensure_private_file(&loose, b"<svg/>").is_err());
    let link = dir.join("link.svg");
    symlink(&document, &link).expect("link");
    assert!(ensure_private_file(&link, b"<svg/>").is_err());
}
