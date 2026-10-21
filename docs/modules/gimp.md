# GIMP module

**Status: tried in GIMP 3.0.4 started without its interface, not yet in the interactive
Python console.** The bridge was written from the public GIMP 3.0 API (GObject
Introspection: `gi.repository.Gimp`, `Gio`, `GLib`). It ran in GIMP 3.0.4 (Debian 13
package) started with `gimp-3.0 --new-instance --no-interface --batch-interpreter
python-fu-eval -b`, the batch command running the bridge file and then calling
`serve_main_thread()` (see "Starting the bridge"). In that run, driven by the Hub over
MCP, `get_state("hub")` listed `gimp` as connected with version `3.0.4`; a script created
an image and a layer, filled the layer with `layer.fill(Gimp.FillType.FOREGROUND)` and
exported the image into `LMM_WORK_DIR` (`status: ok`, a PNG of 128 by 128 pixels), a
second script did the same afterwards and the bridge was still connected;
`get_state("gimp")` returned the keys below and a PNG capture of 200 by 100 pixels in
mode 600; a script that failed returned its traceback with the right line of the script.
The same command with the interface on a virtual screen (`xvfb-run`) gave the same
results. Statements marked unconfirmed below were not part of those runs.

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

The package installs the same file as `/usr/share/libremodular-mcp/bridges/gimp_bridge.py`. The commands below name the bridge file of a clone, then the one of the package.

## Starting the bridge

The bridge never starts on its own. It is not placed in a plug-in or auto-load
directory: the operator starts it by hand, each time, in GIMP's Python console, or from
the batch command of a GIMP started without its interface (see "Started without the
interface").

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
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/gimp_bridge.py", encoding="utf-8").read())
   ```

   From the installed package:

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/gimp_bridge.py", encoding="utf-8").read())
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

### Started without the interface

With `--no-interface`, GIMP has no event loop and the batch command is the whole life of
the process that runs the bridge: when the command ends, that process, and the bridge
with it, ends (GIMP itself stays up). Run the bridge file in the batch command and give
the main thread to the bridge by calling `serve_main_thread()`, which the bridge file
defines:

```sh
LIBREMODULAR_BRIDGE_PORT=9301 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
gimp-3.0 --new-instance --no-interface --batch-interpreter python-fu-eval \
  -b "exec(open('/home/alice/LibreModular-MCP/bridges/generated/gimp_bridge.py', encoding='utf-8').read()); serve_main_thread()"
```

From the installed package:

```sh
LIBREMODULAR_BRIDGE_PORT=9301 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
gimp-3.0 --new-instance --no-interface --batch-interpreter python-fu-eval \
  -b "exec(open('/usr/share/libremodular-mcp/bridges/gimp_bridge.py', encoding='utf-8').read()); serve_main_thread()"
```

`--batch-interpreter python-fu-eval` is required: `python-eval` is not an interpreter of
GIMP 3.0.4, and `--batch-interpreter-args` does not exist. `serve_main_thread()` turns
GLib's main loop in the thread that called it, so that the scripts the bridge queues
run there. It returns when the bridge stops (`stop()`), after `seconds` if you pass
`serve_main_thread(seconds=600)`, or on Ctrl+C; the batch command then ends and so does
the bridge. A batch command that does not call it, or that ends its own loop, closes
port 9301 as soon as it returns. The runs above also passed `--no-data --no-fonts` and
used a profile directory of their own, so that the operator's profile was not touched.

Without its interface, GIMP 3.0.4 still needs an X display it can reach: with no display, the same command stopped
with `Gtk-WARNING: cannot open display` and exit code 1, and port 9301 never opened. On a machine without a screen,
give it a virtual X server.

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
  without the interface, with the main thread inside `serve_main_thread()`; in the
  interactive console it has not been tried.
  Outside GIMP, even on a machine where `gi` and the GIMP typelib are installed, the call
  is made directly in the bridge's own thread, because no main loop would ever run it.
- A script that never returns blocks GIMP's main thread; the Hub answers `timeout`, then
  `busy` until the script ends, and never sends a second script meanwhile.
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
installed), `state` is `{}`, `app_version` is `unknown`, and `warnings` says so.

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

## GIMP 3.0 API: renamed calls and silent failures

Observed in GIMP 3.0.4. A script written from memory of GIMP 2.10 fails with an
`AttributeError` or a `TypeError`; read `dir()` in the running GIMP rather than conclude
that the API is broken.

| To do | GIMP 2.10 | GIMP 3.0 |
|---|---|---|
| Create an image | `Gimp.Image.new(w, h, Gimp.ImageType.RGBA_IMAGE)` | `Gimp.Image.new(w, h, Gimp.ImageBaseType.RGB)`: the third argument is a base type |
| Make a colour | `Gimp.RGBA(r, g, b, a)` | `Gimp.color_parse_hex("#14181c")`, which returns a `Gegl.Color`; `Gimp.RGBA` and `Gimp.RGB` do not exist |
| Select a rectangle | `Gimp.image_select_rectangle(image, Gimp.ChannelOp.REPLACE, ...)` | `image.select_rectangle(Gimp.ChannelOps.REPLACE, ...)`: an instance method, and `ChannelOp` became `ChannelOps` |
| Fill with the foreground | `Gimp.edit_bucket_fill(image, layer, Gimp.BucketFillType.FG, ...)` | `layer.fill(Gimp.FillType.FOREGROUND)`; `BucketFillType` became `FillType` |
| Load a file | `Gimp.File.load(gfile)` | `Gimp.file_load(Gimp.RunMode.NONINTERACTIVE, gfile)` |
| Set the active layer | `Gimp.image_set_active_layer(image, layer)` | does not exist |

Four calls fail without raising an error:

- `image.insert_layer(layer, None, 0)` with a layer that belongs to another image does
  nothing: `image.get_layers()` stays empty. Copy it first with
  `Gimp.Layer.new_from_drawable(layer, image)`, then insert the copy.
- `layer.edit_bucket_fill(...)` left a layer of an `Gimp.ImageBaseType.RGB` image fully
  transparent (the flattened image read `srgba(0,0,0,0)`), whereas
  `layer.fill(Gimp.FillType.FOREGROUND)` filled it. The bridge does not guarantee an
  active layer, which the bucket fill seems to need.
- `layer.fill(Gimp.FillType.FOREGROUND)` ignores the selection and fills the whole layer.
  With an ellipse selected, a transparent layer filled this way read alpha 1.0 at its
  corner; `layer.edit_fill(Gimp.FillType.FOREGROUND)` kept to the selection (alpha 0.0 at
  the corner, 1.0 at the centre). To fill a selection, use `edit_fill`.
- `layer.scale(width, height, local_origin)` with a negative height (`-1`, "keep the
  ratio") does nothing; with two positive dimensions it resizes the layer.

## Limits

- The Hub's script guard does not see GIMP's own save and export functions: a script can
  overwrite a file through `Gimp.file_save`. The guard is a safety net against accidents,
  not a sandbox.
- A script runs with the user's rights, inside GIMP's Python.
- The bridge serves one connection at a time. A connection that arrives while another is
  served is closed at once, including a reconnection made a few milliseconds after the
  previous connection closed, before the bridge noticed.
