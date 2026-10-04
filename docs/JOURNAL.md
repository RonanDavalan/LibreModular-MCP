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
