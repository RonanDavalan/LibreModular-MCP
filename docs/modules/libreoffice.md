# LibreOffice module

**Status: tried in LibreOffice 24.2.7.2 and 25.2.3.2.** The adapter and the launcher were
written from the public UNO API and from measurements made on LibreOffice 25.2.3.2; they then
ran against LibreOffice 24.2.7.2 (the Ubuntu 24.04 packages `libreoffice-core`, `-writer`,
`-calc`, `-impress` and `python3-uno`, with the system `/usr/bin/python3` 3.11.15) and against
LibreOffice 25.2.3.2 (the Debian 13 packages, with the system `/usr/bin/python3` 3.13.5), in two
ways:

- without interface, through `cargo test --test libreoffice_real -- --ignored --nocapture`,
  which starts its own LibreOffice on a throwaway profile and port, starts the generated
  bridge with the environment the launcher gives it, and drives the three targets through
  the Hub (passes on both versions);
- with a window on a virtual X display, started by the real `libremodular-launch`, driven by
  the real `libremodular-mcp` over MCP on stdio: `get_state("hub")` listed the three targets
  as connected in mode `uno` with the version of LibreOffice (`24.2.7.2`, then `25.2.3.2`); a
  script refused with `NoActiveDocument` while no document was open; after a Calc document was
  opened in the window, a script wrote two cells, `get_state` described the sheet and returned
  a PNG capture, a script for Writer was refused with `WrongDocumentType`, `--stop` ended the
  bridge and left LibreOffice running.

Not part of those runs: a real desktop session, a document with several sheets or pages (what
its capture shows), documents of other kinds open next to the three families, and a LibreOffice
that takes longer to start than the bridge waits for (see the version under *Limits*). The
adapter's unit tests use a fake `uno` and run the same whether or not LibreOffice is installed.

## What it does

The Hub reaches LibreOffice in the `uno` mode, through **three targets that share one
bridge and one connection**:

| Target | Family | Documents it serves |
|---|---|---|
| `libreoffice-writer` | `writer` | Text documents |
| `libreoffice-calc` | `calc` | Spreadsheets |
| `libreoffice-impress` | `impress` | Presentations |

The bridge is a stand-alone Python process, not a plug-in: it connects to the UNO port
LibreOffice listens on, for each request, and runs the script there. The Hub never speaks
UNO: it talks to the bridge on `bridge_port` like to any other bridge, and names the family
of the target in each request. A script running on one target makes a request on another
answer `busy`, since the bridge runs one script at a time.

- `execute_script` runs a complete Python script against the **active document** of the
  target's family and returns what it printed, the value of its `result` variable, and the
  trace of the error if it failed.
- `get_state` lists the open documents and describes the active one, and returns the path
  of a PNG capture of it.

The bridge is the generated file `bridges/generated/uno_bridge.py`: the common core
(`bridges/_core.py`) followed by the adapter (`bridges/adapters/uno.py`), assembled by
`python3 scripts/build_bridges.py`. It uses the Python standard library only, apart from
`uno`, which it imports when a request needs it; it loads on Python 3.9 or later, with or
without `uno`. It holds no token.

## The limit to know first

**LibreOffice's UNO port has no authentication.** Whoever can connect to it can drive
LibreOffice: open, change, save and close documents, and reach everything LibreOffice's
Python can reach, with the user's rights. It needs no token, passes through no script guard
and leaves nothing in the audit journal. Only the bridge is protected by the token, and only
against the Hub's side.

- LibreOffice is told to listen on `127.0.0.1` only, so nothing outside the machine reaches
  it. Every other process that can open a connection to that port on this machine can.
- Whether LibreOffice filters connections by user is unconfirmed: the runs above connected as
  the same user. Assume it does not.
- The port exists for as long as LibreOffice was started with `--accept`. Close that
  instance when you do not need it; the launcher never does it for you.
- The script guard of the Hub looks at the scripts sent through `execute_script` only. A
  program that talks to the UNO port directly is not seen by it.

## Starting

LibreOffice and the bridge are never started by the Hub, and never on the word of the model.
Both start on an explicit gesture of the operator.

Enable the targets in `~/.config/libremodular-mcp/config.toml`. The three targets are one
LibreOffice and one bridge: enabled targets must name the same ports, or the launcher
refuses. The values below are the defaults.

```toml
[apps.libreoffice-writer]
enabled = true
mode = "uno"
uno_port = 2002
bridge_port = 9304

[apps.libreoffice-calc]
enabled = true

[apps.libreoffice-impress]
enabled = true
```

`uno_port` is the port LibreOffice listens on and only the launcher uses it; `bridge_port`
is the port of the bridge, the one the Hub connects to.

### With the launcher

Create the token once, if it does not exist yet (`libremodular-mcp init`), then:

```sh
libremodular-launch libreoffice --bridge-file /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
```

It starts LibreOffice with its window, then the bridge, and returns. Output of a run:

```text
LibreOffice started (process 32212), listening for UNO on 127.0.0.1:2002
bridge started (process 32213), listening on 127.0.0.1:9304
open or create a document in LibreOffice: a script only runs on the active document
stop the bridge with 'libremodular-launch libreoffice --stop'; LibreOffice is yours to close
```

What the launcher does, and does not do:

- LibreOffice is started as `/usr/bin/soffice --accept=socket,host=127.0.0.1,port=2002;urp;
  --norestore --nologo --nodefault`, from a fixed argument list, without a shell and without
  `--headless`. It receives the environment of the launcher, except the variables whose name
  starts with `LIBREMODULAR_`.
- The bridge is started as `/usr/bin/python3 <the bridge file>`, with exactly these
  variables: `PATH`, `HOME`, `LANG` and `LC_ALL` as the launcher has them, `LD_LIBRARY_PATH`
  (`/usr/lib/libreoffice/program`) and `PYTHONPATH` (`/usr/lib/python3/dist-packages`) so
  that the system Python finds `uno`, `LIBREMODULAR_UNO_PORT`, `LIBREMODULAR_BRIDGE_PORT` and
  `LIBREMODULAR_TOKEN_PATH`. The token itself is in no argument and no variable: the bridge
  reads the file.
- Both processes are detached, in groups of their own, with their input and output sent
  nowhere. They go on when the launcher returns.
- The bridge file must be an absolute path to a regular file, not a link, owned by you and
  not writable by the group or others. A fresh clone made with a `umask` of `002` (the default
  of many Debian accounts) gives mode `664`, which the launcher refuses: run
  `chmod 600 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py` once. The directory
  that holds the file is not checked.
- **It refuses to start if a LibreOffice of yours is already running** (a `soffice.bin`
  process of your user, read from `/proc`). It never attaches to an instance that may hold
  unsaved work, and never closes one it did not start. Close LibreOffice and run it again,
  or start LibreOffice yourself as shown below.
- It refuses to start a second bridge while the bridge of an earlier launch still runs.
- The processes it started are recorded in `$XDG_RUNTIME_DIR/libremodular-mcp/launch/libreoffice.pids`
  (directory `700`, file `600`):

  ```text
  soffice 32212
  bridge 32213 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
  ```

  The first number is the process the launcher started; it is not the `soffice.bin` process
  the system lists (32240 in that run).

`libremodular-launch libreoffice --stop` ends the bridge, **only after checking that the
recorded process still is the bridge** (one of its arguments is exactly the bridge file): a
number another program has taken since is never signalled. It does not close LibreOffice.
Closing an instance that may hold work is your gesture, and the message says so.

Exit codes: `0` done; `2` a wrong usage, or a refusal you can fix (no enabled target, ports
that disagree or are equal, a bridge file that cannot be trusted, a LibreOffice or a bridge
already running); `1` a failure (an unreadable configuration, no `XDG_RUNTIME_DIR`, a program
that did not start). The other applications are started by hand and answer
`start the bridge by hand, see docs/modules/<name>.md` with code `2`.

If the bridge cannot be started after LibreOffice was, the launcher says so, records
LibreOffice alone and leaves it running.

### By hand

Start LibreOffice with the `--accept` option. The host is the number `127.0.0.1`, never a
name, which may resolve to an address LibreOffice does not listen on:

```sh
soffice --accept="socket,host=127.0.0.1,port=2002;urp;" --norestore --nologo --nodefault
```

Then, once LibreOffice is up, start the bridge with the system Python, which has `uno`:

```sh
LD_LIBRARY_PATH=/usr/lib/libreoffice/program \
PYTHONPATH=/usr/lib/python3/dist-packages \
LIBREMODULAR_UNO_PORT=2002 \
LIBREMODULAR_BRIDGE_PORT=9304 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
/usr/bin/python3 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
```

The bridge writes one line on its error stream, `LMM bridge listening on 127.0.0.1:9304`,
and runs until the process is stopped. It refuses to start if the token file is a link,
belongs to another user, is open to the group or others, or is not a token made by
`libremodular-mcp init`.

### The first document

**A script only runs on an active document of its target's family, and the Hub cannot create
one:** with no document open, every `execute_script` answers `NoActiveDocument`. Open or
create the document yourself in LibreOffice's window. In a run without interface, starting
`soffice` with `--calc` opened no document (measured); with a window it is unconfirmed.

To check the connection from the model's side, call `get_state` with the target `hub`: the
three targets are listed with `connected: true` and the version LibreOffice reported.

## What a script sees

A script runs in the bridge, which is a separate process, in a fresh namespace that holds:

| Name | What it is |
|---|---|
| `uno` | The `uno` module |
| `ctx` | The remote component context of LibreOffice |
| `desktop` | `com.sun.star.frame.Desktop`, the application |
| `doc` | The active document, of the family of the target |
| `LMM_WORK_DIR` | The private directory the Hub prepared for this application's files |

- The script is compiled under the name `<libremodular>`: line numbers in a trace are the
  lines of the script.
- Whatever the script prints is returned in `stdout` and `stderr`. Set `result` to a value
  that JSON can represent to return it; any other value returns `null` with a warning
  appended to `stderr`. UNO objects are not JSON: return plain values.
- The connection to LibreOffice is made for each request, and tried again for up to five
  seconds while LibreOffice has not finished starting.
- The family is checked before anything of the script runs. The document is never
  converted: a Calc document is not offered to a Writer script.
- An exception raised by UNO is reported with its full UNO name, for example
  `com.sun.star.container.NoSuchElementException`.
- Whatever the script does through UNO (save, close, export) is done with your rights; the
  Hub's script guard does not see the save and export functions of the application.

## State

`get_state("libreoffice-calc")`, and the same for the two other targets, returns under
`state`:

| Key | Content |
|---|---|
| `documents` | Every open document of the three families, in the order LibreOffice lists them: `title` and `family` (`writer`, `calc` or `impress`). Drawings and other components are not listed |
| `active` | `null`, or the description of the active document, below |

`active` is `null`, with a warning, when LibreOffice has no current document or its current
document is not of the target's family. That is not an error. The bridge cannot tell the two
cases apart, so the warning names both.

| Family | Content of `active` |
|---|---|
| `writer` | `paragraphs`: the number of paragraphs (tables are listed next to them and are not counted); `pages`: the page count, or `null` for a document without a controller |
| `calc` | `sheets`: a list of `name`, `last_column` and `last_row`, the zero-based indices of the last cell used, as UNO gives them. An empty sheet reports `0` and `0`, like a sheet whose only content is `A1` |
| `impress` | `slides`: the number of slides |

Titles and sheet names are document content: they are returned as they are and are never
interpreted, whatever they say.

When `uno` cannot be imported by the interpreter that runs the bridge, `state` is `{}`,
`app_version` is `unknown`, and `warnings` says so. The same when LibreOffice does not accept
the connection, with the reason in the warning.

## Capture

The capture is an export of the active document of the family, made by LibreOffice with its
PNG filter (`writer_png_Export`, `calc_png_Export` or `impress_png_Export`) to the path the
Hub chose. The document is not saved, closed or changed: after a capture `hasLocation()` is
still `False` and the document is still modified. Nothing in the bridge calls `store`,
`storeSelf`, `storeAsURL`, `close`, `dispose` or `terminate`, and the tests fail if it does.

- The file is written by LibreOffice's own process, with that process's creation mask. The
  Hub then checks it, and makes it private (mode `600`) in its own private directory.
- Without an active document of the family there is no capture (`screenshot: null`).
- What the PNG of a document with several sheets or pages shows is unconfirmed.

## Errors

| `error.type` | When |
|---|---|
| `LibreOfficeUnavailable` | `uno` cannot be imported by the bridge's interpreter, or LibreOffice does not accept a connection within the wait. The message says which |
| `NoActiveDocument` | LibreOffice has no current document. Nothing of the script ran |
| `WrongDocumentType` | The current document is not of the target's family. Nothing of the script ran |
| `UnknownFamily` | The request named no family the bridge serves. The Hub always names one, so this only means the Hub and the bridge file do not match |

The first three are raised by the bridge itself, so their trace shows the bridge's own frames
and the path of the bridge file, not lines of the script. The message is the part to read.

## Example

A Calc script that writes two cells and returns what it wrote. It ran as written on a new
spreadsheet; the answer below is what the Hub returned in that run, with fields in the
order the Hub writes them.

```python
sheet = doc.Sheets.getByIndex(0)
sheet.getCellByPosition(0, 0).setValue(42)
sheet.getCellByPosition(1, 1).setString('hello')
print('wrote A1 and B2')
result = {'sheet': sheet.Name, 'a1': sheet.getCellByPosition(0, 0).getValue(), 'b2': sheet.getCellByPosition(1, 1).getString()}
```

```json
{
  "app": "libreoffice-calc",
  "duration_ms": 53,
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

The `get_state` that followed returned, under `state`:

```json
{
  "active": {"sheets": [{"last_column": 1, "last_row": 1, "name": "Sheet1"}]},
  "documents": [{"family": "calc", "title": "Untitled 1"}]
}
```

## Limits

- **The version is read once, when the bridge starts.** If LibreOffice does not listen yet,
  the bridge waits for it for up to five seconds, then reports `unknown` in its `hello`, and
  every `get_state` then carries a warning that the API cannot be imported, although
  requests work. LibreOffice answered on UNO 1.3 seconds after it started in the test run;
  on its very first start in the environment where this was written (cold caches) it had not
  answered after 8 seconds and had by 14. If `get_state("hub")` shows `unknown`, stop the
  bridge with `--stop` and start it by hand, as shown above, once LibreOffice is up: the
  launcher itself refuses to start next to a running LibreOffice.
- Only the **current** document is reachable. With several documents of a family open, a
  script gets whichever LibreOffice reports as current; the other ones are listed in
  `documents` and are not offered. Which document is current when the focus is on another
  window (a dialog, the Basic IDE) is unconfirmed.
- Writer, Calc and Impress only. A Draw, Math or Base document is not a document of a family:
  it is not listed, and a script for any target is refused with `WrongDocumentType` while it
  is the current one.
- Describing a very large Writer document counts its paragraphs one by one through UNO; the
  time is bounded only by the Hub's timeout.
- The Hub's script guard is an heuristic against accidents, not a sandbox. It does not see
  the save, export and close functions of LibreOffice, a path passed as a function
  parameter, or anything a program does by talking to the UNO port itself.
- The bridge serves one connection at a time, and one script at a time for the three
  targets together.
- The named-pipe variant of the UNO connection is not supported.
