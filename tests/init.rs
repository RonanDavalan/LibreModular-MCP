//! Tests of the init command.
//!
//! Why this module exists
//! The operator's first command creates the token and must do it safely.

#[macro_use]
mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

use common::{current_uid, mode_of, write_file};
use libremodular_mcp::cli::{launch_run, run_init, run_with};
use libremodular_mcp::config::{self, Paths, load};
use libremodular_mcp::security::load_token_as;

fn paths_in(base: &Path) -> Paths {
    let overrides: BTreeMap<&str, OsString> = BTreeMap::from([
        ("HOME", OsString::from(base)),
        ("LIBREMODULAR_CONFIG_DIR", base.join("c").into_os_string()),
        ("LIBREMODULAR_STATE_DIR", base.join("s").into_os_string()),
    ]);
    config::resolve_paths(|name| overrides.get(name).cloned()).expect("paths")
}

fn init(paths: &Paths, rotate: bool) -> (u8, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run_init(paths, rotate, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).expect("utf-8"),
        String::from_utf8(err).expect("utf-8"),
    )
}

fn token_text(paths: &Paths) -> String {
    fs::read_to_string(paths.token_file()).expect("token")
}

#[test]
fn init_creates_everything_with_private_modes_and_never_shows_the_token() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    let (code, text, err) = init(&paths, false);
    assert_eq!(code, 0, "{err}");
    assert_eq!(mode_of(&paths.config_dir), 0o700);
    assert_eq!(mode_of(&paths.state_dir), 0o700);
    assert_eq!(mode_of(&paths.config_file()), 0o600);
    assert_eq!(mode_of(&paths.token_file()), 0o600);
    let token = load_token_as(&paths.token_file(), current_uid()).expect("token loads back");
    assert!(!text.contains(token.as_str()) && !err.contains(token.as_str()));
    assert!(text.contains(&paths.token_file().display().to_string()));
}

#[test]
fn the_example_configuration_written_by_init_is_valid_and_enables_nothing() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    init(&paths, false);
    let config = load(&paths.config_file()).expect("example configuration");
    assert!(config.active_names().is_empty());
    assert_eq!(config.apps().len(), config::APP_DEFAULTS.len());
    assert!(
        !fs::read_to_string(paths.config_file())
            .expect("config")
            .to_lowercase()
            .contains("host")
    );
}

#[test]
fn second_run_changes_nothing_and_says_so() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    init(&paths, false);
    let token_before = token_text(&paths);
    fs::write(paths.config_file(), "[hub]\nmax_timeout_s = 60\n").expect("edit");
    let (code, text, _) = init(&paths, false);
    assert_eq!(code, 0);
    assert!(text.contains("unchanged"));
    assert_eq!(token_text(&paths), token_before);
    assert_eq!(
        fs::read_to_string(paths.config_file()).expect("config"),
        "[hub]\nmax_timeout_s = 60\n"
    );
}

#[test]
fn rotate_changes_the_token_and_warns_about_bridges() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    init(&paths, false);
    let old = token_text(&paths);
    let (code, text, err) = init(&paths, true);
    let new = token_text(&paths);
    assert_eq!(code, 0, "{err}");
    assert!(new != old && mode_of(&paths.token_file()) == 0o600);
    assert!(text.contains("restart every bridge"));
    assert!(!text.contains(&old) && !text.contains(&new));
}

#[test]
fn init_refuses_an_existing_token_with_wide_permissions_and_rotate_repairs_it() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    init(&paths, false);
    let loose = token_text(&paths);
    fs::set_permissions(
        paths.token_file(),
        std::os::unix::fs::PermissionsExt::from_mode(0o644),
    )
    .expect("chmod");
    let (code, text, err) = init(&paths, false);
    assert_eq!(code, 1);
    assert!(!text.contains(&loose) && !err.contains(&loose));
    let (code, _, _) = init(&paths, true);
    assert_eq!(code, 0);
    assert_eq!(mode_of(&paths.token_file()), 0o600);
}

#[test]
fn init_refuses_a_configuration_directory_that_is_not_private() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    fs::create_dir_all(&paths.config_dir).expect("dirs");
    fs::set_permissions(
        &paths.config_dir,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .expect("chmod");
    let (code, _, err) = init(&paths, false);
    assert_eq!(code, 1);
    assert!(err.contains("chmod 700"), "{err}");
    assert!(!paths.token_file().exists());
}

#[test]
fn init_does_not_overwrite_an_existing_configuration_with_wide_permissions() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    init(&paths, false);
    fs::set_permissions(
        paths.config_file(),
        std::os::unix::fs::PermissionsExt::from_mode(0o644),
    )
    .expect("chmod");
    write_file(&paths.config_file(), 0o644, "[hub]\nmax_timeout_s = 99\n");
    let (code, _, _) = init(&paths, false);
    assert_eq!(code, 0, "an existing configuration is left alone");
    assert_eq!(
        fs::read_to_string(paths.config_file()).expect("config"),
        "[hub]\nmax_timeout_s = 99\n"
    );
    assert!(
        load(&paths.config_file()).is_err(),
        "and the Hub refuses to load it"
    );
}

fn run_cli(args: &[&str], paths: &Paths) -> (u8, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let located = paths.clone();
    let code = run_with(
        args.iter().map(OsString::from),
        &mut out,
        &mut err,
        &move || Ok(located.clone()),
    );
    (
        code,
        String::from_utf8(out).expect("utf-8"),
        String::from_utf8(err).expect("utf-8"),
    )
}

#[test]
fn help_option_prints_help() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    let (code, out, _) = run_cli(&["libremodular-mcp", "--help"], &paths);
    assert_eq!(code, 0);
    assert!(out.contains("init"));
    assert!(out.contains("serve"));
    assert!(
        !paths.config_dir.exists(),
        "printing help must not create anything"
    );
}

#[test]
fn main_without_command_serves_and_refuses_to_start_without_a_token() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    let (code, out, err) = run_cli(&["libremodular-mcp"], &paths);
    assert_eq!(code, 1);
    assert!(out.is_empty(), "nothing but the protocol on the standard output");
    assert!(err.contains("token"), "{err}");
    assert!(!paths.config_dir.exists(), "a refused start creates nothing");
}

#[test]
fn the_init_command_line_is_wired_to_the_given_locations() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    let (code, _, err) = run_cli(&["libremodular-mcp", "init"], &paths);
    assert_eq!(code, 0, "{err}");
    assert!(paths.token_file().exists());
    let before = token_text(&paths);
    let (code, _, _) = run_cli(&["libremodular-mcp", "init", "--rotate"], &paths);
    assert_eq!(code, 0);
    assert_ne!(token_text(&paths), before);
}

#[test]
fn an_unknown_argument_is_a_usage_error_on_the_error_stream() {
    let base = common::private_tempdir();
    let paths = paths_in(base.path());
    let (code, out, err) = run_cli(&["libremodular-mcp", "init", "--bogus"], &paths);
    assert_eq!(code, 2);
    assert!(out.is_empty() && !err.is_empty());
    assert!(!paths.config_dir.exists());
}

#[test]
fn launch_reports_that_no_application_is_supported() {
    let mut err = Vec::new();
    assert_eq!(launch_run(&mut err), 2);
    assert!(
        String::from_utf8(err)
            .expect("utf-8")
            .contains("no application")
    );
}
