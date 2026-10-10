# LibreModular-MCP

## Universal MCP server written in Rust, with a Hub that exposes only two tools

An MCP server that lets a language model drive desktop applications by running scripts in them.

## Quick start

Build the Hub, let it create its configuration and its token, then declare it in your MCP client.

```sh
cd /home/alice/LibreModular-MCP
cargo build --release --locked
target/release/libremodular-mcp init
```

The entry for the MCP client runs the compiled binary with the argument `serve`:

```json
{
  "mcpServers": {
    "libremodular": {
      "command": "/home/alice/LibreModular-MCP/target/release/libremodular-mcp",
      "args": ["serve"]
    }
  }
}
```

What to adapt: `/home/alice/LibreModular-MCP/` is the absolute path of your clone. Which file this
entry goes into, and what its keys are called, depends on your client: the shape above is a common
one, and the Hub was tried with a client written for its tests (JSON-RPC lines on its standard
input and output), not with a named MCP client. The Hub needs the variable `XDG_RUNTIME_DIR`,
which a desktop session sets; without it, it refuses to start.

`init` creates `~/.config/libremodular-mcp/config.toml` and the token file next to it, both in
mode 600. It never overwrites the configuration, and replaces the token only with `init --rotate`.
Nothing is enabled afterwards: set `enabled = true` for the
application you want in `config.toml`, then start its bridge as its guide says (see the table
below).

## What it is

The Hub is one binary that gives the model exactly two tools, `execute_script` and `get_state`.
The model writes one complete script; the Hub sends it to a bridge that runs it inside the
application, next to it, or as one process per script, and the model gets back what the script
printed, its result, or the application's own error trace to correct. The Hub knows no
application's interface: each application has a module on the Hub side and a bridge on the
application side, and the bridges are Python files whose only dependencies are the standard
library and the application's own Python interface.

## The two tools

`execute_script(target_app: str, script_content: str) -> str` runs a complete Python script in the
application named by `target_app`. It returns the text of a JSON object with `status` (`ok`,
`error`, `timeout`, `busy`, `refused` or `unavailable`), `app`, `duration_ms`, `stdout`, `stderr`,
`result`, `error` (`null`, or `type`, `message` and `traceback`, the application's own),
`truncated`, and `reason` when the Hub itself refused the script or could not run it.

`get_state(target_app: str) -> dict` returns `app`, `connected`, `app_version`, `bridge_version`,
`captured_at`, `state` (what is specific to the application), `content_origin`, `screenshot` (the
path of a PNG file, or `null`) and `warnings`; the answer reaches the client as the text of a JSON
object. The target `hub` returns the list of configured applications, how each one is reached, and
whether it is connected, without contacting any application.

`target_app` must be the exact name of an enabled application. Neither tool accepts anything else.

## Applications

Linux only. The versions are those the guides report as tried; the guide of each application says
under which conditions, and what was not part of the run.

| Target | Application | Mode | Version tried | Guide |
|---|---|---|---|---|
| `gimp` | GIMP | `tcp` | 3.0.4, started without its interface | [docs/modules/gimp.md](docs/modules/gimp.md) |
| `krita` | Krita | `tcp` | 5.2.9, not started from Scripter | [docs/modules/krita.md](docs/modules/krita.md) |
| `blender` | Blender | `tcp` | 4.3.2 | [docs/modules/blender.md](docs/modules/blender.md) |
| `inkscape` | Inkscape | `cli` | 1.4 | [docs/modules/inkscape.md](docs/modules/inkscape.md) |
| `libreoffice-writer`, `libreoffice-calc`, `libreoffice-impress` | LibreOffice | `uno` | 24.2.7.2 and 25.2.3.2 | [docs/modules/libreoffice.md](docs/modules/libreoffice.md) |

The modes: `tcp`, a bridge loaded in the application listens on a local port; `uno`, a stand-alone
bridge connects to the UNO port LibreOffice listens on; `cli`, one process runs each script.

In the `tcp` and `uno` modes the Hub never starts, closes or stops an application on the model's
request: bridges and applications start on an explicit gesture of the operator, described in each
guide, and `libremodular-launch libreoffice` starts LibreOffice and its bridge (the other
applications are started by hand). In the `cli` mode the Hub starts one Python process per script,
and runs `inkscape` once to export a capture; it never touches an Inkscape window the operator
opened.

## Example

A Calc script that creates the document when none is open, writes two cells and returns what it
wrote, and the answer of the Hub. It ran as written, with no document open, in LibreOffice
24.2.7.2 (the Ubuntu 24.04 packages, with a window on a virtual X display), through the real
`libremodular-mcp`. The same example is in [docs/modules/libreoffice.md](docs/modules/libreoffice.md).

```python
if doc is None:
    doc = desktop.loadComponentFromURL('private:factory/scalc', '_blank', 0, ())
sheet = doc.Sheets.getByIndex(0)
sheet.getCellByPosition(0, 0).setValue(42)
sheet.getCellByPosition(1, 1).setString('hello')
print('wrote A1 and B2')
result = {'sheet': sheet.Name, 'a1': sheet.getCellByPosition(0, 0).getValue(), 'b2': sheet.getCellByPosition(1, 1).getString()}
```

```json
{
  "app": "libreoffice-calc",
  "duration_ms": 170,
  "error": null,
  "result": {
    "a1": 42.0,
    "b2": "hello",
    "sheet": "Sheet1"
  },
  "status": "ok",
  "stderr": "",
  "stdout": "wrote A1 and B2\n",
  "truncated": false
}
```

## What leaves the machine

Everything the Hub returns goes to the language model, and so to whoever runs it.

| What | Leaves the machine |
|---|---|
| The script the model wrote | Yes: the model is its author |
| `stdout`, `stderr` and `result` of a script | Yes: they can hold the content of a document |
| The error trace | Yes: it holds paths of the machine |
| `state` | Yes: it holds content taken from the open documents |
| A capture | Only if the model reads the file |
| The audit journal, the token and the configuration | Never |

A user who opens a confidential document in a connected application knows that its content can
be read by the model.

The Hub opens no connection other than the ones to bridges on `127.0.0.1`: a test reads the source
for any other (`cargo test --locked --test invariants`).

## Limits

- The Hub is not a sandbox. A script runs with the rights of the user, inside an application
  whose Python can do whatever the user can.
- The script guard stops a clumsy script, not a determined one. It reads the text of a script
  before it is sent; it does not see `runpy`, `pickle`, `http.server`, the save functions of the
  applications, or a path passed to a function as a parameter.
- LibreOffice's UNO port has no authentication: any process that can reach it on this machine
  drives LibreOffice without the token, the guard or the audit journal.
- Another process of the same user can read the token file.
- In the `tcp` and `uno` modes, a script that loops keeps the application (for LibreOffice, its
  bridge) busy until it is closed: the Hub answers `timeout`, then `busy`, and does not stop it.
  Only the `cli` mode kills a script that runs past its time.
- `libremodular-launch` refuses a bridge file that the group or others can write. A clone made
  with a `umask` of `002` gives mode 664: run `chmod 600` on the bridge file once. Mode 600 and
  mode 644 are accepted.

## Tests

```sh
cargo test --locked
python3 -m unittest discover -s tests/bridges
python3 scripts/build_bridges.py --check
```

Tests that need a real application are marked `#[ignore]` with their reason and are run on
purpose, for example `cargo test --locked --test libreoffice_real -- --ignored --nocapture`.

## Credits

LibreModular-MCP is built in an asymmetric human–LLM collaboration. The roles are written down as they
were actually played; the full list is in [CONTRIBUTORS.md](CONTRIBUTORS.md).

- **Ronan Davalan**: architect and arbiter. Initiative, priorities, security requirements, validation and testing.
- **Claude Code (Anthropic)**: systems engineer and lead developer. Working framework, review and integration of the code, documentation.
- **Claude Cloud (Anthropic)**: wrote the code on branches from prompts, reviewed before every merge.
- **Gemini (Google)**: architectural advisor and synthesizer; wrote the founding note and proposed the project's name.

## Security and licence

Report a vulnerability as [SECURITY.md](SECURITY.md) says. The code is under the MIT licence, see
[LICENSE](LICENSE).
