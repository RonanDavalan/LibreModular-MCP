//! Tests de la configuration et de la résolution des chemins.
//!
//! Pourquoi ce module existe
//! Un mauvais fichier doit arrêter le Hub plutôt qu'affaiblir une protection.

#[macro_use]
mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use common::{Verdict, must_fail, must_pass, write_file};
use libremodular_mcp::cli::example_config_text;
use libremodular_mcp::config::{
    Config, ConfigError, Limits, Mode, Paths, Rejection, load, resolve_paths,
};

fn parse(text: &str) -> Result<Config, ConfigError> {
    Config::from_bytes(text.as_bytes())
}

#[test]
fn absent_file_gives_strict_defaults_and_nothing_enabled() {
    let base = tempfile::tempdir().expect("temporary directory");
    let config = load(&base.path().join("config.toml")).expect("defaults");
    assert!(config.active_names().is_empty());
    assert_eq!(*config.limits(), Limits::default());
    assert!(!config.security().allow_dangerous_scripts);
    assert!(!config.security().audit_script_text);
    assert!(config.security().extra_write_roots.is_empty());
    assert_eq!(config.app("gimp").and_then(|a| a.port), Some(9301));
    assert_eq!(config.app("krita").and_then(|a| a.port), Some(9302));
    assert_eq!(config.app("blender").and_then(|a| a.port), Some(9303));
    assert_eq!(
        config.app("libreoffice-calc").and_then(|a| a.uno_port),
        Some(2002)
    );
    assert_eq!(
        config.app("libreoffice-calc").and_then(|a| a.bridge_port),
        Some(9304)
    );
    assert_eq!(config.app("inkscape").map(|a| a.mode), Some(Mode::Cli));
    assert_eq!(config.apps().len(), 7);
    assert!(config.apps().iter().all(|a| !a.enabled));
}

#[test]
fn the_documented_limits_are_the_defaults() {
    let limits = Limits::default();
    assert_eq!(limits.max_script_bytes, 262_144);
    assert_eq!(limits.max_output_bytes, 1_048_576);
    assert_eq!(limits.max_line_bytes, 8_388_608);
    assert_eq!(limits.default_timeout_s, 30);
    assert_eq!(limits.max_timeout_s, 300);
}

#[test]
fn values_are_read_and_applications_activate_in_registry_order() {
    let config = parse(
        "[hub]\nmax_timeout_s = 120\n[apps.gimp]\nenabled = true\nport = 9400\n\
         [apps.libreoffice-calc]\nenabled = true\n[security]\nextra_write_roots = ['/home/alice/exports']\n",
    )
    .expect("valid");
    assert_eq!(config.limits().max_timeout_s, 120);
    assert_eq!(config.active_names(), ["gimp", "libreoffice-calc"]);
    assert_eq!(config.app("gimp").and_then(|a| a.port), Some(9400));
    assert_eq!(config.security().extra_write_roots, ["/home/alice/exports"]);
}

const INVALID: [&str; 22] = [
    "[hub]\nbogus = 1\n",
    "[bogus]\nx = 1\n",
    "[apps.nosuchapp]\nenabled = true\n",
    "[apps.gimp]\nhost = '0.0.0.0'\n",
    "[apps.gimp]\nenabled = 'yes'\n",
    "[apps.gimp]\nmode = 'ssh'\n",
    "[apps.gimp]\nport = 80\n",
    "[apps.gimp]\nport = 70000\n",
    "[apps.gimp]\nport = true\n",
    "[apps.libreoffice-calc]\nuno_port = 22\n",
    "[apps.libreoffice-calc]\nbridge_port = 99999\n",
    "[hub]\nmax_timeout_s = 5\ndefault_timeout_s = 30\n",
    "[hub]\nmax_script_bytes = 'big'\n",
    "[hub]\nmax_output_bytes = 10\n",
    "[hub]\ndefault_timeout_s = 0\n",
    "[hub]\nmax_line_bytes = -5\n",
    "[security]\nallow_dangerous_scripts = 1\n",
    "[security]\nextra_write_roots = ['relative/dir']\n",
    "[security]\nextra_write_roots = ['']\n",
    "[security]\nextra_write_roots = 'x'\n",
    "this is not toml",
    "[hub]\nmax_script_bytes = 2048\nmax_script_bytes = 4096\n",
];

#[test]
fn invalid_configuration_fails_closed() {
    for text in INVALID {
        assert!(parse(text).is_err(), "{text:?} was accepted");
    }
}

#[test]
fn a_file_that_is_not_utf8_is_refused() {
    assert!(matches!(
        Config::from_bytes(&[0x5b, 0xff, 0xfe, 0x5d]),
        Err(ConfigError::NotUtf8)
    ));
}

#[test]
fn errors_name_the_kind_and_the_position_but_never_a_value_or_a_key_from_the_file() {
    let secret = "hunter2-secret-value";
    let cases = [
        format!("[hub]\nmax_script_bytes = '{secret}'\n"),
        format!("[hub]\n{secret} = 1\n"),
        format!("[apps.{secret}]\nenabled = true\n"),
        format!("[{secret}]\nx = 1\n"),
        format!("[security]\nextra_write_roots = ['{secret}']\n"),
        format!("[apps.gimp]\nmode = '{secret}'\n"),
        format!("this is {secret} not toml"),
        format!("[hub]\nmax_timeout_s = 1\ndefault_timeout_s = 5\n# {secret}\n"),
    ];
    for text in cases {
        let error = parse(&text).expect_err("must be refused").to_string();
        assert!(!error.contains(secret), "{error}");
        assert!(!error.contains("hunter2"), "{error}");
    }
    let error = parse("[hub]\nbogus = 1\n").expect_err("unknown key");
    assert!(matches!(
        error,
        ConfigError::Rejected {
            kind: Rejection::UnknownKey,
            position: Some((2, 1))
        }
    ));
    let error = parse("[hub]\nmax_script_bytes = 'big'\n").expect_err("wrong type");
    assert!(matches!(
        error,
        ConfigError::Rejected {
            kind: Rejection::WrongType,
            position: Some((2, _))
        }
    ));
}

#[test]
fn no_host_key_exists_anywhere() {
    assert!(!example_config_text().to_lowercase().contains("host"));
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config.rs"))
        .expect("config source");
    assert!(!source.to_lowercase().contains("host"));
    assert!(parse("[apps.gimp]\nhost = '127.0.0.1'\n").is_err());
}

#[test]
fn the_example_configuration_is_valid_and_enables_nothing() {
    let config = parse(&example_config_text()).expect("example configuration");
    assert!(config.active_names().is_empty());
    assert_eq!(config.apps().len(), 7);
}

// --- le fichier de configuration est un fichier de contrôle -------------------------

type ConfigLoader<'l> = &'l dyn Fn(&Path) -> Result<Config, String>;

fn real_loader(path: &Path) -> Result<Config, String> {
    load(path).map_err(|e| e.to_string())
}

fn lenient_loader(path: &Path) -> Result<Config, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    Config::from_bytes(&bytes).map_err(|e| e.to_string())
}

fn check_refuses_untrusted_files(load: ConfigLoader<'_>, base: &Path) -> Verdict {
    let base = common::fresh(base);
    let valid = "[hub]\nmax_timeout_s = 60\n";
    for mode in [0o640, 0o644, 0o660, 0o604] {
        let path = base.join(format!("loose-{mode:o}.toml"));
        write_file(&path, mode, valid);
        ensure!(load(&path).is_err(), "mode {mode:o} was accepted");
    }
    let real = base.join("real.toml");
    write_file(&real, 0o600, valid);
    let link = base.join("link.toml");
    symlink(&real, &link).expect("symlink");
    ensure!(load(&link).is_err(), "a symbolic link was followed");
    let directory = base.join("a-directory");
    fs::create_dir(&directory).expect("dir");
    ensure!(load(&directory).is_err(), "a directory was accepted");
    let huge = base.join("huge.toml");
    write_file(
        &huge,
        0o600,
        &format!("# {}\n", "x".repeat(1024 * 1024 + 1)),
    );
    ensure!(load(&huge).is_err(), "an oversized file was accepted");
    Ok(())
}

fn check_accepts_a_private_configuration(load: ConfigLoader<'_>, base: &Path) -> Verdict {
    let base = common::fresh(base);
    let path = base.join("config.toml");
    write_file(&path, 0o600, "[hub]\nmax_timeout_s = 60\n");
    let config = load(&path)?;
    ensure!(
        config.limits().max_timeout_s == 60,
        "the value was not read"
    );
    Ok(())
}

#[test]
fn the_configuration_file_is_refused_unless_private_regular_and_small() {
    let base = tempfile::tempdir().expect("temporary directory");
    must_pass(check_refuses_untrusted_files(&real_loader, base.path()));
    must_fail(check_refuses_untrusted_files(&lenient_loader, base.path()));
    must_pass(check_accepts_a_private_configuration(
        &real_loader,
        base.path(),
    ));
}

#[test]
fn a_rejected_configuration_file_does_not_echo_its_content() {
    let base = tempfile::tempdir().expect("temporary directory");
    let secret = "do-not-print-this";
    let unknown = base.path().join("unknown.toml");
    write_file(&unknown, 0o600, &format!("[hub]\n{secret} = 1\n"));
    let loose = base.path().join("loose.toml");
    write_file(&loose, 0o644, &format!("# {secret}\n"));
    let broken = base.path().join("broken.toml");
    write_file(&broken, 0o600, &format!("{secret} = = ="));
    for path in [unknown, loose, broken] {
        let error = load(&path).expect_err("refused").to_string();
        assert!(!error.contains(secret), "{error}");
    }
}

#[test]
fn a_configuration_file_that_is_a_dangling_link_is_not_treated_as_absent() {
    let base = tempfile::tempdir().expect("temporary directory");
    let link = base.path().join("config.toml");
    symlink(base.path().join("nowhere"), &link).expect("symlink");
    assert!(load(&link).is_err());
}

// --- emplacements -------------------------------------------------------------------

fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: BTreeMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), OsString::from(*v)))
        .collect();
    move |name| map.get(name).cloned()
}

fn paths(pairs: &[(&str, &str)]) -> Result<Paths, ConfigError> {
    resolve_paths(lookup(pairs))
}

#[test]
fn xdg_paths_and_fallbacks() {
    let fallback = paths(&[("HOME", "/home/alice")]).expect("paths");
    assert_eq!(
        fallback.config_dir,
        Path::new("/home/alice/.config/libremodular-mcp")
    );
    assert_eq!(
        fallback.state_dir,
        Path::new("/home/alice/.local/state/libremodular-mcp")
    );
    assert_eq!(fallback.runtime_dir, None);
    assert!(fallback.require_runtime_dir().is_err());
    let explicit = paths(&[
        ("HOME", "/home/alice"),
        ("XDG_CONFIG_HOME", "/x/c"),
        ("XDG_STATE_HOME", "/x/s"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
    ])
    .expect("paths");
    assert_eq!(
        explicit.token_file(),
        Path::new("/x/c/libremodular-mcp/token")
    );
    assert_eq!(
        explicit.config_file(),
        Path::new("/x/c/libremodular-mcp/config.toml")
    );
    assert_eq!(
        explicit.audit_file(),
        Path::new("/x/s/libremodular-mcp/audit.jsonl")
    );
    assert_eq!(
        explicit.require_runtime_dir().expect("runtime"),
        Path::new("/run/user/1000/libremodular-mcp")
    );
}

#[test]
fn relative_xdg_values_are_ignored_and_relative_overrides_refused() {
    let relative = paths(&[("HOME", "/home/alice"), ("XDG_RUNTIME_DIR", "run")]).expect("paths");
    assert_eq!(relative.runtime_dir, None);
    let ignored =
        paths(&[("HOME", "/home/alice"), ("XDG_CONFIG_HOME", "relative")]).expect("paths");
    assert_eq!(
        ignored.config_dir,
        Path::new("/home/alice/.config/libremodular-mcp")
    );
    assert!(paths(&[("HOME", "/home/alice"), ("LIBREMODULAR_CONFIG_DIR", "rel")]).is_err());
}

#[test]
fn overrides_replace_the_base_directories() {
    let overridden = paths(&[
        ("HOME", "/home/alice"),
        ("LIBREMODULAR_CONFIG_DIR", "/t/c"),
        ("LIBREMODULAR_STATE_DIR", "/t/s"),
        ("LIBREMODULAR_RUNTIME_DIR", "/t/r"),
        ("XDG_CONFIG_HOME", "/ignored"),
    ])
    .expect("paths");
    assert_eq!(overridden.config_dir, Path::new("/t/c/libremodular-mcp"));
    assert_eq!(overridden.state_dir, Path::new("/t/s/libremodular-mcp"));
    assert_eq!(
        overridden.runtime_dir.as_deref(),
        Some(Path::new("/t/r/libremodular-mcp"))
    );
}

#[test]
fn the_home_directory_is_only_needed_when_a_fallback_is_needed() {
    let no_home = paths(&[("XDG_CONFIG_HOME", "/x/c"), ("XDG_STATE_HOME", "/x/s")]).expect("paths");
    assert_eq!(no_home.config_dir, Path::new("/x/c/libremodular-mcp"));
    assert!(paths(&[]).is_err());
    assert!(paths(&[("HOME", "relative/home")]).is_err());
    assert!(paths(&[("HOME", "")]).is_err());
}

#[test]
fn a_missing_runtime_directory_never_falls_back_to_a_temporary_one() {
    let resolved = paths(&[("HOME", "/home/alice"), ("TMPDIR", "/tmp")]).expect("paths");
    assert_eq!(resolved.runtime_dir, None);
}

#[test]
fn deeply_nested_values_in_the_file_are_an_error_not_a_crash() {
    let arrays = format!("a = {}1{}\n", "[".repeat(100_000), "]".repeat(100_000));
    let tables = format!("a = {}1{}\n", "{b=".repeat(25_000), "}".repeat(25_000));
    for text in [arrays, tables] {
        let error = parse(&text).expect_err("must be refused").to_string();
        assert!(
            error.len() < 200,
            "the error repeats the file: {} bytes",
            error.len()
        );
    }
}
