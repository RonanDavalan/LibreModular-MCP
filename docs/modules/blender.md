# Blender module

**Status: tried in Blender 4.3.2 (Debian 13 package), in both ways of starting it.** The
bridge was written from the public Blender Python API (`bpy`). It ran in Blender 4.3.2
started without its interface (`blender --background --python <boot file>`, the boot file
running the bridge and then sleeping so that Blender stayed up) and with its interface on a
virtual X display (`xvfb-run blender --python <bridge file>`), driven by the Hub over MCP on
stdio. In both, `get_state("hub")` listed `blender` as connected with version `4.3.2`;
`get_state("blender")` returned the default scene (`Cube`, `Light`, `Camera`, frames 1 to
250, engine `BLENDER_EEVEE_NEXT`, `Cube` selected); the example below returned
`{"object": "Generated triangle", "objects": 4}`, after which the object list had four
entries; a script that raised returned its traceback with the right line. A call took 1 ms
without the interface and about 70 ms with it (the timer looks every 50 ms). Not part of
those runs: a bridge started from the *Scripting* workspace console of an interface the
operator opened, and `bpy.ops` operators.

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

   or, in a Blender that is already open, run it from the Python console of the
   *Scripting* workspace:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py").read())
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
a boot file that then keeps the main thread busy (for example `time.sleep(100)`): the bridge
serves meanwhile. In that mode no timer runs, so the bridge calls `bpy` directly from its
own thread.

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
  runs them; the connection thread waits for the outcome. Outside Blender's interface (the
  stand-alone `bpy` package, background mode, or a bridge started from another thread), no
  timer would ever run, so scripts are called directly. Both paths ran in Blender 4.3.2
  (see the status above).
- A script that never returns blocks Blender's main thread; the Hub answers `timeout`, then
  `busy` until the script ends, and never sends a second script meanwhile.
- Operators (`bpy.ops`) depend on the context they run in; from a timer there is no active
  area. Whether a given operator works from the bridge is unconfirmed: the data API
  (`bpy.data`) does not depend on it.

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
the 3D view without the interface. Rendering from a script has its own limit, below.

## Rendering

In `blender -b` (background mode) the bridge runs a script in its network thread, not in
Blender's main thread. `bpy.ops.render.render` called that way never returns: in Blender
4.3.2 the Hub answered `timeout` after 30 seconds and the bridge connection was closed.
Reading and changing the scene by the data API works. Render with Blender's own command
line (`blender -b file.blend -o <path> -f 1`, or `--python` with a script). Rendering
through the bridge with the interface open has not been tried.

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
