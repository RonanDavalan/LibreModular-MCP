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
