# Inkscape module

**Status: tried in Inkscape 1.4 (Debian 13 package), driven by the Hub over MCP on stdio.**
With `/usr/bin/python3` (3.13.5) and `/usr/share/inkscape/extensions` on `PYTHONPATH`, the
example below ran and returned `{"layers": ["Layer 1", "Notes"]}` in 141 ms; `get_state`
then listed both layers and a PNG capture (RGBA, mode 600); a script that raised returned
its traceback with the right line; a second script found the document as the first had
left it. One defect came out of that run and is fixed: `import inkex` imports numpy, and on
a machine with many cores its BLAS library started one thread per core, which never
finished starting under the 2 GiB address-space limit (the call ended in `timeout` after
30 s). The module now sets `OPENBLAS_NUM_THREADS=1` and `OMP_NUM_THREADS=1`.

## What it does

The Hub reaches Inkscape in the `cli` mode: there is no bridge and no long-running
process. Each `execute_script` starts one Python process that runs the script and ends;
each `get_state` starts one Python process that reads the working document, then runs
`inkscape` once to export a capture.

The working document is `current.svg` in the Hub's private work directory for Inkscape,
`<runtime>/libremodular-mcp/work/inkscape/`. The first script creates it, private (mode
600), as an empty SVG with one layer named `Layer 1`. Every script starts from that file
as the previous script left it: nothing else persists between scripts. If `current.svg`
is a symbolic link, belongs to another user or is open to the group or others, the Hub
refuses to use it and answers `unavailable`.

## Configuration

```toml
[apps.inkscape]
enabled = true
mode = "cli"
```

There is no port. The programs the module starts are fixed when the Hub starts, and
neither a script nor the configuration can change them:

| Program | Path |
|---|---|
| Python interpreter | `/usr/bin/python3` |
| Inkscape extensions, put on `PYTHONPATH` so that `import inkex` works | `/usr/share/inkscape/extensions` |
| Inkscape, for its version and for captures | `/usr/bin/inkscape` |

## What a script sees

The script is written to a private file of the work directory, never passed on the
command line, and removed once the process has ended. The command is an argument list
without a shell: `python3 -c <fixed bootstrap> <script file> <limits>`. The bootstrap
applies the limits below, then compiles the script under the name `<libremodular>`, so
line numbers in a traceback are the lines of the script.

The process starts in the work directory, in a process group of its own, with exactly
this environment and nothing else from the Hub (in particular never the token):

| Variable | Value |
|---|---|
| `PATH`, `HOME`, `LANG`, `LC_ALL` | The Hub's values, when the Hub has them |
| `PYTHONPATH` | `/usr/share/inkscape/extensions` |
| `LMM_DOCUMENT` | The path of `current.svg` |
| `LMM_WORK_DIR` | The work directory |
| `OPENBLAS_NUM_THREADS`, `OMP_NUM_THREADS` | `1`: numpy, which `inkex` imports, would otherwise start one thread per core |

Files the script creates follow the Hub's creation mask, `077`: they are private. A
script that replaces `current.svg` with a new file therefore leaves a private file the
next script can use; one that writes a file readable by others makes the Hub refuse it.

## Result and errors

- To return a value, a script prints `LMM_RESULT: ` followed by JSON as the **last line**
  of its standard output. The Hub takes that line out of `stdout` and returns the value
  as `result`. Without such a line, or when its JSON is invalid (the line then stays in
  `stdout`), `result` is `null`. If the output exceeds `max_output_bytes`, the end of the
  output, and the result line with it, is lost.
- When the script raises an exception (`SystemExit` included), the bootstrap writes
  `LMM_ERROR: ` followed by `{"type", "message", "traceback"}` as the last line of the
  standard error. The Hub takes that line out of `stderr` and returns it as `error`, with
  Python's own traceback.
- A process that ends with a non-zero code or a signal without that line (for example
  killed by a limit) gives `error.type` `ProcessError`, and a message with the exit code
  or the signal; there is no traceback.

## Limits

| Limit | Value |
|---|---|
| Wall-clock time | The Hub's `default_timeout_s` (30 s by default). When it runs out, every process of the group, children included, is killed with `SIGKILL`, and the answer is `timeout`. |
| `RLIMIT_CPU` | The same number of seconds |
| `RLIMIT_AS` (address space) | 2 GiB: a larger allocation fails with `MemoryError` or ends the process |
| `RLIMIT_NOFILE` | 256 open files |
| `RLIMIT_CORE` | 0: no core dump |
| Output | `max_output_bytes` per stream; the rest is read and dropped, so the process never blocks on a full pipe |

After a process ends, anything still running in its group is killed as well. A process
that leaves its group on purpose (with `setsid`, for example) is not followed. One script
runs at a time: a second request meanwhile answers `busy`.

## State

`get_state("inkscape")` reads `current.svg` in a Python process started with the same
environment and limits, and returns, under `state`:

| Key | Content |
|---|---|
| `width`, `height` | The attributes of the root element, as written (for example `210mm`), or `null` |
| `element_count` | The number of elements, root included |
| `layers` | Every `g` element whose `inkscape:groupmode` is `layer`, at any depth: `id`, `label` (`inkscape:label`) |

Before any script has run, `state` is `{}` and a warning says that the document does not
exist yet. A document that declares entities is refused: `state` is `{}` and a warning
says so. Nothing in the document is resolved or fetched, and a `DOCTYPE` without entity
declarations is read without fetching its DTD. The Hub itself parses no XML.

Layer names are document content: they are returned as they are and are never
interpreted, whatever they say. A layer made with `inkex.Layer.new` has no `id` until the
script sets one, and is listed with `id: null`.

## Capture

`inkscape --export-type=png --export-filename=<path chosen by the Hub> current.svg`, with
the same environment and its own process group, and 30 seconds to finish. The resource
limits of scripts are not applied to Inkscape itself. If Inkscape is missing or fails,
`screenshot` is `null` and a warning says why. The Hub then checks the file before
returning its path.

## Connection and version

`get_state("hub")` shows Inkscape as connected when `/usr/bin/python3` exists and
`inkscape --version` answers within 5 seconds; the version is the word after `Inkscape`
in that answer (for example `1.2.2`). The module keeps no process: releasing it does
nothing, and an Inkscape window the operator opened is never touched.

## Example

A script that adds a layer with a rectangle through `inkex`, saves the document and
returns the layer labels. Run as written on a fresh document, it returns
`{"layers": ["Layer 1", "Notes"]}`.

```python
import json
import os

import inkex

document = inkex.load_svg(os.environ["LMM_DOCUMENT"])
svg = document.getroot()
layer = svg.add(inkex.Layer.new("Notes"))
layer.add(inkex.Rectangle.new(10, 10, 80, 40))
document.write(os.environ["LMM_DOCUMENT"])
labels = [node.label for node in svg.descendants().filter(inkex.Layer)]
print("LMM_RESULT: " + json.dumps({"layers": labels}))
```

## Limits of the protection

- The Hub's script guard does not see `inkex`'s own ways to write or run things. The
  guard is a safety net against accidents, not a sandbox.
- A script runs with the user's rights.
