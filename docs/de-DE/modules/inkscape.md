# Inkscape-Modul

**Status: ausprobiert in Inkscape 1.4 (Paket von Debian 13), gesteuert vom Hub über MCP auf Standardeingabe und
Standardausgabe.** Mit `/usr/bin/python3` (3.13.5) und `/usr/share/inkscape/extensions` auf `PYTHONPATH` lief das
Beispiel unten und lieferte in 141 ms `{"layers": ["Layer 1", "Notes"]}` zurück; `get_state` führte danach beide Ebenen
und eine PNG-Aufnahme (RGBA, Modus 600) auf. Ein Skript, das eine Ausnahme auslöste, lieferte seine Fehlerspur mit der
richtigen Zeile zurück, und ein zweites Skript fand das Dokument so vor, wie das erste es hinterlassen hatte. Aus
diesem Lauf ging ein Fehler hervor, der behoben ist: `import inkex` importiert numpy, und auf einer Maschine mit vielen
Kernen startete dessen BLAS-Bibliothek einen Thread je Kern, der unter der Adressraumgrenze von 2 GiB nie zu Ende
gestartet wurde (der Aufruf endete nach 30 s mit `timeout`). Das Modul setzt jetzt `OPENBLAS_NUM_THREADS=1` und
`OMP_NUM_THREADS=1`.

## Was es tut

Der Hub erreicht Inkscape im Modus `cli`: Es gibt keine Brücke und keinen dauerhaft laufenden Prozess. Jedes
`execute_script` startet einen Python-Prozess, der das Skript ausführt und endet; jedes `get_state` startet einen
Python-Prozess, der das Arbeitsdokument liest und danach einmal `inkscape` ausführt, um eine Aufnahme zu exportieren.

Das Arbeitsdokument ist `current.svg` im privaten Arbeitsverzeichnis des Hubs für Inkscape,
`<runtime>/libremodular-mcp/work/inkscape/`. Das erste Skript legt es an, privat (Modus 600), als leere SVG-Datei mit
einer Ebene namens `Layer 1`. Jedes Skript beginnt mit dieser Datei in dem Zustand, in dem das vorige Skript sie
hinterlassen hat: Sonst bleibt zwischen Skripten nichts erhalten. Ist `current.svg` ein symbolischer Link, gehört es
einem anderen Benutzer oder ist es für die Gruppe oder andere zugänglich, weigert sich der Hub, es zu verwenden, und
antwortet `unavailable`.

## Konfiguration

```toml
[apps.inkscape]
enabled = true
mode = "cli"
```

Es gibt keinen Port. Die Programme, die das Modul startet, stehen beim Start des Hubs fest, und weder ein Skript noch
die Konfiguration kann sie ändern:

| Programm | Pfad |
|---|---|
| Python-Interpreter | `/usr/bin/python3` |
| Erweiterungen von Inkscape, auf `PYTHONPATH` gelegt, damit `import inkex` funktioniert | `/usr/share/inkscape/extensions` |
| Inkscape, für seine Version und für Aufnahmen | `/usr/bin/inkscape` |

Auf einem frischen Debian 13 zieht das Paket `inkscape` `python3-tinycss2` nicht nach, das `import inkex` braucht: Installieren Sie beide mit `sudo apt install inkscape python3-tinycss2`. Ohne es endet ein Skript, das `inkex` importiert, mit `ModuleNotFoundError: No module named 'tinycss2'`.

## Was ein Skript sieht

Das Skript wird in eine private Datei des Arbeitsverzeichnisses geschrieben, nie auf der Befehlszeile übergeben und
entfernt, sobald der Prozess beendet ist. Der Befehl ist eine Argumentliste ohne Shell:
`python3 -c <fixed bootstrap> <script file> <limits>`. Der Bootstrap wendet die unten genannten Grenzwerte an und
kompiliert das Skript dann unter dem Namen `<libremodular>`; die Zeilennummern in einer Fehlerspur sind deshalb die
Zeilen des Skripts.

Der Prozess startet im Arbeitsverzeichnis, in einer eigenen Prozessgruppe, und zwar mit genau dieser Umgebung und
sonst nichts vom Hub (insbesondere nie mit dem Token):

| Variable | Wert |
|---|---|
| `PATH`, `HOME`, `LANG`, `LC_ALL` | Die Werte des Hubs, sofern der Hub sie hat |
| `PYTHONPATH` | `/usr/share/inkscape/extensions` |
| `LMM_DOCUMENT` | Der Pfad von `current.svg` |
| `LMM_WORK_DIR` | Das Arbeitsverzeichnis |
| `OPENBLAS_NUM_THREADS`, `OMP_NUM_THREADS` | `1`: numpy, das `inkex` importiert, würde sonst einen Thread je Kern starten |

Dateien, die das Skript anlegt, folgen der Erstellungsmaske des Hubs, `077`: Sie sind privat. Ein Skript, das
`current.svg` durch eine neue Datei ersetzt, hinterlässt deshalb eine private Datei, die das nächste Skript verwenden
kann; schreibt eines eine Datei, die andere lesen können, lehnt der Hub sie ab.

## Ergebnis und Fehler

- Um einen Wert zurückzugeben, gibt ein Skript `LMM_RESULT: ` gefolgt von JSON als **letzte Zeile** seiner
  Standardausgabe aus. Der Hub entnimmt diese Zeile aus `stdout` und liefert den Wert als `result` zurück. Fehlt
  diese Zeile oder ist ihr JSON ungültig (die Zeile bleibt dann in `stdout`), ist `result` `null`. Überschreitet die
  Ausgabe `max_output_bytes`, gehen das Ende der Ausgabe und mit ihm die Ergebniszeile verloren, und die Antwort trägt `"truncated": true`.
- Löst das Skript eine Ausnahme aus (auch `SystemExit`), schreibt der Bootstrap `LMM_ERROR: ` gefolgt von
  `{"type", "message", "traceback"}` als letzte Zeile der Standardfehlerausgabe. Der Hub entnimmt diese Zeile aus
  `stderr` und liefert sie als `error` zurück, mit der eigenen Fehlerspur von Python.
- Ein Prozess, der ohne diese Zeile mit einem Exit-Code ungleich null oder durch ein Signal endet (z. B. weil ihn ein
  Grenzwert beendet hat), ergibt für `error.type` den Wert `ProcessError` und eine Meldung mit dem Exit-Code oder dem
  Signal; eine Fehlerspur gibt es nicht.

## Grenzwerte

| Grenzwert | Wert |
|---|---|
| Tatsächliche Laufzeit | `default_timeout_s` des Hubs (standardmäßig 30 s). Läuft sie ab, wird jeder Prozess der Gruppe, Kindprozesse eingeschlossen, mit `SIGKILL` beendet, und die Antwort lautet `timeout`. |
| `RLIMIT_CPU` | Dieselbe Anzahl Sekunden |
| `RLIMIT_AS` (Adressraum) | 2 GiB: Eine größere Speicheranforderung schlägt mit `MemoryError` fehl oder beendet den Prozess |
| `RLIMIT_NOFILE` | 256 offene Dateien |
| `RLIMIT_CORE` | 0: kein Core-Dump |
| Ausgabe | `max_output_bytes` je Datenstrom; der Rest wird gelesen und verworfen, sodass der Prozess nie an einer vollen Pipe blockiert |

Wenn ein Prozess endet, wird auch alles beendet, was in seiner Gruppe noch läuft, und der Hub antwortet erst, wenn die Gruppe verschwunden ist (höchstens zwei Sekunden). Ein Prozess, der seine Gruppe mit
Absicht verlässt (z. B. mit `setsid`), wird nicht weiterverfolgt. Es läuft jeweils nur ein Skript: Eine zweite Anfrage
in der Zwischenzeit erhält die Antwort `busy`.

## Zustand

`get_state("inkscape")` liest `current.svg` in einem Python-Prozess, der mit derselben Umgebung und denselben Grenzwerten
gestartet wird, und liefert unter `state`:

| Schlüssel | Inhalt |
|---|---|
| `width`, `height` | Die Attribute des Wurzelelements, wie geschrieben (z. B. `210mm`), oder `null` |
| `element_count` | Die Anzahl der Elemente, das Wurzelelement eingeschlossen |
| `layers` | Jedes `g`-Element, dessen `inkscape:groupmode` den Wert `layer` hat, in beliebiger Tiefe: `id`, `label` (`inkscape:label`) |

Solange `current.svg` nicht existiert, ist `state` gleich `{}`, und eine Warnung sagt, dass das Dokument noch nicht
existiert; die Abfrage des Zustands legt es nie an. Ein vorhandenes `current.svg` wird gelesen, gleichgültig, ob
seit dem Start des Hubs ein Skript gelaufen ist; so wird auch ein Dokument beschrieben, das ein früherer Lauf des Hubs
hinterlassen hat. Ein Dokument, das Entitäten deklariert, wird abgelehnt: `state` ist dann `{}`, und eine Warnung sagt
das. Nichts im Dokument wird aufgelöst oder abgerufen, und ein `DOCTYPE` ohne Entitätsdeklarationen wird gelesen, ohne
seine DTD abzurufen. Der Hub selbst parst kein XML.

Namen von Ebenen sind Dokumentinhalt: Sie werden unverändert zurückgegeben und nie interpretiert, gleich, was sie sagen.
Eine Ebene, die mit `inkex.Layer.new` erzeugt wurde, hat keine `id`, bis das Skript eine setzt, und wird mit `id: null`
aufgeführt.

## Aufnahme

`inkscape --export-type=png --export-filename=<path chosen by the Hub> current.svg`, mit derselben Umgebung und einer
eigenen Prozessgruppe und mit 30 Sekunden, um fertig zu werden. Die Ressourcengrenzen der Skripte gelten nicht für
Inkscape selbst. Fehlt Inkscape oder schlägt es fehl, ist `screenshot` gleich `null`, und eine Warnung nennt den
Grund. Der Hub prüft die Datei dann, bevor er ihren Pfad zurückgibt.

## Verbindung und Version

`get_state("hub")` zeigt Inkscape als verbunden, wenn `/usr/bin/python3` existiert und `inkscape --version` innerhalb
von 5 Sekunden antwortet; die Version ist das Wort nach `Inkscape` in dieser Antwort (zum Beispiel `1.2.2`). Das Modul
behält keinen Prozess: Es freizugeben bewirkt nichts, und ein Inkscape-Fenster, das der Bediener geöffnet hat, wird nie
angefasst.

## Beispiel

Ein Skript, das über `inkex` eine Ebene mit einem Rechteck hinzufügt, das Dokument speichert und die Bezeichnungen der
Ebenen zurückgibt. Unverändert auf einem neuen Dokument ausgeführt, liefert es `{"layers": ["Layer 1", "Notes"]}` zurück.

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

## `inkex` 1.4: Fallen, die keinen Fehler auslösen

Beobachtet in Inkscape 1.4.

- `inkex.Circle.new(a, b)` und `inkex.Ellipse.new(a, b)` nehmen zwei Argumente, und das zweite ist nicht der
  Mittelpunkt: `inkex.Circle.new(64, 64)` ergibt `cx="64.0" cy="0.0" r="64.0"`, einen Kreis, der am oberen Rand des
  Dokuments klebt. Ein drittes Argument löst einen `TypeError` aus. Erzeugen Sie das Element und setzen Sie `cx` und
  `cy` danach ausdrücklich.
- `inkex.Path` hat keine Segmentmethoden (`M`, `L`, `C`, `Z`, `moveTo`, `lineTo`). Setzen Sie stattdessen das Attribut
  `d` eines `path`-Elements, zum Beispiel mit `lxml.etree.SubElement` und `.set("d", ...)`.
- Das Dokument bleibt zwischen zwei Skripten erhalten: Was ein Skript hinzugefügt hat, ist für das nächste noch da. Ein
  Skript, das die ganze Zeichnung neu aufbaut, entfernt zuerst die untergeordneten Elemente der Wurzel
  (`for child in list(svg): svg.remove(child)`).

## Grenzen des Schutzes

- Die Skriptsperre des Hubs sieht die eigenen Wege von `inkex` nicht, etwas zu schreiben oder auszuführen. Die Sperre
  ist ein Sicherheitsnetz gegen Versehen, keine Sandbox.
- Ein Skript läuft mit den Rechten des Benutzers.
