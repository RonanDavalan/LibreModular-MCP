# Blender module

**Status: tried in Blender 4.3.2 (Debian 13 package), in both ways of starting it.** The
bridge was written from the public Blender Python API (`bpy`). It ran in Blender 4.3.2
started without its interface (`blender --background --python <boot file>`, the boot file
running the bridge and then handing the main thread to it, see "Starting the bridge") and
with its interface on a virtual X display (`xvfb-run blender --python <bridge file>`),
driven by the Hub over MCP on stdio. In both, `get_state("hub")` listed `blender` as
connected with version `4.3.2`; `get_state("blender")` returned the default scene (`Cube`,
`Light`, `Camera`, frames 1 to 250, engine `BLENDER_EEVEE_NEXT`, `Cube` selected); the
example below returned `{"object": "Generated triangle", "objects": 4}`, after which the
object list had four entries; a script that raised returned its traceback with the right
line; a script that built a cube and rendered it at 128 x 128 with `bpy.ops.render.render`
returned `ok` and wrote the PNG (0.7 s without the interface, 27 s with it on a virtual
display, where EEVEE renders in software). A call that only reads or edits the scene took
1 ms without the interface and about 70 ms with it (the timer looks every 50 ms). Not part
of those runs: a bridge started from the *Scripting* workspace console of an interface the
operator opened, and a display-less session with the interface.

## What it does

The Hub reaches Blender in the `tcp` mode. A bridge runs inside Blender's Python, listens
on `127.0.0.1` only, and runs the scripts the Hub sends:

- `execute_script` runs a complete Python script inside Blender and returns what it
  printed, the value of its `result` variable, and Python's own traceback if it failed.
- `get_state` returns the active scene, its objects, the selected objects, the frame range
  and the render engine. It never returns a capture (see below).

The bridge is the generated file `bridges/generated/blender_bridge.py`: the common core
(`bridges/_core.py`) followed by the Blender adapter (`bridges/adapters/blender.py`),
assembled by `python3 scripts/build_bridges.py`. It uses the Python standard library only
and loads on Python 3.9 or later. It holds no token: it reads the token file at start.

The package installs the same file as `/usr/share/libremodular-mcp/bridges/blender_bridge.py`. The commands below name the bridge file of a clone, then the one of the package.

## Starting the bridge

The bridge never starts on its own: the operator starts it by hand, each time.

1. Create the token once, if it does not exist yet:

   ```sh
   libremodular-mcp init
   ```

2. Enable Blender in the Hub's configuration, `~/.config/libremodular-mcp/config.toml`:

   ```toml
   [apps.blender]
   enabled = true
   mode = "tcp"
   port = 9303
   ```

3. Start Blender with the bridge. The bridge reads two environment variables:

   | Variable | Default |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9303` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token`, or `~/.config/libremodular-mcp/token` when `XDG_CONFIG_HOME` is unset or relative |

   With the defaults of `init` and of the configuration above, nothing needs to be set.
   Either let Blender run the file as it starts, with its interface:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9303 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   blender --python /home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py
   ```

   From the installed package:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9303 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   blender --python /usr/share/libremodular-mcp/bridges/blender_bridge.py
   ```

   or, in a Blender that is already open, run it from the Python console of the
   *Scripting* workspace:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py", encoding="utf-8").read())
   ```

   From the installed package:

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/blender_bridge.py", encoding="utf-8").read())
   ```

   The variables can also be set in that console, before the line above, through
   `os.environ`: the bridge reads them when its file runs.

4. The bridge writes one line on Blender's error stream (the terminal Blender was started
   from) and, when the console redirects that stream, in the console:

   ```text
   LMM bridge listening on 127.0.0.1:9303
   ```

   The bridge refuses to start if the token file is a symbolic link, belongs to another
   user, is readable or writable by the group or others, is not a regular file, or does
   not hold a token made by `libremodular-mcp init`. To stop it, run `stop()` in the
   console where the file was run, or quit Blender.

Started without the interface (`blender --background --python ...`), Blender quits as soon
as the file has run, which ends the bridge with it. To keep it up, run the bridge file from
a boot file that then gives the main thread to the bridge by calling `serve_main_thread()`:

```python
import runpy

runpy.run_path("/home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py")[
    "serve_main_thread"
]()
```

From the installed package:

```python
import runpy

runpy.run_path("/usr/share/libremodular-mcp/bridges/blender_bridge.py")[
    "serve_main_thread"
]()
```

`serve_main_thread()` returns when the bridge stops (`stop()`), after `seconds` if you pass
`serve_main_thread(seconds=600)`, or on Ctrl+C; Blender then quits. In that mode no timer
runs, so this call is what runs the scripts, in the main thread, as Blender's own command
line does. A boot file that sleeps instead of calling it keeps the bridge up but runs
scripts in the bridge's network thread, where `bpy.ops.render.render` can hang (see
"Rendering").

To check the connection from the model's side, call `get_state` with the target `hub`:
Blender is listed with `connected: true` and the version Blender reported.

## What a script sees

Each script runs in a fresh namespace that holds:

| Name | What it is |
|---|---|
| `bpy` | Blender's Python module |
| `LMM_WORK_DIR` | The private directory the Hub prepared for this application's files |

- The script is compiled under the name `<libremodular>`: line numbers in a traceback are
  the lines of the script.
- Whatever the script prints is returned in `stdout` and `stderr`. Set `result` to a value
  that JSON can represent to return it; any other value returns `null` with a warning
  appended to `stderr`.
- Files created while the script runs are private (creation mask `077`).
- `bpy` must be used from Blender's main thread. When the bridge file runs in Blender's
  main thread with the interface, the adapter registers one persistent timer there
  (`bpy.app.timers.register`), which looks for queued scripts every 50 milliseconds and
  runs them; the connection thread waits for the outcome. In background mode, scripts run
  in the main thread while it is inside `serve_main_thread()` (see "Starting the bridge").
  Otherwise (the stand-alone `bpy` package, a bridge started from another thread, or a
  boot file that never calls `serve_main_thread()`), nothing would ever run a queued
  script, so scripts are called directly in the bridge's thread. Both paths ran in
  Blender 4.3.2 (see the status above).
- A script that never returns blocks Blender's main thread; the Hub answers `timeout`, then
  `busy` until the script ends, and never sends a second script meanwhile.
- Operators (`bpy.ops`) depend on the context they run in; from a timer there is no active
  area. `bpy.ops.mesh.primitive_cube_add` and `bpy.ops.render.render` ran in background
  mode (see "Rendering"); `bpy.ops.render.render` also ran with the interface on a virtual
  display. Any other operator is untried: the data API (`bpy.data`) does not depend on
  the context.

## Version

`app_version` is the first word of `bpy.app.version_string`, for example `4.3.2`; a long
term support release named `4.2.3 LTS` reads as `4.2.3`. Without `bpy` it is `unknown`.

## State

`get_state("blender")` returns, under `state`:

| Key | Content |
|---|---|
| `scene` | The name of the active scene (`bpy.context.scene`), or `null` without one |
| `objects` | Every object of the scene: `name`, `type` (for example `MESH`, `CAMERA`, `LIGHT`) |
| `selected_objects` | The names of the selected objects; an object outside the active view layer counts as not selected |
| `frame_range` | `start` and `end` frames of the scene, or `null` without a scene |
| `render_engine` | The render engine of the scene, for example `CYCLES`, or `null` without a scene |

Names of scenes and objects are document content: they are returned as they are and are
never interpreted, whatever they say.

When `bpy` cannot be imported (the file was run outside Blender), `state` is `{}`,
`app_version` is `unknown`, and `warnings` says so.

## No capture

`screenshot` is always `null` for Blender. Producing an image would mean starting a render
implicitly, which a request to read the state must never do, and there is no safe copy of
the 3D view without the interface. Rendering from a script is described below.

## Rendering

`bpy.ops.render.render(write_still=True)` works through the bridge when the main thread
runs the script: with the interface (the timer), and in `blender -b` when the boot file
calls `serve_main_thread()` (see "Starting the bridge"). In Blender 4.3.2 a 128 x 128
render of a cube in the default engine (`BLENDER_EEVEE_NEXT`) came back `ok` in 0.6 to
0.9 s every time it was tried that way (11 runs, with and without a display variable, and
under CPU load), and `get_state("blender")` answered afterwards. A boot file that sleeps
instead keeps the script in the bridge's network thread. There the same render came back in
0.6 to 0.9 s in 22 runs out of 23, and once never returned: the Hub answered `timeout`
after 30 seconds, then `busy`. Blender does not support `bpy.ops` outside its main thread,
so that thread can hang at any render; running the script in the main thread removes the
cause. Set `filepath` under `LMM_WORK_DIR`.

For a render driven by `bpy` and judged by pixel, in the Workbench engine (measured in
Blender 4.3.2, each point a render that came out wrong without an error):

- `scene.display.shading.color_type = "OBJECT"` reads `object.color`, white by default:
  everything renders white. Use `"MATERIAL"`.
- `material.diffuse_color` is linear. Writing the sRGB value of `#c4553a` renders
  `#E39C83`. Convert each channel from sRGB to linear before assigning.
- The background of a Workbench render stays the theme grey (`srgba(64,64,64,1)`) even
  with a world colour assigned; `background_type = "WORLD"` changes nothing, and
  `"CUSTOM"` fails in background mode. Render on a transparent film and compose the
  background afterwards (`magick render.png -background "#f4efe8" -alpha remove -alpha off`).
- A bevel set after `obj.scale` is overridden by the scale. Apply the scale to the mesh
  first (`bpy.ops.object.transform_apply(scale=True)`), then add the bevel.
- A transparent PNG reads as blank in a viewer that composes on white: judge the render
  on the composed image, and read the real colours with a histogram
  (`magick render.png +dither -colors 6 -format %c histogram:info:`).

## Example

A script that adds a triangle to the active scene with the data API. Run as written in
the default scene, it returns `{"object": "Generated triangle", "objects": 4}`.

```python
mesh = bpy.data.meshes.new("Generated mesh")
mesh.from_pydata([(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)], [], [(0, 1, 2)])
mesh.update()
item = bpy.data.objects.new("Generated triangle", mesh)
bpy.context.scene.collection.objects.link(item)
result = {"object": item.name, "objects": len(bpy.context.scene.objects)}
```

## Limits

- The Hub's script guard does not see Blender's own save and export functions: a script
  can overwrite a file through `bpy.ops.wm.save_as_mainfile`. The guard is a safety net
  against accidents, not a sandbox.
- A script runs with the user's rights, inside Blender's Python.
- The bridge serves one connection at a time.
