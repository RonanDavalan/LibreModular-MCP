//! The launcher of LibreOffice and its bridge, with fake programs and a fake `/proc`.
//!
//! Why this module exists
//! The launcher starts two programs and later stops one of them, on the word of a
//! file it wrote. These tests show that nothing is started through a shell or next
//! to a LibreOffice of the operator's, that the token never reaches a child, that
//! the environment of the bridge is exactly the listed one, and that a process
//! number taken by another program is never signalled. Nothing here depends on a
//! LibreOffice, a `uno` or a Python package being installed, and the processes of
//! the machine are never looked at, apart from the two tests that say so.

#[macro_use]
mod common;

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use common::{FAKE_TOKEN, FILE_TOKEN, Verdict, must_fail, must_pass, write_file};
use libremodular_mcp::config::{Config, ConfigError, Paths};
use libremodular_mcp::launch::{
    LaunchError, Plan, Programs, Record, accept_argument, bridge_environment, bridge_spawn,
    is_the_bridge, launch, plan_from_config, run, running_soffice, soffice_spawn, stop,
};
use rustix::process::{Pid, Signal, kill_process, test_kill_process};
use serde_json::Value;

const UNO_PORT: u16 = 2002;
const BRIDGE_PORT: u16 = 9304;

static PROCESS_START: Mutex<()> = Mutex::new(());

/// Held while a fake program is written and while any process is started. A process
/// started in one test copies, until its `exec`, every file another test has open for
/// writing; running that file then fails with "Text file busy". Tests share one process,
/// so they take turns.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    PROCESS_START
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The first `python3` on the test's PATH: the interpreter of the fake programs.
fn python() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .map(|dir| dir.join("python3"))
        .find(|candidate| candidate.is_file())
        .expect("python3 is required to test the launcher")
}

/// A fake program: it writes what it received to `report` and stays alive, so that
/// it can be signalled, until the directory of the test is removed. `/proc/self/environ` holds the environment exactly as it
/// was given, whatever the interpreter did to its own copy afterwards.
fn write_fake_program(path: &Path, report: &Path) {
    let text = format!(
        "#!{python}\n\
         import json, os, sys, time\n\
         environ = open('/proc/self/environ', 'rb').read().split(b'\\0')\n\
         variables = sorted(item.decode() for item in environ if item)\n\
         streams = [os.readlink('/proc/self/fd/%d' % number) for number in (0, 1, 2)]\n\
         record = {{'argv': sys.argv[1:], 'environment': variables, 'pid': os.getpid(),\n\
                   'group': os.getpgid(0), 'streams': streams, 'cwd': os.getcwd()}}\n\
         with open('{report}.tmp', 'w') as handle:\n\
         \x20   json.dump(record, handle)\n\
         os.rename('{report}.tmp', '{report}')\n\
         deadline = time.time() + 60\n\
         while os.path.exists('{directory}') and time.time() < deadline:\n\
         \x20   time.sleep(0.1)\n",
        python = python().display(),
        report = report.display(),
        directory = report.parent().expect("directory").display(),
    );
    let _turn = one_at_a_time();
    fs::write(path, text).expect("fake program");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("mode 700");
}

/// What the fake programs are given, and the directories around them.
struct Setup {
    _base: tempfile::TempDir,
    root: PathBuf,
    paths: Paths,
    programs: Programs,
    bridge_file: PathBuf,
    soffice_report: PathBuf,
    bridge_report: PathBuf,
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).expect("directory");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("mode");
}

fn setup() -> Setup {
    let base = common::private_tempdir();
    let root = base.path().to_path_buf();
    let config_dir = root.join("config");
    let state_dir = root.join("state");
    let runtime_dir = root.join("run").join("libremodular-mcp");
    let proc_dir = root.join("proc");
    for dir in [&config_dir, &state_dir, &proc_dir, &root.join("bin")] {
        private_dir(dir);
    }
    write_file(&config_dir.join("token"), 0o600, FILE_TOKEN);
    let soffice_report = root.join("soffice.json");
    let bridge_report = root.join("bridge.json");
    let soffice = root.join("bin").join("soffice");
    let python_program = root.join("bin").join("python3");
    write_fake_program(&soffice, &soffice_report);
    write_fake_program(&python_program, &bridge_report);
    let bridge_file = root.join("uno_bridge.py");
    write_file(
        &bridge_file,
        0o644,
        "# the bridge, which the fake python never reads\n",
    );
    Setup {
        paths: Paths {
            config_dir,
            state_dir,
            runtime_dir: Some(runtime_dir),
        },
        programs: Programs {
            soffice,
            python: python_program,
            proc_dir,
        },
        bridge_file,
        soffice_report,
        bridge_report,
        root,
        _base: base,
    }
}

impl Setup {
    fn config(&self, text: &str) -> Config {
        Config::from_bytes(text.as_bytes()).expect("test configuration")
    }

    fn calc_only(&self) -> Config {
        self.config("[apps.libreoffice-calc]\nenabled = true\n")
    }

    fn plan(&self) -> Plan {
        plan_from_config(
            &self.calc_only(),
            self.bridge_file.clone(),
            self.paths.token_file(),
        )
        .expect("plan")
    }

    fn record_file(&self) -> PathBuf {
        self.paths
            .runtime_dir
            .clone()
            .expect("runtime")
            .join("launch")
            .join("libreoffice.pids")
    }

    fn launch(&self) -> Result<String, LaunchError> {
        let _turn = one_at_a_time();
        let mut out = Vec::new();
        launch(
            &self.programs,
            &self.paths,
            &self.plan(),
            &environment(),
            &mut out,
        )?;
        Ok(String::from_utf8(out).expect("utf-8"))
    }

    /// A process of the fake `/proc`.
    fn fake_process(&self, pid: u32, comm: &str, arguments: &[&str]) {
        let dir = self.programs.proc_dir.join(pid.to_string());
        private_dir(&dir);
        write_file(&dir.join("comm"), 0o444, &format!("{comm}\n"));
        let mut cmdline = arguments.join("\0");
        cmdline.push('\0');
        write_file(&dir.join("cmdline"), 0o444, &cmdline);
    }

    /// Both fake programs are alive: the fake `/proc` shows what a real one would.
    fn mirror_the_started_processes(&self) -> (u32, u32) {
        let soffice = report_of(&self.soffice_report);
        let bridge = report_of(&self.bridge_report);
        let (soffice_pid, bridge_pid) = (pid_of(&soffice), pid_of(&bridge));
        self.fake_process(
            soffice_pid,
            "soffice.bin",
            &[self.programs.soffice.to_str().expect("utf-8")],
        );
        self.fake_process(
            bridge_pid,
            "python3",
            &[
                self.programs.python.to_str().expect("utf-8"),
                self.bridge_file.to_str().expect("utf-8"),
            ],
        );
        (soffice_pid, bridge_pid)
    }
}

impl Drop for Setup {
    /// Whatever a test concluded, the fake programs do not outlive it.
    fn drop(&mut self) {
        for report in [&self.soffice_report, &self.bridge_report] {
            if let Ok(text) = fs::read_to_string(report)
                && let Ok(value) = serde_json::from_str::<Value>(&text)
            {
                kill(pid_of(&value));
            }
        }
    }
}

fn kill(pid: u32) {
    if let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
        let _ = kill_process(pid, Signal::KILL);
    }
}

fn alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .is_some_and(|pid| test_kill_process(pid).is_ok())
}

fn pid_of(report: &Value) -> u32 {
    u32::try_from(report["pid"].as_u64().expect("pid")).expect("pid fits")
}

/// What a fake program wrote about itself; waits for it, as the process is detached.
fn report_of(path: &Path) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && let Ok(value) = serde_json::from_str(&text)
        {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "{} was never written",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The variables the launcher itself is given, including some that must not travel.
fn environment() -> Vec<(OsString, OsString)> {
    [
        ("PATH", "/usr/bin:/bin"),
        ("HOME", "/home/alice"),
        ("LANG", "C.UTF-8"),
        ("DISPLAY", ":0"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ("LIBREMODULAR_TOKEN", FAKE_TOKEN),
        ("SECRET_FROM_THE_SHELL", "do-not-forward"),
    ]
    .iter()
    .map(|(name, value)| (OsString::from(name), OsString::from(value)))
    .collect()
}

fn text_of(report: &Value, key: &str) -> Vec<String> {
    report[key]
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item.as_str().expect("text").to_owned())
        .collect()
}

// --- the commands -----------------------------------------------------------------------

#[test]
fn libreoffice_is_started_with_a_window_from_a_fixed_list_of_arguments() {
    let setup = setup();
    let spawn = soffice_spawn(&setup.programs, &setup.plan(), &environment());
    assert_eq!(spawn.program, setup.programs.soffice);
    let arguments: Vec<&str> = spawn
        .args
        .iter()
        .map(|a| a.to_str().expect("utf-8"))
        .collect();
    assert_eq!(
        arguments,
        vec![
            "--accept=socket,host=127.0.0.1,port=2002;urp;",
            "--norestore",
            "--nologo",
            "--nodefault",
        ]
    );
    assert!(!arguments.contains(&"--headless"), "a window is wanted");
    assert_eq!(
        accept_argument(2345),
        "--accept=socket,host=127.0.0.1,port=2345;urp;"
    );
}

#[test]
fn the_bridge_is_the_system_python_on_the_bridge_file_and_nothing_else() {
    let setup = setup();
    let spawn = bridge_spawn(&setup.programs, &setup.plan(), &environment());
    assert_eq!(spawn.program, setup.programs.python);
    assert_eq!(spawn.args, vec![setup.bridge_file.clone().into_os_string()]);
}

#[test]
fn the_defaults_name_the_system_programs() {
    let programs = Programs::default();
    assert_eq!(programs.soffice, Path::new("/usr/bin/soffice"));
    assert_eq!(programs.python, Path::new("/usr/bin/python3"));
    assert_eq!(programs.proc_dir, Path::new("/proc"));
}

#[test]
fn the_bridge_environment_is_exactly_the_listed_one() {
    let setup = setup();
    let env = bridge_environment(&environment(), &setup.plan());
    let got: Vec<(String, String)> = env
        .iter()
        .map(|(n, v)| {
            (
                n.to_str().expect("utf-8").to_owned(),
                v.to_str().expect("utf-8").to_owned(),
            )
        })
        .collect();
    let token_file = setup.paths.token_file().to_str().expect("utf-8").to_owned();
    let expected: Vec<(String, String)> = [
        ("PATH", "/usr/bin:/bin"),
        ("HOME", "/home/alice"),
        ("LANG", "C.UTF-8"),
        ("LD_LIBRARY_PATH", "/usr/lib/libreoffice/program"),
        ("PYTHONPATH", "/usr/lib/python3/dist-packages"),
        ("LIBREMODULAR_UNO_PORT", "2002"),
        ("LIBREMODULAR_BRIDGE_PORT", "9304"),
        ("LIBREMODULAR_TOKEN_PATH", token_file.as_str()),
    ]
    .iter()
    .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
    .collect();
    assert_eq!(got, expected);
}

// --- what the children really receive ---------------------------------------------------

/// How a launch is done: the launcher, or something built to do it wrongly.
type Launcher<'l> = &'l dyn Fn(&Setup) -> Result<(), String>;

fn the_real_launcher(setup: &Setup) -> Result<(), String> {
    setup.launch().map(drop).map_err(|error| error.to_string())
}

/// Defective on purpose: the same two programs, through `sh -c` and one string.
fn through_a_shell(setup: &Setup) -> Result<(), String> {
    let plan = setup.plan();
    let soffice = soffice_spawn(&setup.programs, &plan, &environment());
    let line = std::iter::once(soffice.program.as_os_str().to_owned())
        .chain(soffice.args.iter().cloned())
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    for spawn in [
        ("/bin/sh", vec!["-c".to_owned(), line]),
        (
            setup.programs.python.to_str().expect("utf-8"),
            vec![setup.bridge_file.to_string_lossy().into_owned()],
        ),
    ] {
        detached(spawn.0, &spawn.1, &[]).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Defective on purpose: the real launcher's commands, with the token's content
/// added to the bridge's environment.
fn with_the_token_in_the_environment(setup: &Setup) -> Result<(), String> {
    let plan = setup.plan();
    let soffice = soffice_spawn(&setup.programs, &plan, &environment());
    let bridge = bridge_spawn(&setup.programs, &plan, &environment());
    let token = fs::read_to_string(setup.paths.token_file()).map_err(|e| e.to_string())?;
    let with_token = vec![(OsString::from("LIBREMODULAR_TOKEN"), OsString::from(token))];
    detached(
        soffice.program.to_str().expect("utf-8"),
        &soffice
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        &soffice.env,
    )
    .map_err(|e| e.to_string())?;
    let mut env = bridge.env.clone();
    env.extend(with_token);
    detached(
        bridge.program.to_str().expect("utf-8"),
        &bridge
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        &env,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn detached(
    program: &str,
    args: &[String],
    env: &[(OsString, OsString)],
) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    let _turn = one_at_a_time();
    Command::new(program)
        .args(args)
        .env_clear()
        .envs(env.iter().map(|(n, v)| (n, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
}

fn check_the_arguments_arrive_exactly_as_listed(launcher: Launcher<'_>) -> Verdict {
    let setup = setup();
    launcher(&setup)?;
    let soffice = report_of(&setup.soffice_report);
    let got = text_of(&soffice, "argv");
    let expected = [
        "--accept=socket,host=127.0.0.1,port=2002;urp;",
        "--norestore",
        "--nologo",
        "--nodefault",
    ];
    ensure!(got == expected, "LibreOffice received {got:?}");
    let bridge = report_of(&setup.bridge_report);
    ensure!(
        text_of(&bridge, "argv") == [setup.bridge_file.to_string_lossy().into_owned()],
        "the bridge received {:?}",
        text_of(&bridge, "argv")
    );
    Ok(())
}

#[test]
fn the_arguments_reach_the_programs_unchanged_with_no_shell_in_between() {
    must_pass(check_the_arguments_arrive_exactly_as_listed(
        &the_real_launcher,
    ));
}

#[test]
fn counter_proof_a_launcher_that_goes_through_a_shell_is_detected() {
    must_fail(check_the_arguments_arrive_exactly_as_listed(
        &through_a_shell,
    ));
}

fn check_the_token_is_in_no_argument_and_no_variable(launcher: Launcher<'_>) -> Verdict {
    let setup = setup();
    launcher(&setup)?;
    let token_file = setup.paths.token_file().to_string_lossy().into_owned();
    let mut found_the_path = false;
    for report in [&setup.soffice_report, &setup.bridge_report] {
        let report = report_of(report);
        let everything = format!(
            "{:?} {:?}",
            text_of(&report, "argv"),
            text_of(&report, "environment")
        );
        ensure!(
            !everything.contains(FILE_TOKEN),
            "the token reached a child"
        );
        ensure!(
            !everything.contains(FAKE_TOKEN),
            "a variable holding a token reached a child"
        );
        found_the_path |= text_of(&report, "environment")
            .contains(&format!("LIBREMODULAR_TOKEN_PATH={token_file}"));
    }
    ensure!(
        found_the_path,
        "the bridge was not told where the token file is"
    );
    Ok(())
}

#[test]
fn the_token_is_in_no_argument_and_no_variable_of_a_child_only_its_path_is() {
    must_pass(check_the_token_is_in_no_argument_and_no_variable(
        &the_real_launcher,
    ));
}

#[test]
fn counter_proof_a_launcher_that_passes_the_token_is_detected() {
    must_fail(check_the_token_is_in_no_argument_and_no_variable(
        &with_the_token_in_the_environment,
    ));
}

#[test]
fn the_bridge_inherits_only_what_is_listed_and_libreoffice_what_it_was_given() {
    let setup = setup();
    setup.launch().expect("launch");
    let bridge = text_of(&report_of(&setup.bridge_report), "environment");
    let token_file = setup.paths.token_file().display().to_string();
    let mut expected = vec![
        "HOME=/home/alice".to_owned(),
        "LANG=C.UTF-8".to_owned(),
        "LD_LIBRARY_PATH=/usr/lib/libreoffice/program".to_owned(),
        "LIBREMODULAR_BRIDGE_PORT=9304".to_owned(),
        format!("LIBREMODULAR_TOKEN_PATH={token_file}"),
        "LIBREMODULAR_UNO_PORT=2002".to_owned(),
        "PATH=/usr/bin:/bin".to_owned(),
        "PYTHONPATH=/usr/lib/python3/dist-packages".to_owned(),
    ];
    expected.sort();
    assert_eq!(bridge, expected);
    // A window needs the display and the session: it gets the launcher's environment,
    // except the variables of this program's own.
    let soffice = text_of(&report_of(&setup.soffice_report), "environment");
    let mut expected: Vec<String> = environment()
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                name.to_str().expect("utf-8"),
                value.to_str().expect("utf-8")
            )
        })
        .filter(|line| !line.starts_with("LIBREMODULAR_"))
        .collect();
    expected.sort();
    assert_eq!(soffice, expected);
    assert!(soffice.contains(&"DISPLAY=:0".to_owned()));
    assert!(
        !soffice.iter().any(|line| line.starts_with("LIBREMODULAR_")),
        "{soffice:?}"
    );
}

#[test]
fn both_programs_start_detached_in_a_group_of_their_own_with_nothing_attached() {
    let setup = setup();
    setup.launch().expect("launch");
    let own_group = i64::from(rustix::process::getpgrp().as_raw_nonzero().get());
    for report in [&setup.soffice_report, &setup.bridge_report] {
        let report = report_of(report);
        assert_eq!(report["group"], report["pid"], "the leader of a new group");
        assert_ne!(report["group"].as_i64(), Some(own_group));
        assert_eq!(
            text_of(&report, "streams"),
            ["/dev/null", "/dev/null", "/dev/null"]
        );
        assert_eq!(report["cwd"], "/");
    }
}

#[test]
fn only_the_loopback_address_is_named_in_what_is_started() {
    let setup = setup();
    setup.launch().expect("launch");
    for report in [&setup.soffice_report, &setup.bridge_report] {
        let report = report_of(report);
        let everything = format!("{:?}", report["argv"]);
        for forbidden in ["localhost", "0.0.0.0", "::", "[::"] {
            assert!(
                !everything.contains(forbidden),
                "{forbidden} in {everything}"
            );
        }
    }
    let argv = text_of(&report_of(&setup.soffice_report), "argv");
    assert!(argv[0].contains("host=127.0.0.1,"), "{argv:?}");
}

// --- a LibreOffice that is already running -------------------------------------------------

/// With a `soffice.bin` of the user in the listing, nothing starts: no program,
/// no record. The message says what to do.
fn check_nothing_starts_beside_a_running_libreoffice(launcher: Launcher<'_>) -> Verdict {
    let setup = setup();
    setup.fake_process(
        4242,
        "soffice.bin",
        &["/usr/lib/libreoffice/program/soffice.bin"],
    );
    let outcome = launcher(&setup);
    std::thread::sleep(Duration::from_millis(300));
    ensure!(
        !setup.soffice_report.exists() && !setup.bridge_report.exists(),
        "a program was started beside the running LibreOffice"
    );
    ensure!(!setup.record_file().exists(), "a record was written");
    ensure!(outcome.is_err(), "the launch was not refused");
    Ok(())
}

/// Defective on purpose: the programs are started without looking for LibreOffice.
fn without_looking_for_libreoffice(setup: &Setup) -> Result<(), String> {
    let plan = setup.plan();
    let soffice = soffice_spawn(&setup.programs, &plan, &environment());
    detached(soffice.program.to_str().expect("utf-8"), &[], &soffice.env)
        .map(drop)
        .map_err(|e| e.to_string())?;
    // Wait until it has run, so that the check sees what this launcher did.
    report_of(&setup.soffice_report);
    Err("started".to_owned())
}

#[test]
fn a_running_libreoffice_makes_the_launcher_refuse_and_start_nothing() {
    must_pass(check_nothing_starts_beside_a_running_libreoffice(
        &the_real_launcher,
    ));
}

#[test]
fn counter_proof_a_launcher_that_does_not_look_for_libreoffice_is_detected() {
    must_fail(check_nothing_starts_beside_a_running_libreoffice(
        &without_looking_for_libreoffice,
    ));
}

#[test]
fn the_refusal_says_what_to_do_and_names_the_option_to_use() {
    let setup = setup();
    setup.fake_process(4242, "soffice.bin", &["soffice.bin"]);
    setup.fake_process(4300, "soffice.bin", &["soffice.bin"]);
    let error = setup.launch().expect_err("refused");
    assert_eq!(error.exit_code(), 2);
    let text = error.to_string();
    assert!(text.contains("4242") && text.contains("4300"), "{text}");
    assert!(text.contains("Close LibreOffice"), "{text}");
    assert!(
        text.contains("--accept=\"socket,host=127.0.0.1,port=2002;urp;\""),
        "{text}"
    );
    assert!(text.contains("docs/modules/libreoffice.md"), "{text}");
    assert!(text.contains("never closes one it did not start"), "{text}");
}

#[test]
fn only_a_soffice_bin_of_the_same_user_counts() {
    let setup = setup();
    setup.fake_process(10, "soffice.bin", &["soffice.bin"]);
    setup.fake_process(11, "soffice", &["soffice"]);
    setup.fake_process(12, "firefox", &["firefox"]);
    fs::write(setup.programs.proc_dir.join("self"), "not a process").expect("file");
    let uid = common::current_uid();
    assert_eq!(
        running_soffice(&setup.programs.proc_dir, uid).expect("listing"),
        vec![10]
    );
    assert_eq!(
        running_soffice(&setup.programs.proc_dir, uid + 1).expect("listing"),
        Vec::<u32>::new(),
        "another user's LibreOffice is not ours to worry about"
    );
    assert!(running_soffice(&setup.root.join("no-such-proc"), uid).is_err());
}

#[test]
fn the_real_listing_of_processes_can_be_read() {
    // What it holds depends on the machine: only that it is readable is asserted.
    assert!(running_soffice(Path::new("/proc"), common::current_uid()).is_ok());
}

#[test]
fn a_bridge_of_an_earlier_launch_that_still_runs_makes_the_launcher_refuse() {
    let setup = setup();
    setup.launch().expect("first launch");
    setup.mirror_the_started_processes();
    // LibreOffice was closed by hand; the bridge is still there.
    fs::remove_dir_all(
        setup
            .programs
            .proc_dir
            .join(pid_of(&report_of(&setup.soffice_report)).to_string()),
    )
    .expect("LibreOffice is gone");
    let error = setup.launch().expect_err("refused");
    assert_eq!(error.exit_code(), 2);
    assert!(error.to_string().contains("--stop"), "{error}");
}

// --- the configuration --------------------------------------------------------------------

#[test]
fn the_ports_come_from_the_enabled_targets_and_must_agree() {
    let setup = setup();
    let bridge_file = setup.bridge_file.clone();
    let token_file = setup.paths.token_file();
    let plan =
        |text: &str| plan_from_config(&setup.config(text), bridge_file.clone(), token_file.clone());
    let both = plan(
        "[apps.libreoffice-writer]\nenabled = true\nuno_port = 2102\nbridge_port = 9404\n\
         [apps.libreoffice-impress]\nenabled = true\nuno_port = 2102\nbridge_port = 9404\n",
    )
    .expect("agreeing targets");
    assert_eq!((both.uno_port, both.bridge_port), (2102, 9404));
    let defaults = plan("[apps.libreoffice-calc]\nenabled = true\n").expect("defaults");
    assert_eq!(
        (defaults.uno_port, defaults.bridge_port),
        (UNO_PORT, BRIDGE_PORT)
    );

    let none = plan("[apps.gimp]\nenabled = true\n[apps.libreoffice-calc]\nenabled = false\n")
        .expect_err("no target");
    assert!(matches!(none, LaunchError::NoTarget));
    assert_eq!(none.exit_code(), 2);
    assert!(none.to_string().contains("libreoffice-calc"), "{none}");

    let apart = plan(
        "[apps.libreoffice-writer]\nenabled = true\nuno_port = 2102\n\
         [apps.libreoffice-calc]\nenabled = true\nuno_port = 2202\n",
    )
    .expect_err("disagreement");
    assert_eq!(apart.exit_code(), 2);
    let text = apart.to_string();
    assert!(text.contains("uno_port"), "{text}");
    assert!(
        text.contains("apps.libreoffice-writer.uno_port = 2102"),
        "{text}"
    );
    assert!(
        text.contains("apps.libreoffice-calc.uno_port = 2202"),
        "{text}"
    );

    let bridge_apart = plan(
        "[apps.libreoffice-writer]\nenabled = true\nbridge_port = 9404\n\
         [apps.libreoffice-impress]\nenabled = true\n",
    )
    .expect_err("disagreement");
    assert!(
        bridge_apart.to_string().contains("bridge_port"),
        "{bridge_apart}"
    );

    let same = plan("[apps.libreoffice-calc]\nenabled = true\nuno_port = 9304\n")
        .expect_err("the same port");
    assert!(matches!(same, LaunchError::PortsClash { port: 9304 }));
    assert_eq!(same.exit_code(), 2);
}

#[test]
fn a_target_enabled_in_another_mode_is_not_launched_for() {
    let setup = setup();
    let none = plan_from_config(
        &setup.config("[apps.libreoffice-calc]\nenabled = true\nmode = \"tcp\"\n"),
        setup.bridge_file.clone(),
        setup.paths.token_file(),
    )
    .expect_err("no uno target");
    assert!(matches!(none, LaunchError::NoTarget));
}

// --- the bridge file ------------------------------------------------------------------

#[test]
fn a_bridge_file_that_cannot_be_trusted_is_refused_and_nothing_starts() {
    let setup = setup();
    let victim = setup.root.join("elsewhere.py");
    write_file(&victim, 0o644, "pass\n");
    symlink(&victim, setup.root.join("link.py")).expect("link");
    write_file(&setup.root.join("open.py"), 0o666, "pass\n");
    for (label, file) in [
        ("a link", setup.root.join("link.py")),
        ("a file others can write", setup.root.join("open.py")),
        ("a directory", setup.root.clone()),
        ("a relative path", PathBuf::from("uno_bridge.py")),
        ("a missing file", setup.root.join("missing.py")),
    ] {
        let mut out = Vec::new();
        let plan = Plan {
            bridge_file: file,
            ..setup.plan()
        };
        let _turn = one_at_a_time();
        let error = launch(
            &setup.programs,
            &setup.paths,
            &plan,
            &environment(),
            &mut out,
        )
        .expect_err(label);
        assert_eq!(error.exit_code(), 2, "{label}: {error}");
        assert!(
            error.to_string().contains("--bridge-file"),
            "{label}: {error}"
        );
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(!setup.soffice_report.exists() && !setup.bridge_report.exists());
    assert!(!setup.record_file().exists());
}

// --- the record -----------------------------------------------------------------------

#[test]
fn the_record_lists_both_processes_in_a_private_file_of_a_private_directory() {
    let setup = setup();
    let out = setup.launch().expect("launch");
    let soffice = pid_of(&report_of(&setup.soffice_report));
    let bridge = pid_of(&report_of(&setup.bridge_report));
    let record = setup.record_file();
    assert_eq!(common::mode_of(&record), 0o600);
    assert_eq!(common::mode_of(record.parent().expect("launch dir")), 0o700);
    assert_eq!(
        common::mode_of(setup.paths.runtime_dir.as_deref().expect("runtime")),
        0o700
    );
    let text = fs::read_to_string(&record).expect("record");
    assert_eq!(
        text,
        format!(
            "soffice {soffice}\nbridge {bridge} {}\n",
            setup.bridge_file.display()
        )
    );
    assert_eq!(
        Record::parse(&text),
        Some(Record {
            soffice_pid: Some(soffice),
            bridge: Some((bridge, setup.bridge_file.clone())),
        })
    );
    assert!(
        out.contains(&format!("process {soffice}")) && out.contains(&format!("process {bridge}"))
    );
    assert!(
        out.contains("127.0.0.1:2002") && out.contains("127.0.0.1:9304"),
        "{out}"
    );
    assert!(!out.contains(FILE_TOKEN));
}

#[test]
fn a_record_that_is_a_link_stops_the_launch_and_is_never_written_through() {
    let setup = setup();
    let victim = setup.root.join("victim.txt");
    write_file(&victim, 0o600, "precious\n");
    fs::create_dir_all(setup.record_file().parent().expect("dir")).expect("dir");
    fs::set_permissions(
        setup.record_file().parent().expect("dir"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("mode");
    fs::set_permissions(
        setup.paths.runtime_dir.as_deref().expect("runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("mode");
    symlink(&victim, setup.record_file()).expect("link");
    // A link at the record's place makes the record unreadable as a control file:
    // the launcher stops there instead of following it.
    let error = setup.launch().expect_err("refused");
    assert_eq!(error.exit_code(), 1, "{error}");
    assert_eq!(fs::read_to_string(&victim).expect("victim"), "precious\n");
    assert!(
        !setup.soffice_report.exists(),
        "nothing starts on a suspect record"
    );
}

#[test]
fn without_a_volatile_directory_nothing_is_written_and_nothing_starts() {
    let mut setup = setup();
    setup.paths.runtime_dir = None;
    let error = setup.launch().expect_err("refused");
    assert_eq!(error.exit_code(), 1);
    assert!(error.to_string().contains("XDG_RUNTIME_DIR"), "{error}");
    assert!(!setup.soffice_report.exists());
}

#[test]
fn a_bridge_that_cannot_start_leaves_libreoffice_alone_and_says_so() {
    let mut setup = setup();
    setup.programs.python = setup.root.join("bin").join("no-such-python");
    let error = setup.launch().expect_err("the bridge did not start");
    assert_eq!(error.exit_code(), 1);
    let text = error.to_string();
    assert!(
        text.contains("LibreOffice was started") && text.contains("close it yourself"),
        "{text}"
    );
    let soffice = pid_of(&report_of(&setup.soffice_report));
    assert!(alive(soffice), "LibreOffice is not closed by the launcher");
    let record = fs::read_to_string(setup.record_file()).expect("record");
    assert_eq!(record, format!("soffice {soffice}\n"));
}

// --- stopping --------------------------------------------------------------------------------

fn wait_until_gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        // A child of this test process stays a zombie until it is waited for:
        // its state is read, since a signal still reaches a zombie.
        let state = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if state.is_empty() || state.contains(") Z ") {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn stopped(setup: &Setup) -> String {
    let mut out = Vec::new();
    stop(&setup.programs, &setup.paths, &mut out).expect("stop");
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn stop_ends_the_bridge_only_and_leaves_libreoffice_to_its_operator() {
    let setup = setup();
    setup.launch().expect("launch");
    let (soffice, bridge) = setup.mirror_the_started_processes();
    let said = stopped(&setup);
    assert!(wait_until_gone(bridge), "the bridge was not stopped");
    assert!(
        alive(soffice),
        "LibreOffice must never be closed by the launcher"
    );
    assert!(
        said.contains(&format!("bridge stopped (process {bridge})")),
        "{said}"
    );
    assert!(
        said.contains(&format!("LibreOffice (process {soffice}) was left running")),
        "{said}"
    );
    assert!(!setup.record_file().exists(), "the record is removed");
    let again = stopped(&setup);
    assert!(again.contains("nothing to stop"), "{again}");
}

/// A record names a process number that belongs to another program now: stopping
/// must not touch it. `stopper` is what stops the bridge.
type Stopper<'s> = &'s dyn Fn(&Setup, &Record);

fn the_real_stop(setup: &Setup, _: &Record) {
    let mut out = Vec::new();
    let _ = stop(&setup.programs, &setup.paths, &mut out);
}

/// Defective on purpose: it trusts the record and signals the number in it.
fn stop_without_checking_the_command_line(setup: &Setup, record: &Record) {
    if let Some((pid, _)) = &record.bridge {
        kill(*pid);
    }
    let _ = fs::remove_file(setup.record_file());
}

fn check_a_reused_process_number_is_never_signalled(stopper: Stopper<'_>) -> Verdict {
    let setup = setup();
    let turn = one_at_a_time();
    let mut other = Command::new(python())
        .args(["-c", "import time; time.sleep(60)"])
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    drop(turn);
    let pid = other.id();
    // The listing says that process is not the bridge.
    setup.fake_process(pid, "python3", &["python3", "-c", "import time"]);
    let record = Record {
        soffice_pid: None,
        bridge: Some((pid, setup.bridge_file.clone())),
    };
    let directory = setup.record_file().parent().expect("dir").to_path_buf();
    private_dir(setup.paths.runtime_dir.as_deref().expect("runtime"));
    private_dir(&directory);
    write_file(&setup.record_file(), 0o600, &record.render());
    stopper(&setup, &record);
    std::thread::sleep(Duration::from_millis(300));
    let survived = other.try_wait().map_err(|e| e.to_string())?.is_none();
    let _ = other.kill();
    let _ = other.wait();
    ensure!(survived, "a process that is not the bridge was signalled");
    Ok(())
}

#[test]
fn a_process_number_taken_by_another_program_is_never_signalled() {
    must_pass(check_a_reused_process_number_is_never_signalled(
        &the_real_stop,
    ));
}

#[test]
fn counter_proof_a_stop_that_trusts_the_record_kills_the_other_program() {
    must_fail(check_a_reused_process_number_is_never_signalled(
        &stop_without_checking_the_command_line,
    ));
}

#[test]
fn stop_with_no_record_is_not_an_error() {
    let setup = setup();
    assert!(stopped(&setup).contains("nothing to stop"));
}

#[test]
fn stop_with_a_record_it_cannot_read_signals_nothing() {
    let setup = setup();
    let directory = setup.record_file().parent().expect("dir").to_path_buf();
    private_dir(setup.paths.runtime_dir.as_deref().expect("runtime"));
    private_dir(&directory);
    for (label, text, mode) in [
        ("garbage", "this is not a record\n", 0o600),
        ("a relative bridge path", "bridge 4242 relative.py\n", 0o600),
        (
            "a bad number",
            "bridge forty-two /home/alice/uno_bridge.py\n",
            0o600,
        ),
        (
            "a record open to others",
            "bridge 4242 /home/alice/uno_bridge.py\n",
            0o644,
        ),
    ] {
        write_file(&setup.record_file(), mode, text);
        let mut out = Vec::new();
        let error = stop(&setup.programs, &setup.paths, &mut out).expect_err(label);
        assert_eq!(error.exit_code(), 1, "{label}: {error}");
        assert!(out.is_empty(), "{label}");
        assert!(
            setup.record_file().exists(),
            "{label}: the record is kept for the operator"
        );
        fs::remove_file(setup.record_file()).expect("remove");
    }
}

#[test]
fn a_recorded_bridge_that_is_gone_is_reported_and_the_record_removed() {
    let setup = setup();
    let directory = setup.record_file().parent().expect("dir").to_path_buf();
    private_dir(setup.paths.runtime_dir.as_deref().expect("runtime"));
    private_dir(&directory);
    let record = Record {
        soffice_pid: Some(4000),
        bridge: Some((4001, setup.bridge_file.clone())),
    };
    write_file(&setup.record_file(), 0o600, &record.render());
    let said = stopped(&setup);
    assert!(
        said.contains("no longer running: nothing was signalled"),
        "{said}"
    );
    assert!(
        said.contains("LibreOffice (process 4000) was left running"),
        "{said}"
    );
    assert!(!setup.record_file().exists());
}

// --- with the real /proc ---------------------------------------------------------------

#[test]
fn the_bridge_is_recognised_by_an_argument_of_its_real_command_line() {
    let setup = setup();
    let turn = one_at_a_time();
    let mut child = Command::new(python())
        .args(["-c", "import sys, time; time.sleep(60)"])
        .arg(&setup.bridge_file)
        .stdin(Stdio::null())
        .spawn()
        .expect("child");
    drop(turn);
    let pid = child.id();
    let real = Path::new("/proc");
    // Right after the program was executed its command line may not be there yet.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !is_the_bridge(real, pid, &setup.bridge_file) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let recognised = is_the_bridge(real, pid, &setup.bridge_file);
    let other_file = is_the_bridge(real, pid, &setup.root.join("other_bridge.py"));
    let part_of_a_name = is_the_bridge(
        real,
        pid,
        Path::new(
            setup
                .bridge_file
                .to_str()
                .expect("utf-8")
                .trim_end_matches(".py"),
        ),
    );
    let _ = child.kill();
    let _ = child.wait();
    assert!(recognised, "the real command line was not matched");
    assert!(!other_file, "another file was matched");
    assert!(!part_of_a_name, "a part of the argument was matched");
    assert!(
        !is_the_bridge(real, pid, &setup.bridge_file),
        "a process that is gone is not the bridge"
    );
}

// --- the command line -------------------------------------------------------------------------

struct Run {
    code: u8,
    out: String,
    err: String,
}

fn run_args(setup: &Setup, args: &[&str]) -> Run {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let paths = setup.paths.clone();
    let locate = move || -> Result<Paths, ConfigError> { Ok(paths.clone()) };
    let code = run(
        args.iter().map(OsString::from),
        &locate,
        &setup.programs,
        &environment(),
        &mut out,
        &mut err,
    );
    Run {
        code,
        out: String::from_utf8(out).expect("utf-8"),
        err: String::from_utf8(err).expect("utf-8"),
    }
}

fn write_configuration(setup: &Setup, text: &str) {
    write_file(&setup.paths.config_file(), 0o600, text);
}

#[test]
fn the_other_applications_are_started_by_hand() {
    let setup = setup();
    for name in ["gimp", "krita", "blender", "inkscape"] {
        let result = run_args(&setup, &["libremodular-launch", name]);
        assert_eq!(result.code, 2, "{name}");
        assert!(result.out.is_empty());
        assert_eq!(
            result.err,
            format!(
                "libremodular-launch: {name}: start the bridge by hand, see docs/modules/{name}.md\n"
            )
        );
        let with_more = run_args(&setup, &["libremodular-launch", name, "--stop"]);
        assert_eq!(with_more.code, 2);
    }
    assert!(!setup.soffice_report.exists());
}

#[test]
fn an_unknown_name_or_a_wrong_usage_prints_the_usage_and_exits_with_2() {
    let setup = setup();
    for args in [
        vec!["libremodular-launch"],
        vec!["libremodular-launch", "word"],
        vec!["libremodular-launch", "libreoffice"],
        vec!["libremodular-launch", "libreoffice", "--bridge-file"],
        vec![
            "libremodular-launch",
            "libreoffice",
            "--stop",
            "--bridge-file",
            "/x.py",
        ],
        vec![
            "libremodular-launch",
            "libreoffice",
            "--bridge-file",
            "/a.py",
            "/b.py",
        ],
        vec!["libremodular-launch", "libreoffice", "--start"],
        vec!["libremodular-launch", "libreoffice", "--bridge-file="],
        vec!["libremodular-launch", "LibreOffice", "--stop"],
    ] {
        let result = run_args(&setup, &args);
        assert_eq!(result.code, 2, "{args:?}");
        assert!(result.out.is_empty(), "{args:?}");
        assert!(result.err.starts_with("usage:"), "{args:?}: {}", result.err);
        assert!(result.err.contains("--bridge-file") && result.err.contains("--stop"));
    }
    assert!(!setup.soffice_report.exists());
}

#[test]
fn help_prints_the_usage_on_the_standard_output_and_exits_with_0() {
    let setup = setup();
    for args in [
        vec!["libremodular-launch", "--help"],
        vec!["libremodular-launch", "-h"],
        vec!["libremodular-launch", "libreoffice", "--help"],
    ] {
        let result = run_args(&setup, &args);
        assert_eq!(result.code, 0, "{args:?}");
        assert!(result.out.starts_with("usage:") && result.err.is_empty());
    }
}

#[test]
fn from_the_command_line_the_launch_and_the_stop_work_and_report_their_exit_codes() {
    let setup = setup();
    write_configuration(&setup, "[apps.libreoffice-calc]\nenabled = true\n");
    let bridge_file = setup.bridge_file.to_str().expect("utf-8").to_owned();
    let started = run_args(
        &setup,
        &[
            "libremodular-launch",
            "libreoffice",
            "--bridge-file",
            &bridge_file,
        ],
    );
    assert_eq!(started.code, 0, "{}", started.err);
    assert!(started.err.is_empty());
    assert!(started.out.contains("LibreOffice started"));
    let soffice = report_of(&setup.soffice_report);
    assert_eq!(
        text_of(&soffice, "argv")[0],
        "--accept=socket,host=127.0.0.1,port=2002;urp;"
    );
    // The same with the other spelling of the option.
    setup.mirror_the_started_processes();
    let again = run_args(
        &setup,
        &[
            "libremodular-launch",
            "libreoffice",
            &format!("--bridge-file={bridge_file}"),
        ],
    );
    assert_eq!(
        again.code, 2,
        "a running LibreOffice and bridge: {}",
        again.err
    );
    let bridge = pid_of(&report_of(&setup.bridge_report));
    let stop_result = run_args(&setup, &["libremodular-launch", "libreoffice", "--stop"]);
    assert_eq!(stop_result.code, 0, "{}", stop_result.err);
    assert!(wait_until_gone(bridge));
    assert!(stop_result.out.contains("was left running"));
}

#[test]
fn a_configuration_without_a_libreoffice_target_is_refused_with_code_2_and_starts_nothing() {
    let setup = setup();
    write_configuration(&setup, "[apps.gimp]\nenabled = true\n");
    let result = run_args(
        &setup,
        &[
            "libremodular-launch",
            "libreoffice",
            "--bridge-file",
            setup.bridge_file.to_str().expect("utf-8"),
        ],
    );
    assert_eq!(result.code, 2);
    assert!(
        result.err.contains("no LibreOffice target is enabled"),
        "{}",
        result.err
    );
    assert!(!setup.soffice_report.exists());
}

#[test]
fn a_configuration_the_hub_would_refuse_is_an_error_with_code_1() {
    let setup = setup();
    write_file(
        &setup.paths.config_file(),
        0o644,
        "[apps.libreoffice-calc]\nenabled = true\n",
    );
    let result = run_args(
        &setup,
        &[
            "libremodular-launch",
            "libreoffice",
            "--bridge-file",
            setup.bridge_file.to_str().expect("utf-8"),
        ],
    );
    assert_eq!(result.code, 1);
    assert!(
        result.err.contains("configuration file refused"),
        "{}",
        result.err
    );
    assert!(!setup.soffice_report.exists());
}

#[test]
fn directories_that_cannot_be_found_are_an_error_with_code_1() {
    let setup = setup();
    let locate = || -> Result<Paths, ConfigError> {
        Err(ConfigError::Environment(
            "cannot determine the home directory".to_owned(),
        ))
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(
        ["libremodular-launch", "libreoffice", "--stop"]
            .iter()
            .map(OsString::from),
        &locate,
        &setup.programs,
        &environment(),
        &mut out,
        &mut err,
    );
    assert_eq!(code, 1);
    assert!(
        String::from_utf8(err)
            .expect("utf-8")
            .contains("home directory")
    );
}
