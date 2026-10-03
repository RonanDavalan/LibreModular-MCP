# GIMP module

**Status: tried in GIMP 3.0.4 started without its interface, not yet in the interactive
Python console.** The bridge was written from the public GIMP 3.0 API (GObject
Introspection: `gi.repository.Gimp`, `Gio`, `GLib`). It ran in GIMP 3.0.4 (Debian 13
package) started with `gimp-3.0 --new-instance --no-interface --batch-interpreter
python-fu-eval`, with a Python main loop running after the bridge started. In that run,
driven by the Hub over MCP, `get_state("hub")` listed `gimp` as connected with version
`3.0.4`; a script created an image and a layer; `get_state("gimp")` returned the keys
below and a PNG capture of 200 by 100 pixels in mode 600; a script that failed returned
its traceback with the right line of the script. Statements marked unconfirmed below
were not part of that run.

## What it does

The Hub reaches GIMP in the `tcp` mode. A bridge runs inside GIMP's Python, listens on
`127.0.0.1` only, and runs the scripts the Hub sends:

- `execute_script` runs a complete Python script inside GIMP and returns what it printed,
  the value of its `result` variable, and Python's own traceback if it failed.
- `get_state` returns the open images, the layers of the active image, the selection,
  and the path of a PNG capture of the active image.

The bridge is the generated file `bridges/generated/gimp_bridge.py`: the common core
(`bridges/_core.py`) followed by the GIMP adapter (`bridges/adapters/gimp.py`), assembled
by `python3 scripts/build_bridges.py`. It uses the Python standard library only and
loads on Python 3.9 or later. It holds no token: it reads the token file at start.

## Starting the bridge

The bridge never starts on its own. It is not placed in a plug-in or auto-load
directory: the operator starts it by hand, each time, in GIMP's Python console.

1. Create the token once, if it does not exist yet:

   ```sh
   libremodular-mcp init
   ```

2. Enable GIMP in the Hub's configuration, `~/.config/libremodular-mcp/config.toml`:

   ```toml
   [apps.gimp]
   enabled = true
   mode = "tcp"
   port = 9301
   ```

3. Start GIMP. The bridge reads two environment variables, which GIMP passes on to its
   plug-ins, the Python console included (not part of the run above, which set them
   in the process environment of the batch instance):

   | Variable | Default |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9301` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token`, or `~/.config/libremodular-mcp/token` when `XDG_CONFIG_HOME` is unset or relative |

   With the defaults of `init` and of the configuration above, nothing needs to be set.
   Otherwise, set them before GIMP starts, for example from a terminal:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9301 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   gimp-3.0
   ```

   The bridge reads them when its file is run, not when GIMP starts, so they can also be
   set in the console itself, before the next step:

   ```python
   import os
   os.environ["LIBREMODULAR_BRIDGE_PORT"] = "9301"
   os.environ["LIBREMODULAR_TOKEN_PATH"] = "/home/alice/.config/libremodular-mcp/token"
   ```

4. Open the Python console (in GIMP 3.0, under *Filters → Development → Python-Fu*;
   the exact menu path is unconfirmed) and run the generated file:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/gimp_bridge.py").read())
   ```

   The bridge starts in a background thread and the console stays usable. It writes one
   line on the error stream of the process (visible when GIMP was started from a
   terminal) and, when the console redirects that stream, in the console (unconfirmed):

   ```text
   LMM bridge listening on 127.0.0.1:9301
   ```

   The port is the one actually bound. The bridge refuses to start, with a message in the
   console, if the token file is a symbolic link, belongs to another user, is readable or
   writable by the group or others, is not a regular file, or does not hold a token made
   by `libremodular-mcp init`.

5. Keep the console open. In GIMP 3 the Python console is a plug-in that runs in its own
   process; the bridge lives in that process and stops when the console is closed
   (unconfirmed). To stop the bridge without closing the console, run `stop()` in it.

To check the connection from the model's side, call `get_state` with the target `hub`:
GIMP is listed with `connected: true` and the version GIMP reported.

## What a script sees

Each script runs in a fresh namespace that holds:

| Name | What it is |
|---|---|
| `Gimp`, `Gio`, `GLib` | The introspection modules, when they can be imported |
| `images()` | The images open in GIMP, as `Gimp.get_images()` lists them |
| `LMM_WORK_DIR` | The private directory the Hub prepared for this application's files |

- The script is compiled under the name `<libremodular>`: line numbers in a traceback are
  the lines of the script.
- Whatever the script prints is returned in `stdout` and `stderr`. Set `result` to a value
  that JSON can represent to return it; any other value returns `null` with a warning
  appended to `stderr`.
- Files created while the script runs are private (creation mask `077`).
- The bridge hands each script to GIMP's main thread through `GLib.idle_add` and waits for
  it: GIMP's API is not meant to be called from another thread. This worked in GIMP 3.0.4
  with a running main loop; in the interactive console it has not been tried.
  Outside GIMP, even on a machine where `gi` and the GIMP typelib are installed, the call
  is made directly in the bridge's own thread, because no main loop would ever run it. A script that never returns blocks GIMP's main thread; the Hub
  answers `timeout`, then `busy` until the script ends, and never sends a second script
  meanwhile.
- `sys.exit()` in a script ends the script with a `SystemExit` error, not the bridge.

## State

`get_state("gimp")` returns, under `state`:

| Key | Content |
|---|---|
| `images` | Every open image: `id`, `name`, `width`, `height`, `color_mode` (the short name GIMP gives the base type: `rgb` was observed; `gray` and `indexed` are expected, unconfirmed) |
| `active_image` | `null` when no image is open; otherwise the first image `Gimp.get_images()` returns, with the keys below |
| `active_image.id`, `active_image.name` | As in `images` |
| `active_image.layers` | The top-level layers: `id`, `name`, `visible`, `opacity` (0 to 100), `blend_mode` (for example `normal`, `multiply`) |
| `active_image.active_layer` | The `id` of the first selected layer, or `null` |
| `active_image.selection` | `null` without selection, otherwise its bounding box: `x`, `y`, `width`, `height` |

Which image GIMP lists first, and therefore which one is called active, is only confirmed
with one open image; with several, it is unconfirmed. A non-empty selection was not
exercised: only `selection: null` was observed.
Layer groups are listed as one layer; their children are not.

Names of images and layers are document content: they are returned as they are and are
never interpreted, whatever they say.

When GIMP's API cannot be used (the file was run outside GIMP, with or without `gi`
installed), `state` is `{}`,
`app_version` is `unknown`, and `warnings` says so.

## Capture

The capture is a PNG of the active image. The bridge duplicates the image, merges the
visible layers of the copy, exports the copy with `Gimp.file_save` to the path the Hub
chose, then deletes the copy. Exporting the user's own image would change the file name
GIMP remembers for later exports; the copy leaves it untouched. Without an open image
there is no capture (`screenshot: null`). The Hub then checks the file before returning
its path.

## Example

A script that adds a transparent layer to the active image. It has not been run as
written (it needs an open image, and `Gimp.displays_flush()` was not exercised); a
similar script that created its own image and layer ran in GIMP 3.0.4 and returned
`{'layers': ['<name of the layer>']}`.

```python
image = images()[0]
layer = Gimp.Layer.new(
    image,
    "Notes",
    image.get_width(),
    image.get_height(),
    Gimp.ImageType.RGBA_IMAGE,
    100.0,
    Gimp.LayerMode.NORMAL,
)
image.insert_layer(layer, None, 0)
Gimp.displays_flush()
result = {"layer": layer.get_id(), "layers": [item.get_name() for item in image.get_layers()]}
```

## Limits

- The Hub's script guard does not see GIMP's own save and export functions: a script can
  overwrite a file through `Gimp.file_save`. The guard is a safety net against accidents,
  not a sandbox.
- A script runs with the user's rights, inside GIMP's Python.
- The bridge serves one connection at a time. A connection that arrives while another is
  served is closed at once, including a reconnection made a few milliseconds after the
  previous connection closed, before the bridge noticed.
