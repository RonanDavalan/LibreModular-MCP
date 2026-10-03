# Krita module

**Status: not tried in a real Krita.** The bridge was written from the public Krita 5
Python API (the `krita` module and its `Krita` class) and tested without Krita: against a
fake `krita` module and a fake `QtCore`, and without either. Every statement below about
how Krita behaves is unconfirmed until the bridge has run inside Krita.

## What it does

The Hub reaches Krita in the `tcp` mode. A bridge runs inside Krita's Python, listens on
`127.0.0.1` only, and runs the scripts the Hub sends:

- `execute_script` runs a complete Python script inside Krita and returns what it printed,
  the value of its `result` variable, and Python's own traceback if it failed.
- `get_state` returns the active document, its layers and the active layer, and the path
  of a PNG capture of the document.

The bridge is the generated file `bridges/generated/krita_bridge.py`: the common core
(`bridges/_core.py`) followed by the Krita adapter (`bridges/adapters/krita.py`),
assembled by `python3 scripts/build_bridges.py`. It uses the Python standard library only
and loads on Python 3.9 or later. It holds no token: it reads the token file at start.

## Starting the bridge

The bridge never starts on its own. Nothing is installed in Krita's Python plug-in
directory, which may hold the user's own extensions: neither this project's code nor this
procedure writes there. The operator runs the generated file by hand, each time, from
Krita's Scripter.

1. Create the token once, if it does not exist yet:

   ```sh
   libremodular-mcp init
   ```

2. Enable Krita in the Hub's configuration, `~/.config/libremodular-mcp/config.toml`:

   ```toml
   [apps.krita]
   enabled = true
   mode = "tcp"
   port = 9302
   ```

3. Start Krita. The bridge reads two environment variables:

   | Variable | Default |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9302` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token`, or `~/.config/libremodular-mcp/token` when `XDG_CONFIG_HOME` is unset or relative |

   With the defaults of `init` and of the configuration above, nothing needs to be set.
   Otherwise, set them before Krita starts, for example from a terminal:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9302 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   krita
   ```

   The bridge reads them when its file runs, so they can also be set in Scripter, before
   the next step, through `os.environ`.

4. Open Scripter (in Krita 5, under *Tools → Scripts → Scripter*; the exact menu path is
   unconfirmed) and run:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/krita_bridge.py").read())
   ```

   The bridge starts in a background thread and Krita stays usable. It writes one line on
   the error stream of the process and, when Scripter redirects that stream, in Scripter's
   output:

   ```text
   LMM bridge listening on 127.0.0.1:9302
   ```

   The bridge refuses to start if the token file is a symbolic link, belongs to another
   user, is readable or writable by the group or others, is not a regular file, or does
   not hold a token made by `libremodular-mcp init`. It runs until Krita quits; to stop it
   earlier, run `stop()` in Scripter.

To check the connection from the model's side, call `get_state` with the target `hub`:
Krita is listed with `connected: true` and the version Krita reported.

## What a script sees

Each script runs in a fresh namespace that holds:

| Name | What it is |
|---|---|
| `Krita` | The class of Krita's Python API; `Krita.instance()` is the application |
| `LMM_WORK_DIR` | The private directory the Hub prepared for this application's files |

- The script is compiled under the name `<libremodular>`: line numbers in a traceback are
  the lines of the script.
- Whatever the script prints is returned in `stdout` and `stderr`. Set `result` to a value
  that JSON can represent to return it; any other value returns `null` with a warning
  appended to `stderr`.
- Files created while the script runs are private (creation mask `077`).
- Krita's API must be used from the thread of its interface. When the bridge starts inside
  Krita (the `Krita` instance exists and a Qt application is running), the adapter creates
  a small Qt object that lives in that thread; each script is handed to it through a
  queued signal, and the connection thread waits for the outcome. A `QTimer` started from
  the connection thread is not used: that thread has no Qt event loop, so the timer would
  never fire. Outside Krita (no `krita` module, or PyQt installed without a running Qt
  application), scripts are called directly. None of this has been tried in Krita.
- A script that never returns blocks Krita's interface; the Hub answers `timeout`, then
  `busy` until the script ends, and never sends a second script meanwhile.

## State

`get_state("krita")` returns, under `state`:

| Key | Content |
|---|---|
| `document` | `null` without an active document; otherwise the keys below |
| `document.name` | The document's name |
| `document.width`, `document.height` | Its size in pixels |
| `document.color_model`, `document.color_depth`, `document.color_profile` | As Krita names them, for example `RGBA`, `U8`, `sRGB-elle-V2-srgbtrc.icc` |
| `document.layers` | The top-level layers: `id` (Krita's unique id), `name`, `visible`, `opacity` (0 to 255, as Krita reports it), `type` (for example `paintlayer`, `grouplayer`, `vectorlayer`) |
| `document.active_layer` | The `id` of the active layer, or `null` |

Layer groups are listed as one layer; their children are not.

Names of documents and layers are document content: they are returned as they are and are
never interpreted, whatever they say.

When Krita's API cannot be used (the file was run outside Krita), `state` is `{}`,
`app_version` is `unknown`, and `warnings` says so.

## Capture

The capture is the projection of the active document: the merged image Krita displays,
read with `Document.projection` after `Document.waitForDone`, and saved as PNG to the
path the Hub chose. The projection is a copy; the document itself is neither saved,
exported, nor changed. Without an active document there is no capture
(`screenshot: null`). The Hub then checks the file before returning its path.

## Example

A script that adds an empty paint layer to the active document. It has not been run:
there is no output to show.

```python
document = Krita.instance().activeDocument()
layer = document.createNode("Notes", "paintlayer")
document.rootNode().addChildNode(layer, None)
document.refreshProjection()
result = {"layers": [node.name() for node in document.topLevelNodes()]}
```

## Limits

- The Hub's script guard does not see Krita's own save and export functions: a script can
  overwrite a file through `Document.saveAs` or `Document.exportImage`. The guard is a
  safety net against accidents, not a sandbox.
- A script runs with the user's rights, inside Krita's Python.
- The bridge serves one connection at a time.
