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
