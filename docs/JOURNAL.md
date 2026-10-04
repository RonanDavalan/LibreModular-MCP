# Journal

Append-only product log.

## 2026-10-03 — Clean start in Rust

The hub is rewritten in Rust so that it ships as a single binary with no interpreter to install.
The first Python implementation of the security foundation was removed; its behaviour was ported
to Rust and compared on 119 scripts (see the next entry).
The bridges that run inside the applications stay in Python.

## 2026-10-03 — Security foundation of the Hub, in Rust

The security foundation that existed in Python is ported to Rust: process hardening, the bridge token,
private control files, path confinement, size bounds, the script guard, the single output point, the
audit journal, strict configuration and the `init` command. There is no MCP server, no tool, no bridge
and no application module yet.

Done, with the command that shows it:

- Hardening, token, control files, paths and bounds: `cargo test --locked --test token_and_files`.
- Script guard, with import aliases resolved on the syntax tree: `cargo test --locked --test guard`.
  The false positive rate on the realistic scripts is 0 of 21
  (`cargo test --locked --test guard realistic -- --nocapture`).
- Guard results pinned against the earlier Python implementation on 98 scripts, 95 identical and 3 that
  differ on purpose (`rmdir` is only refused on an object recognised as a path):
  `cargo test --locked --test guard_golden`.
- Configuration read as a control file, strict by default: `cargo test --locked --test config`.
- Single output point and audit journal: `cargo test --locked --test output --test audit`.
- `init` and the two binaries: `cargo test --locked --test init`.
- Mechanical invariants, replayed in fresh processes where an ordering or a permission is observable:
  `cargo test --locked --test invariants`.
- Whole suite, formatting, lints and release build:
  `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --release --locked`.

Decisions taken while porting:

- The configuration file is now a control file: it is refused when it is a link, belongs to someone else,
  carries a group or other permission bit, or exceeds 1 MiB. The Python version read it directly.
- A configuration error reports a kind and a position, never text from the file, because the parser's own
  message quotes the surrounding lines.
- The guard runs on its own thread. The stack is 16 MiB plus 512 bytes per byte of script, measured worst
  case being about 128 bytes per byte, and a script that would need more than 1 GiB (a little under 2 MiB of text)
  is refused instead of attempted. A panic of the parser is caught and refuses the script.
- Bracket nesting deeper than 200 is refused: CPython itself rejects it, so no valid script is lost.
- Name resolution has a work budget of two million steps, and a name met on its own is resolved once.
  A script that exhausts the budget is refused as too complex to analyse.
- `rmdir` and `unlink` are refused only on an object recognised as a path. The Python version refused any
  `.rmdir`, which also caught unknown objects.
- The token type prints `Token([redacted])` and has no `Display`. The token is not wiped from memory after
  use: that needs a crate outside the approved list or unsafe code.
- Output objects serialise with sorted keys; the journal writes its fixed keys in a fixed order.

Not covered yet:

- The guard cannot see `runpy`, `pickle`, `http.server`, the save functions of the applications, or a path
  passed as a function parameter. It is a safety net against accidents, not a sandbox.
- A script that ruff's parser rejects but CPython would accept is not analysed, because an unparsable
  script is passed on to the application for its own syntax error.
- Nothing has been run against a real application.

## 2026-10-03 — The Hub serves MCP against a reference bridge

The Hub now serves MCP on stdio with exactly two tools, `execute_script` and `get_state`. It reaches
bridges through a line-delimited JSON client on 127.0.0.1, routes only to enabled applications, and
is tested end to end against a reference bridge written for the tests. No application module and no
bridge exist yet: GIMP, Krita and Blender use the generic `tcp` module; Inkscape and LibreOffice,
when enabled, answer `unavailable` with "module not available in this build".

Done, with the command that shows it:

- Bridge client: loopback only, hello with the token, bounded lines, one request in flight, busy after
  a timeout until the late answer arrives: `cargo test --locked --test protocol`.
- Every status (`ok`, `error`, `timeout`, `busy`, `refused`, `unavailable`), the guard and its bypass,
  the journal checked before sending, truncation, the token kept out of answers and journal, captures
  and their purge: `cargo test --locked --test server`.
- The binary on stdio prints only protocol lines and refuses to start (exit code 1, no token shown)
  without a token, with a token readable by others, with a volatile directory open to others, or
  without `XDG_RUNTIME_DIR`: `cargo test --locked --test stdio`.
- Single output point, network confined to `protocol.rs` and 127.0.0.1, modules isolated from the
  security and output modules, nothing but `cli.rs` on stdout: `cargo test --locked --test invariants`.
- Whole suite, formatting, lints and release build:
  `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --release --locked`.

Protocol details fixed while writing the client:

- The `hello` answer must carry `"protocol": 1` besides `ok`, `app`, `app_version` and
  `bridge_version`; any other answer, a malformed line or a closed connection fails the handshake.
- Request ids are `r1`, `r2`, ... per connection. An answer with another id breaks the connection.
- A bridge that cannot take a request answers `{"id": ..., "ok": false, "busy": true}`; the Hub
  reports `busy`.
- A `ping` request answers `{"id": ..., "ok": true}`; it is the only thing `health()` sends.

Decisions:

- rmcp 3.5.0 with `default-features = false` and the features `server`, `macros` and
  `transport-io`: the tool macros and the stdio transport, nothing for HTTP, clients or
  authentication. tokio 1.53.2 with `rt` only; the tests add `io-util` to speak JSON-RPC lines in
  memory. Discarded: rmcp's `client` feature for the tests, raw JSON-RPC lines are enough.
- The bridge client uses blocking `std::net` sockets, called from `spawn_blocking`. It keeps its own
  line buffer, so it can bound a line, look for a late answer without blocking, and close the socket
  from another thread. Discarded: `tokio::net`, which adds the `net` feature for no gain here.
- The token is not wiped from memory after use: that needs unsafe code or a crate outside the
  approved list. The `Token` type hides its value from `Debug`, has no `Display`, and the process
  never writes a core dump.
- Answers keep their fixed keys. `reason` also comes with `busy`, `timeout` and `unavailable`, which
  the Hub produces itself. Guard warnings are appended to `stderr`, prefixed with
  `libremodular-mcp guard warning:`, since the answer has no warnings key.
- With `allow_dangerous_scripts = true`, every `execute_script` is journalled with `guard: bypassed`,
  whether or not the guard found something.
- `get_state("hub")` lists the enabled applications with the same keys as any `get_state`, its
  `content_origin` being `hub`, and the list under `state.modules`.
- A capture reported outside the captures directory is refused but never deleted: a bridge must not
  be able to make the Hub delete a file elsewhere. A refused capture inside the directory is deleted;
  a link is removed without touching its target. An accepted capture is reduced to mode 600.
- A client that closes stdin before `initialize` ends the process with exit code 1 (session failed).

Not covered yet:

- No bridge and no application module: the Python bridges and the GIMP, Krita, Blender, Inkscape and
  LibreOffice modules come next. Nothing has been run against a real application.
- On SIGTERM or SIGINT the process ends without the shutdown purge; captures older than an hour are
  purged at the next start.
- The hello does not check that the bridge reports the application the port was configured for.

## 2026-10-03 — Bridge core and GIMP bridge

The Python side exists: a common core for every bridge of the `tcp` and `uno` modes, a GIMP
adapter, and a build script that joins them into the single file GIMP loads,
`bridges/generated/gimp_bridge.py`. The Hub's `tcp` module drives that file end to end. Nothing
has been run inside a real GIMP: the adapter is written from the public GIMP 3.0 API.

Done, with the command that shows it:

- The Hub refuses a bridge that announces another application than the one configured on its
  port, without repeating the announced name: `cargo test --locked --test protocol --test server`.
- On SIGTERM and SIGINT the Hub purges the captures, releases its connections and exits with
  code 0, after `libremodular-mcp: stopped on SIGTERM` (or `SIGINT`) on the error stream:
  `cargo test --locked --test stdio`.
- Bridge core: loopback only, one connection at a time, five seconds for the hello, token
  compared in constant time, silent refusal of a bad hello, lines bounded to 8 MiB without
  reading a longer one, token file refused when it is a link, another user's, open to others
  or not a regular file, scripts compiled as `<libremodular>` in a fresh namespace with output
  captured and the creation mask at 077, no effect on import:
  `python3 -m unittest discover -s tests/bridges` (69 tests, also run on Python 3.9.23).
- GIMP adapter with a fake `gi` package and without `gi`; generated file current, compilable
  for Python 3.9, unchanged by regeneration, without token: same command.
- Both, from cargo, which fails when Python is missing, and the generated GIMP bridge driven by
  the Hub's `TcpModule`: `cargo test --locked --test bridges`.
- Token comparison and listening sockets only in `bridges/_core.py`, generated files made of
  the core and the adapter only, no address but 127.0.0.1 in `bridges/`, no outgoing connection
  or process launch in the core: `cargo test --locked --test invariants`.
- Whole suite, formatting, lints, release build, bridges:
  `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --release --locked && python3 -m compileall -q bridges && python3 scripts/build_bridges.py --check`.

Decisions:

- tokio gains the `signal` feature, which brings `mio` and `signal-hook-registry` as transitive
  dependencies. The wait on the session and on the signals is written with `std::future::poll_fn`,
  so tokio's `macros` feature is not needed.
- `BridgeClient::connect` and `BridgeConnector::new` take the expected application name; the
  registry passes the name from its table of known applications.
- The bridge reads the application version once, when it starts, in the thread that starts it:
  the hello never calls the application from a connection thread.
- The core reports an application whose API cannot be imported by a warning in `state`; it
  recognises it by the version `unknown`, which every adapter returns in that case.
- A script's exceptions, `SystemExit` included, are caught inside the call handed to the main
  thread, and the traceback starts at the first frame of the script.
- Each captured stream keeps at most 512 Ki characters, and an answer that would exceed the
  line limit is reduced (result dropped, texts cut) rather than sent: the Hub would otherwise
  drop the connection.
- Run as a program, a generated bridge serves until stopped; run from a console with `exec`, it
  returns at once. It tells the two apart by comparing its compiled file name with `sys.argv[0]`.
- The default token path follows `XDG_CONFIG_HOME`, then `~/.config`. The bridge does not read
  `LIBREMODULAR_CONFIG_DIR`: `LIBREMODULAR_TOKEN_PATH` is its explicit override.
- The capture exports a flattened copy of the active image, so that the user's image keeps the
  export file name GIMP remembers. The active image is the first one `Gimp.get_images()` lists.

Not covered yet:

- Nothing has run inside GIMP: the console gesture, `GLib.idle_add` as the way to GIMP's main
  thread, `Gimp.get_images`, `Gimp.Selection.bounds`, `Gimp.file_save` with `None` options and
  the enumeration names are unconfirmed.
- A signal that arrives while the Hub is still starting, before the signal handlers are in
  place, ends the process without the shutdown purge.
- The bridge does not lower `RLIMIT_CORE`: it runs inside the application's process.
- A reconnection made right after the previous connection closed can be refused for a few
  milliseconds, until the bridge notices the closure and frees its single place.

## 2026-10-04 — The GIMP bridge tried in GIMP 3.0.4, and a hang outside GIMP fixed

The bridge ran in GIMP 3.0.4 (Debian 13 package) started without its interface
(`gimp-3.0 --new-instance --no-interface --batch-interpreter python-fu-eval`, a Python main
loop kept running after the bridge started), driven by the Hub over MCP on stdio:

- `get_state("hub")` lists `gimp` as connected, version `3.0.4`, bridge version `1`.
- A script that creates an image and a layer returns `status: ok` with the layer name in
  `result`; the next `get_state("gimp")` lists the image (`color_mode: rgb`), the layer
  (`blend_mode: normal`, `opacity: 100.0`, `visible: true`), `selection: null`, and a PNG
  capture of 200 by 100 pixels in mode 600.
- A script that fails returns `NameError` with the traceback line 2 of `<libremodular>`.

Fixed: on a machine where `gi` and the GIMP typelib are installed, a bridge run as a plain
Python program (as the tests do) queued its calls with `GLib.idle_add` and waited for a main
loop that never ran, so `execute` and `state` never answered. Outside a process GIMP started,
`Gimp.version()` is `None`: the adapter now calls directly, and answers `state: {}` with the
usual warning, without asking GIMP anything. Tests: `test_run_on_main_is_direct_when_gi_loads_but_the_process_is_not_gimp`,
`test_outside_gimp_nothing_asks_gimp_even_when_gi_loads` (the first fails on the earlier adapter).
The end-to-end Rust test now says which step failed.

Still unconfirmed: starting the bridge from the interactive Python console (menu path, the
environment variables reaching it, its lifetime when the console closes), several open images
(which one is listed first), a non-empty selection.

## 2026-10-04 — Krita and Blender bridges, the Inkscape module, a reconnection delay

Three more applications: Krita and Blender in the `tcp` mode, each with an adapter and a
generated bridge, and Inkscape in the `cli` mode, a Rust module that starts one Python process
per script. None of them has been tried in the real application.

Done, with the command that shows it:

- The `tcp` module waits 100 ms before its second attempt to connect, so a bridge that frees
  its single place late is not reported unavailable: `cargo test --locked --test protocol`
  (`health_reaches_a_bridge_that_frees_its_place_late`, and its counter-proof without the wait).
- Blender adapter: one persistent timer, registered once from the thread that runs the bridge
  file, drains a queue in Blender's main thread; outside Blender's interface (background mode,
  the stand-alone `bpy` package) calls are direct. Krita adapter: a Qt object living in the
  interface thread receives calls through a queued signal; without a Krita instance and a Qt
  application, calls are direct. Both: `python3 -m unittest discover -s tests/bridges`
  (90 tests, also on Python 3.9.23), including a double whose mechanism never runs anything,
  and its counter-proof that always queues and waits forever.
- The three generated bridges driven end to end by the Hub's `TcpModule`, passing whether or
  not the application's module is installed: `cargo test --locked --test bridges`.
- Inkscape module: argument list without shell, new process group, environment cleared to
  `PATH`, `HOME`, `LANG`, `LC_ALL`, `PYTHONPATH`, `LMM_DOCUMENT`, `LMM_WORK_DIR`, script in a
  private file, limits applied by a fixed bootstrap, whole group killed on timeout, output
  bounded without blocking, entities refused when reading the state, a linked document refused:
  `cargo test --locked --test inkscape` (24 tests, with the test's Python and a fake
  `inkscape`).
- Private work files created once or replaced without following a link:
  `cargo test --locked --test token_and_files`.
- No process started in `src/` outside the Inkscape module, which answers only through the
  `Module` trait: `cargo test --locked --test invariants`.
- Whole suite, formatting, lints, release build, bridges:
  `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --release --locked && python3 -m compileall -q bridges && python3 scripts/build_bridges.py --check`.

Decisions:

- Blender: a single persistent timer registered from the main thread, rather than one
  registration per call from the connection thread. Blender's own documentation shows the
  queue-and-timer pattern for work coming from another thread; whether registering a timer
  from another thread is safe is unconfirmed. `app_version` is the first word of
  `bpy.app.version_string`, so `4.2.3 LTS` reads `4.2.3`.
- Krita: a queued signal to an object of the interface thread, rather than `QTimer.singleShot`,
  which, called from a thread without a Qt event loop, would never fire. The capture saves
  `Document.projection`, a copy; the document is never saved or exported.
- Inkscape: the module never opens a file. The registry gives it the Hub's file operations
  (`ensure_private_file`, `replace_private_file` in `security.rs`) through a `WorkFiles`
  trait, since modules may not import the security module. The process environment values
  are read by `config.rs`, the only reader of the environment.
- Inkscape: the work directory is the one the Hub passes to `execute`; the module remembers
  it for `state`, which has no work directory in its signature. Before any script, `state` is
  `{}` with a warning. The script file is removed once its process ends.
- Inkscape: the bootstrap is given the script file and the limits as arguments (CPU seconds,
  address space, open files), since the environment is fixed. It drops the current directory
  from `sys.path` before importing anything, so a file a script left in the work directory
  cannot shadow a module the bootstrap needs. `SystemExit` is reported as an error, as in the
  bridges. The address space limit is 2 GiB, open files 256.
- Inkscape: the process group is killed after every run, not only on timeout; the leader is
  left unreaped (`waitid` with `WNOWAIT`) until then, so its group id cannot be reused. A
  process that leaves its group on purpose is not followed.
- Inkscape: the capture runs `inkscape` with the reduced environment and its own process
  group, without the resource limits of scripts.
- The test of applications without a module now uses LibreOffice, since Inkscape has one.

Not covered yet:

- Nothing has run in a real Krita, Blender or Inkscape: the Scripter gesture and the queued
  signal, `blender --python` and the timer, whether `blender --background --python` keeps
  running long enough for requests, `Document.projection`, `inkex` on `PYTHONPATH`, and the
  `inkscape` command line are unconfirmed.
- `execute` in the `tcp` module still reconnects at once after a connection lost before the
  request was sent; only `health` and `state` wait.

## 2026-10-04 — Blender, Krita and Inkscape tried, and an Inkscape hang fixed

The three modules ran against the real applications on a Debian 13 machine with 32 cores,
driven by the Hub over MCP on stdio, in a throwaway home directory with its own token
(Inkscape 1.4, Blender 4.3.2, Krita 5.2.9; the interface ones on a virtual X display):

- Inkscape: the example of `docs/modules/inkscape.md` returned `{"layers": ["Layer 1", "Notes"]}`;
  `get_state` listed the layers and a PNG capture; a second script found the document as the
  first had left it; an error returned the right traceback line.
- Blender: both paths ran, the direct call without the interface (1 ms) and the persistent
  timer with it (about 70 ms); the state listed the default scene and the new object.
- Krita: the queued signal to the interface thread ran (1 to 7 ms); the state listed the
  document and its layers, and the capture was a 300 by 200 PNG in mode 600. The bridge was
  started by a startup extension of the throwaway profile, not from Scripter.

Fixed: `import inkex` imports numpy, and its BLAS library starts one thread per core. Under
the module's 2 GiB address-space limit, thirty-two threads reserving their stacks never
finished starting: the first script that imported `inkex` ended in `timeout` after 30 s
(reproduced outside the Hub with `RLIMIT_AS` alone; `OPENBLAS_NUM_THREADS=1` removes it).
The cloud environment has four cores and could not show it. The module now sets
`OPENBLAS_NUM_THREADS=1` and `OMP_NUM_THREADS=1`. Test:
`the_process_sees_only_the_allowed_variables_and_never_the_token` now also requires both
variables to be `1`, and fails without the fix.

Still unconfirmed: starting the Blender and Krita bridges from the consoles of an interface
the operator opened; `bpy.ops` from the bridge; several documents open in Krita.

## 2026-10-04 — LibreOffice: three targets, one bridge, a launcher, and two corrections of the Hub

Writer, Calc and Impress are now served by one stand-alone bridge that connects to LibreOffice's
UNO port, behind three Hub targets that share one connection. `libremodular-launch libreoffice`
starts LibreOffice and the bridge, or stops the bridge. Two defects found on the operator's
machine were fixed first. The whole of it ran against a real LibreOffice 24.2.7.2 (not 25.2.3.2).

Done, with the command that shows it:

- `Module::state` receives the work directory of the application, prepared by the server like
  the one `execute` gets. An Inkscape module that has run no script describes the `current.svg`
  a previous Hub left; without one the state stays `{}` and the document is not created:
  `cargo test --locked --test inkscape --test server`
  (`a_new_module_describes_the_document_a_previous_hub_left` and its counter-proof, which models
  the previous behaviour; `get_state_prepares_the_private_work_directory_and_hands_over_the_one_execute_gets`).
- The second attempt of `TcpModule::execute`, after a connection found gone before anything was
  written, waits the reconnect delay: `cargo test --locked --test protocol`
  (`execute_waits_before_its_second_attempt_to_connect`, which fails with
  "the bridge closed the connection during the handshake" once the wait is removed, and its counter-proof).
- The bridge core hands the family of a request to an adapter that declares `families`, refuses
  an unnamed or unserved family with `UnknownFamily` before any line of the adapter runs, and
  leaves the other adapters untouched: `python3 -m unittest discover -s tests/bridges`
  (`FamilyTests`; 145 tests in all, run with `-W error::ResourceWarning`).
- The UNO adapter and its generated bridge, with `uno` and `com.sun.star.beans` imported when a
  request needs them: the same tests pass with `python3 -S` (no `uno` anywhere) and with
  `PYTHONPATH=/usr/lib/python3/dist-packages LD_LIBRARY_PATH=/usr/lib/libreoffice/program`
  (the real `uno` importable), since they install a fake. `python3 -m unittest discover -s tests/bridges`
  (`test_uno_adapter.py`).
- The Hub module: one connection for the three targets, the family in every request, a shared
  busy state, `get_state("hub")` listing the three in mode `uno`, a harmless `close()`:
  `cargo test --locked --test libreoffice`. The generated bridge, started by a Python that cannot
  import `uno`, answers for the three targets (`LibreOfficeUnavailable`, state `{}` with a warning,
  version `unknown`): `cargo test --locked --test bridges`.
- The launcher: argument lists without a shell, `127.0.0.1` only, the token in no argument and no
  variable (only the path of its file), the bridge's environment exactly the listed one, detached
  processes in groups of their own, a refusal beside a running `soffice.bin` (read from a fake
  `/proc`), a private record, `--stop` that spares a process number another program has taken:
  `cargo test --locked --test launch` (38 tests, 12 runs in a row without a failure). The helpers
  it needed in `security.rs`: `cargo test --locked --test token_and_files`. Only the Inkscape module
  and the launcher start processes: `cargo test --locked --test invariants`.
- A real LibreOffice, without interface: `cargo test --locked --test libreoffice_real -- --ignored --nocapture`
  (ignored by default, `needs LibreOffice and its python3-uno`). With a window, on a virtual X display,
  the real `libremodular-launch`, then the real `libremodular-mcp` over MCP on stdio: the three targets
  connected with version `24.2.7.2`, a script refused with `NoActiveDocument`, a Calc script run, the state
  and a PNG capture returned, `WrongDocumentType` for Writer, `--stop` ending the bridge and leaving
  LibreOffice alone.
- Whole suite, formatting, lints, release build, bridges:
  `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --release --locked && python3 -m compileall -q bridges && python3 scripts/build_bridges.py --check`.

Decisions:

- Sharing the connection: `TcpModule` gains an optional `family` and a connection slot other modules
  can hold (`SharedConnection`); its reconnection logic stays in one place and its behaviour without them
  is unchanged. `LibreOfficeModule` is a thin wrapper over it. The registry makes one slot per bridge port.
- The core gives an adapter no way to add a warning to a state answer, and changing that was outside this
  work. `active: null` therefore reaches the Hub without a reason, and `LibreOfficeModule` adds the warning
  itself, naming both causes (no current document, or one of another family).
- The bridge file is checked by a new `security::check_trusted_file` (absolute, regular, not a link, ours,
  not writable by the group or others; readable by everyone, it is code). Files under `/proc` are read by
  `security::read_system_file`, one bounded read without following links. Both are listed as definitions
  that may exist only in `security.rs`.
- LibreOffice receives the environment of the launcher except the `LIBREMODULAR_` variables: a window needs
  the display and the session bus, and no application needs this program's own variables. The environment is
  passed in as a parameter (`config::environment_snapshot` in production), like the programs and `/proc`.
- The record is `soffice <pid>` and `bridge <pid> <bridge file>`. The bridge is recognised when one of the
  arguments of its command line is exactly the bridge file. Exit code `2` is a usage error or a refusal the
  operator can fix, `1` a failure.
- Two refusals beyond the ones asked for: equal `uno_port` and `bridge_port`, and a bridge of an earlier
  launch that still runs (a second launch would leave it unrecorded and impossible to stop).
- The test reference bridge gained a single-place mode, as a real bridge refuses a second connection at
  once, and can announce `libreoffice`. Fake programs of the launcher tests end when their directory goes.
- `NOT_IN_BUILD` is no longer reachable from a configuration, since every known application has a module;
  it stays for an application that would not have one.
- In the build environment (not in the repository) `libreoffice-writer`, `libreoffice-calc`,
  `libreoffice-impress` and `xvfb` were installed with the package manager to run the real tests.

Findings, not fixed:

- With no document open, every script is refused with `NoActiveDocument`: the Hub cannot create the first
  document, and `soffice --headless --calc` opened none. The real test opens it with a separate UNO snippet.
- The core reads the application's version once, when the bridge starts. A bridge started before LibreOffice
  listens (the launcher starts them one right after the other, and the bridge waits five seconds) reports
  `unknown` for good, and every state then carries the warning that the API cannot be imported.
  LibreOffice listened 1.3 s after its start in one run, and after 8 s but by 14 s on its first start.
- The errors the adapter raises itself (`NoActiveDocument`, `WrongDocumentType`, `LibreOfficeUnavailable`)
  come back with the frames of the bridge and the path of the bridge file in their trace.
- The directory that holds the bridge file is not checked: only the file is.
- The process the launcher records is LibreOffice's start process, not the `soffice.bin` the system lists.

Not covered yet:

- LibreOffice 25.2.3.2, the interface on a real desktop session, Python 3.13.5 of Debian 13, what the
  PNG capture of a document with several sheets or pages shows, which document is current when the focus
  is on another window.
- The UNO port has no authentication: any process that can reach it drives LibreOffice without the token,
  the guard or the journal. Whether LibreOffice filters by user is unconfirmed.

## 2026-10-04 — LibreOffice tried in LibreOffice 25.2.3.2

- `cargo test --locked --test libreoffice_real -- --ignored --nocapture` passes against LibreOffice
  25.2.3.2 (Debian 13 packages, `/usr/bin/python3` 3.13.5), 1 test in 3.1 s, no process left.
- The real `libremodular-launch libreoffice` and the real `libremodular-mcp serve` on a virtual X
  display, in a throwaway home: `get_state("hub")` lists the three targets connected in mode `uno`
  with `app_version` `25.2.3.2` and `bridge_version` `1`; a Calc script is refused with
  `NoActiveDocument` while no document is open; after a Calc document is opened, a script writes
  `A1` and `B2` and returns them, `get_state("libreoffice-calc")` describes the sheet and returns a
  PNG path, a Writer script is refused with `WrongDocumentType`; `--stop` ends the bridge and
  leaves LibreOffice running. The sheet is named `Feuille1` there: names follow the locale.
- A bridge file checked out under a `umask` of `002` has mode `664` and the launcher refuses it
  (`chmod go-w`); the guide now says so.


## 2026-10-04 — Two corrections of the LibreOffice bridge, the public files, the invariants, the cold recipe

Two defects found on the operator's machine are fixed, each in its own commit. The repository gets
the files a public repository needs, the mechanical invariants are reviewed and completed, and a
smoke test runs the compiled binary. Nothing is published: no tag, no release.

Done, with the command that shows it:

- The core reads the application version again at each valid hello and at each state while it is
  `unknown`, and keeps the first real answer: `python3 -m unittest discover -s tests/bridges`
  (`VersionTests` of `test_core.py`; counter-proof `core_reading_the_version_once()` of
  `bridge_support.py`, the core as it was). The adapters of GIMP, Krita and Blender and their tests
  are unchanged. The question goes through `run_on_main`, and a hello that does not carry the token
  never reaches the adapter.
- A bridge started before LibreOffice, then LibreOffice, then the state and a new connection:
  `cargo test --locked --test libreoffice_real -- --ignored --nocapture a_bridge_started_before`,
  against LibreOffice 24.2.7.2 (Ubuntu 24.04 packages installed in the build environment, without
  interface). Run against the bridge file of before the change it fails with the warning of the cold
  start still there.
- With no document open a LibreOffice script runs with `doc` set to `None` and can create the first
  document with `desktop.loadComponentFromURL`; the next request receives it; a current document of
  another family still gives `WrongDocumentType` before any line runs; `NoActiveDocument` is gone from
  the adapter, its tests and the guide (history entries above keep the name): `test_uno_adapter.py`
  (`ScriptTests`, with the adapter that refuses as counter-proof) and
  `cargo test --locked --test libreoffice_real -- --ignored --nocapture the_hub_drives`, which now
  creates the Calc, Impress and Writer documents through scripts of the Hub.
- The example of the guide is the script that creates the document and writes two cells; its answer
  comes from the real `libremodular-mcp` driven over MCP on stdio, with LibreOffice 24.2.7.2 in a
  window on a virtual X display, started by the real `libremodular-launch`.
- `README.md`, `docs/GUIDE_LLM.md`, `SECURITY.md`, `LICENSE` (MIT), `.github/workflows/ci.yml`, and
  `scripts/generate_changelog.py` with its test (`tests/bridges/test_changelog.py`: exact output for a
  fixed log, a script that misfiles a title and a check that never fails are detected).
- Invariants: `cargo test --locked --test invariants` (36 tests). New: exact approved dependencies
  pinned as `=x.y.z`; no path other than the `/home/alice` examples and no token, journal, capture or
  operator configuration in the tree; `server.rs` declares exactly two tools and reaches no application
  module; the README never says AI, Windows or macOS; the README and the guides agree on the versions
  and the example. Each has its counter-proof. The check that the single place opening a file does not
  follow links had none: it has one now.
- `tests/smoke.rs`: the compiled binary through `init`, then a session of a script that succeeds, one
  that raises, one refused by the guard and one for an unknown target; four journal lines in order and
  no script text, only the protocol on the standard output, the token only in its own file. Two
  counter-proofs: a journal asked to keep the script text, a token copied into another file.
- Whole suite, formatting, lints, release build, bridges:
  `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked && cargo build --release --locked && python3 -m compileall -q bridges && python3 scripts/build_bridges.py --check`
  (322 Rust tests and 3 ignored, 174 Python tests; the Python ones also on 3.9.23 and with `python3 -S`).
- From a fresh clone made under a `umask` of `002`, with the commands of the README:
  `cargo build --release --locked`, `target/release/libremodular-mcp init`, `cargo test --locked`,
  `python3 -m unittest discover -s tests/bridges`, `python3 scripts/build_bridges.py --check`.

Decisions:

- The version is asked through `run_on_main`, as the earlier entry on the bridge core decided for the
  hello: the application is not called from a connection thread. In `state` it is asked after the
  adapter's own call, which may have waited for the application, and the warning goes first.
- The UNO adapter makes one attempt to read the version instead of waiting up to five seconds. With the
  re-read, a wait there would have been paid at every hello while LibreOffice is down, and the Hub gives
  up on a hello after five seconds: measured, 5.0 s. The bridge no longer waits at its start either.
- The launcher said "a script only runs on the active document": one line of its output now says what
  is true. No other behaviour of the Hub changed.
- `server.rs` does import from `security`: it chains the checks. The invariant pins the exact list of
  items it takes, and that it defines none, instead of forbidding the import.
- The README states the launcher check as the code makes it: a bridge file the group or others can
  write is refused, mode 644 is accepted.
- The changelog lists titles under a single `Unreleased` section, sorted by their first word.

Findings, not fixed:

- The Hub keeps the version of the hello of its current connection: after the bridge has found the
  version, `get_state` still says `unknown` until the Hub connects again (shown by the real cold start
  test, which asserts it).
- The errors the LibreOffice adapter raises itself still come back with the frames of the bridge file in
  their trace, and the directory that holds the bridge file is still not checked.
- `CHANGELOG.md` is built from the history: any commit after it makes `python3 scripts/generate_changelog.py --check`
  fail until the file is generated again, which is the point of that check before a publication.

Not covered yet:

- LibreOffice 25.2.3.2 for the two corrections, and a launcher start where LibreOffice is slow: it
  answered on UNO about one second after its start here.
- The CI workflow has never run on the forge. Private vulnerability reporting, which `SECURITY.md`
  names, must be enabled in the repository settings before publication; it cannot be checked from here.
