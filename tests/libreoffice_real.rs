//! Le Hub face à un vrai LibreOffice, par le pont généré.
//!
//! Pourquoi ce module existe
//! Les faux montrent que le Hub et le pont tiennent parole ; seul un vrai LibreOffice montre que
//! les appels UNO de l'adaptateur veulent dire ce qu'on suppose. Le test est ignoré par défaut
//! parce qu'il exige LibreOffice et son `python3-uno` ; lancez-le avec `cargo test --test
//! libreoffice_real -- --ignored --nocapture`. Il démarre son propre LibreOffice sans interface
//! sur un profil et un port jetables, ne touche jamais une instance de l'opérateur, et arrête
//! tout ce qu'il a démarré.

#[macro_use]
mod common;

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::hub::{Session, build_with_token, config};
use common::{FILE_TOKEN, Verdict, must_pass, write_file};
use libremodular_mcp::launch::{Plan, Programs, bridge_spawn};
use libremodular_mcp::security::Token;
use rustix::process::{Pid, Signal, kill_process_group};
use serde_json::{Value, json};

const LISTENING: &str = "LMM bridge listening on 127.0.0.1:";

fn free_port() -> u16 {
    let listener =
        TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("a free port");
    listener.local_addr().expect("address").port()
}

/// Un processus qui mène son propre groupe ; tout le groupe est arrêté quand il est abandonné, pour que
/// ce que LibreOffice a démarré parte avec lui.
struct Leader(Child);

impl Drop for Leader {
    fn drop(&mut self) {
        let Some(group) = i32::try_from(self.0.id()).ok().and_then(Pid::from_raw) else {
            return;
        };
        let _ = kill_process_group(group, Signal::TERM);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = kill_process_group(group, Signal::KILL);
        let _ = self.0.wait();
    }
}

/// Les variables dont le Python du système a besoin pour importer `uno` : ce que le lanceur donne au pont, pour la même raison.
fn uno_environment(path: &OsString, uno_port: u16) -> Vec<(String, OsString)> {
    vec![
        ("PATH".to_owned(), path.clone()),
        (
            "PYTHONPATH".to_owned(),
            "/usr/lib/python3/dist-packages".into(),
        ),
        (
            "LD_LIBRARY_PATH".to_owned(),
            "/usr/lib/libreoffice/program".into(),
        ),
        ("UNO_PORT".to_owned(), uno_port.to_string().into()),
    ]
}

const PRELUDE: &str = r#"
import os, time, uno
local = uno.getComponentContext()
resolver = local.ServiceManager.createInstanceWithContext("com.sun.star.bridge.UnoUrlResolver", local)
url = "uno:socket,host=127.0.0.1,port=%s;urp;StarOffice.ComponentContext" % os.environ["UNO_PORT"]
deadline = time.monotonic() + 5
while True:
    try:
        ctx = resolver.resolve(url)
        break
    except Exception:
        if time.monotonic() > deadline:
            raise
        time.sleep(0.2)
desktop = ctx.ServiceManager.createInstanceWithContext("com.sun.star.frame.Desktop", ctx)
"#;

/// Quelques lignes d'UNO exécutées par le Python du système, hors du pont : il prépare et inspecte
/// LibreOffice comme le ferait une personne, jamais par le Hub.
fn uno_snippet(path: &OsString, uno_port: u16, code: &str) -> Result<String, String> {
    let output = Command::new("/usr/bin/python3")
        .args(["-c", &format!("{PRELUDE}\n{code}")])
        .env_clear()
        .envs(uno_environment(path, uno_port))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run the system python3: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn start_libreoffice(base: &Path, path: &OsString, uno_port: u16) -> Result<Leader, String> {
    let home = base.join("home");
    fs::create_dir(&home).map_err(|error| error.to_string())?;
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let profile = base.join("profile");
    let child = Command::new("/usr/bin/soffice")
        .args([
            "--headless",
            "--invisible",
            "--norestore",
            "--nologo",
            "--nodefault",
        ])
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.display()
        ))
        .arg(format!(
            "--accept=socket,host=127.0.0.1,port={uno_port};urp;"
        ))
        .env_clear()
        .env("HOME", &home)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("cannot start /usr/bin/soffice: {error}"))?;
    Ok(Leader(child))
}

/// Attend que LibreOffice réponde sur UNO ; il n'écoute qu'une fois démarré.
fn wait_for_libreoffice(path: &OsString, uno_port: u16) -> Result<String, String> {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(120);
    let mut last = String::new();
    while Instant::now() < deadline {
        match uno_snippet(path, uno_port, "print(len(list(range(1))))") {
            Ok(_) => {
                return Ok(format!(
                    "LibreOffice answered on UNO after {:.1} s",
                    started.elapsed().as_secs_f32()
                ));
            }
            Err(error) => last = error,
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!(
        "LibreOffice never answered on UNO within 120 s; last error: {last}"
    ))
}

fn start_bridge(plan: &Plan, path: &OsString) -> Result<(Leader, u16), String> {
    // Le pont est démarré exactement comme le lanceur le démarre.
    let environment = vec![(OsString::from("PATH"), path.clone())];
    let spawn = bridge_spawn(&Programs::default(), plan, &environment);
    let mut child = Command::new(&spawn.program)
        .args(&spawn.args)
        .current_dir(&spawn.current_dir)
        .env_clear()
        .envs(spawn.env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("cannot start the bridge: {error}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or("the bridge has no error stream")?;
    let leader = Leader(child);
    let (sender, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = lines
            .recv_timeout(remaining)
            .map_err(|_| "the bridge never said where it listens".to_owned())?;
        ensure!(!line.contains(FILE_TOKEN), "the bridge printed the token");
        if let Some(port) = line.strip_prefix(LISTENING) {
            let port = port
                .trim()
                .parse()
                .map_err(|_| format!("bad line {line}"))?;
            return Ok((leader, port));
        }
    }
}

fn show(label: &str, value: &Value) {
    eprintln!(
        "--- {label}\n{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn drive(base: &Path) -> Verdict {
    let path = std::env::var_os("PATH").ok_or("PATH is not set")?;
    let (uno_port, bridge_port) = (free_port(), free_port());
    let _libreoffice = start_libreoffice(base, &path, uno_port)?;
    eprintln!("{}", wait_for_libreoffice(&path, uno_port)?);

    let token_file = base.join("token");
    write_file(&token_file, 0o600, FILE_TOKEN);
    let bridge_file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("bridges")
        .join("generated")
        .join("uno_bridge.py");
    let plan = Plan {
        uno_port,
        bridge_port,
        bridge_file,
        token_file,
    };
    let (_bridge, bridge_port) = start_bridge(&plan, &path)?;

    let hub = build_with_token(
        base,
        config(&format!(
            "[apps.libreoffice-writer]\nenabled = true\nuno_port = {uno_port}\nbridge_port = {bridge_port}\n\
             [apps.libreoffice-calc]\nenabled = true\nuno_port = {uno_port}\nbridge_port = {bridge_port}\n\
             [apps.libreoffice-impress]\nenabled = true\nuno_port = {uno_port}\nbridge_port = {bridge_port}\n"
        )),
        Vec::new(),
        Token::new(FILE_TOKEN).map_err(|error| error.to_string())?,
    );
    let mut session = Session::start(hub.server.clone());

    // 1. Les trois cibles sont joignables par une seule connexion.
    let listing = session.state("hub");
    show("get_state hub", &listing);
    let modules = listing["state"]["modules"].as_array().ok_or("no modules")?;
    ensure!(modules.len() == 3, "{listing}");
    for module in modules {
        ensure!(module["connected"] == true, "not connected: {module}");
        ensure!(module["mode"] == "uno", "{module}");
        ensure!(module["bridge_version"] == "1", "{module}");
    }
    let version = modules[0]["app_version"].as_str().unwrap_or_default();
    ensure!(
        version.chars().next().is_some_and(|c| c.is_ascii_digit()),
        "the version of LibreOffice was not read: {version:?}"
    );

    // 2. Sans document ouvert, un script s'exécute quand même, avec `doc` à None, et l'état dit qu'il
    // n'y a pas de document actif.
    let bare = session.execute(
        "libreoffice-calc",
        "result = {'doc_is_none': doc is None, 'has_desktop': desktop is not None}",
    );
    show("execute without a document", &bare);
    ensure!(bare["status"] == "ok", "{bare}");
    ensure!(
        bare["result"] == json!({"doc_is_none": true, "has_desktop": true}),
        "{bare}"
    );
    let empty = session.state("libreoffice-calc");
    show("get_state without a document", &empty);
    ensure!(empty["connected"] == true, "{empty}");
    ensure!(empty["state"]["documents"] == json!([]), "{empty}");
    ensure!(empty["state"]["active"] == Value::Null, "{empty}");
    ensure!(
        empty["warnings"]
            .to_string()
            .contains("no active calc document"),
        "{empty}"
    );
    ensure!(empty["screenshot"] == Value::Null, "{empty}");

    // 3. Le modèle crée le premier document avec un script, puis y écrit.
    let script = "\
if doc is None:
    doc = desktop.loadComponentFromURL('private:factory/scalc', '_blank', 0, ())
sheet = doc.Sheets.getByIndex(0)
sheet.getCellByPosition(0, 0).setValue(42)
sheet.getCellByPosition(1, 1).setString('hello')
print('wrote A1 and B2')
result = {'sheet': sheet.Name, 'a1': sheet.getCellByPosition(0, 0).getValue(), 'b2': sheet.getCellByPosition(1, 1).getString()}
";
    let wrote = session.execute("libreoffice-calc", script);
    show("execute on a spreadsheet", &wrote);
    ensure!(wrote["status"] == "ok", "{wrote}");
    ensure!(wrote["stdout"] == "wrote A1 and B2\n", "{wrote}");
    ensure!(
        wrote["result"] == json!({"sheet": "Sheet1", "a1": 42.0, "b2": "hello"}),
        "{wrote}"
    );

    // 4. L'état décrit le document et une capture est faite, sans enregistrer.
    let state = session.state("libreoffice-calc");
    show("get_state of a spreadsheet", &state);
    ensure!(state["connected"] == true, "{state}");
    ensure!(
        state["state"]["documents"] == json!([{"title": "Untitled 1", "family": "calc"}]),
        "{state}"
    );
    ensure!(
        state["state"]["active"]
            == json!({"sheets": [{"name": "Sheet1", "last_column": 1, "last_row": 1}]}),
        "{state}"
    );
    ensure!(state["warnings"] == json!([]), "{state}");
    let capture = state["screenshot"].as_str().ok_or("no capture")?;
    ensure!(Path::new(capture).starts_with(hub.captures()), "{capture}");
    let bytes = fs::read(capture).map_err(|error| error.to_string())?;
    ensure!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "the capture is not a PNG"
    );
    ensure!(
        common::mode_of(Path::new(capture)) == 0o600,
        "the capture is not private"
    );
    let saved = uno_snippet(
        &path,
        uno_port,
        "doc = desktop.getCurrentComponent()\nprint(doc.hasLocation(), doc.isModified())",
    )?;
    ensure!(
        saved == "False True",
        "the capture saved the document (hasLocation, isModified): {saved}"
    );

    // 5. Un script pour une autre famille est refusé, et rien de lui ne s'exécute.
    let wrong = session.execute(
        "libreoffice-writer",
        "open(LMM_WORK_DIR + '/ran', 'w').write('x')",
    );
    show("execute for the wrong family", &wrong);
    ensure!(wrong["error"]["type"] == "WrongDocumentType", "{wrong}");
    ensure!(
        !hub.volatile
            .join("work")
            .join("libreoffice-writer")
            .join("ran")
            .exists(),
        "a line of the script ran"
    );
    let writer_state = session.state("libreoffice-writer");
    ensure!(
        writer_state["state"]["active"] == Value::Null,
        "{writer_state}"
    );
    ensure!(
        writer_state["state"]["documents"] == json!([{"title": "Untitled 1", "family": "calc"}]),
        "{writer_state}"
    );

    // 6. Une erreur de l'application revient telle quelle.
    let broken = session.execute(
        "libreoffice-calc",
        "x = 1\nresult = doc.Sheets.getByName('Nope')",
    );
    show("execute with an error", &broken);
    ensure!(broken["status"] == "error", "{broken}");
    ensure!(
        broken["error"]["traceback"]
            .as_str()
            .unwrap_or_default()
            .contains("File \"<libremodular>\", line 2, in <module>"),
        "{broken}"
    );

    // 7. Un diaporama et un document texte, l'un après l'autre, chacun sur sa propre cible.
    // Chacun est créé par un script de la cible qui était courante jusque-là.
    for (creator, target, factory, probe, expected, active) in [
        (
            "libreoffice-calc",
            "libreoffice-impress",
            "private:factory/simpress",
            "result = {'slides': doc.DrawPages.Count}",
            json!({"slides": 1}),
            json!({"slides": 1}),
        ),
        (
            "libreoffice-impress",
            "libreoffice-writer",
            "private:factory/swriter",
            "text = doc.Text\n\
             text.insertString(text.End, 'one', False)\n\
             text.insertControlCharacter(text.End, 0, False)\n\
             text.insertString(text.End, 'two', False)\n\
             enumeration = text.createEnumeration()\n\
             count = 0\n\
             while enumeration.hasMoreElements():\n\
             \x20   enumeration.nextElement()\n\
             \x20   count += 1\n\
             result = {'paragraphs': count}",
            json!({"paragraphs": 2}),
            json!({"paragraphs": 2, "pages": 1}),
        ),
    ] {
        let created = session.execute(
            creator,
            &format!("desktop.loadComponentFromURL('{factory}', '_blank', 0, ())"),
        );
        ensure!(
            created["status"] == "ok",
            "{creator} creating {factory}: {created}"
        );
        let answer = session.execute(target, probe);
        show(&format!("execute on {target}"), &answer);
        ensure!(answer["status"] == "ok", "{target}: {answer}");
        ensure!(answer["result"] == expected, "{target}: {answer}");
        let described = session.state(target);
        show(&format!("get_state of {target}"), &described);
        ensure!(described["warnings"] == json!([]), "{target}: {described}");
        ensure!(
            described["state"]["active"] == active,
            "{target}: {described}"
        );
        ensure!(
            described["screenshot"]
                .as_str()
                .is_some_and(|path| Path::new(path).exists()),
            "{target}: no capture"
        );
    }
    hub.server.shutdown();
    Ok(())
}

/// Le démarrage à froid : le pont est levé et LibreOffice ne l'est pas, puis LibreOffice démarre.
fn drive_cold_start(base: &Path) -> Verdict {
    let path = std::env::var_os("PATH").ok_or("PATH is not set")?;
    let (uno_port, bridge_port) = (free_port(), free_port());
    let token_file = base.join("token");
    write_file(&token_file, 0o600, FILE_TOKEN);
    let bridge_file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("bridges")
        .join("generated")
        .join("uno_bridge.py");
    let plan = Plan {
        uno_port,
        bridge_port,
        bridge_file,
        token_file,
    };
    // Le pont démarre d'abord : rien n'écoute encore sur le port UNO.
    let (_bridge, bridge_port) = start_bridge(&plan, &path)?;
    let hub = build_with_token(
        base,
        config(&format!(
            "[apps.libreoffice-calc]\nenabled = true\nuno_port = {uno_port}\nbridge_port = {bridge_port}\n"
        )),
        Vec::new(),
        Token::new(FILE_TOKEN).map_err(|error| error.to_string())?,
    );
    let mut session = Session::start(hub.server.clone());
    let announced = |listing: &Value| -> String {
        listing["state"]["modules"][0]["app_version"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };

    // 1. Sans LibreOffice, le pont est là et ne connaît pas la version.
    let early = session.state("hub");
    show("get_state hub, before LibreOffice", &early);
    ensure!(early["state"]["modules"][0]["connected"] == true, "{early}");
    ensure!(announced(&early) == "unknown", "{early}");
    let without = session.state("libreoffice-calc");
    show("get_state, before LibreOffice", &without);
    ensure!(
        without["warnings"]
            .to_string()
            .contains("cannot be imported"),
        "{without}"
    );

    // 2. LibreOffice démarre ; la même connexion du Hub redemande l'état.
    let _libreoffice = start_libreoffice(base, &path, uno_port)?;
    eprintln!("{}", wait_for_libreoffice(&path, uno_port)?);
    let later = session.state("libreoffice-calc");
    show("get_state, once LibreOffice listens", &later);
    ensure!(
        !later["warnings"].to_string().contains("cannot be imported"),
        "the bridge still reports a missing API: {later}"
    );
    ensure!(later["state"]["documents"] == json!([]), "{later}");
    // Le Hub garde la version du hello de sa connexion.
    ensure!(later["app_version"] == "unknown", "{later}");

    // 3. Une nouvelle connexion du Hub annonce la version que le pont connaît maintenant.
    hub.server.shutdown();
    let after = session.state("hub");
    show("get_state hub, on a new connection", &after);
    let version = announced(&after);
    ensure!(
        version.chars().next().is_some_and(|c| c.is_ascii_digit()),
        "the version was not announced on the new connection: {after}"
    );
    hub.server.shutdown();
    Ok(())
}

#[test]
#[ignore = "needs LibreOffice and its python3-uno"]
fn a_bridge_started_before_libreoffice_announces_the_version_once_it_listens() {
    let base = common::private_tempdir();
    must_pass(drive_cold_start(base.path()));
}

#[test]
#[ignore = "needs LibreOffice and its python3-uno"]
fn the_hub_drives_a_real_libreoffice_through_the_generated_bridge() {
    let base = common::private_tempdir();
    must_pass(drive(base.path()));
}
