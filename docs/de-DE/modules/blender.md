# Blender-Modul

**Status: ausprobiert in Blender 4.3.2 (Paket von Debian 13), in beiden Startarten.** Die Brücke wurde nach der
öffentlichen Python-Schnittstelle von Blender (`bpy`) geschrieben. Sie lief in Blender 4.3.2, gestartet ohne
Oberfläche (`blender --background --python <boot file>`, wobei die Startdatei die Brücke ausführt und ihr dann den
Hauptthread übergibt, siehe „Die Brücke starten“) und mit Oberfläche auf einem virtuellen X-Display
(`xvfb-run blender --python <bridge file>`), gesteuert vom Hub über MCP auf Standardeingabe und Standardausgabe. In
beiden Fällen führte `get_state("hub")` `blender` als verbunden auf, mit der Version `4.3.2`; `get_state("blender")`
lieferte die Standardszene (`Cube`, `Light`, `Camera`, Einzelbilder 1 bis 250, Render-Engine `BLENDER_EEVEE_NEXT`,
`Cube` ausgewählt). Das Beispiel unten lieferte `{"object": "Generated triangle", "objects": 4}` zurück, danach hatte
die Objektliste vier Einträge. Ein Skript, das eine Ausnahme auslöste, lieferte seine Fehlerspur mit der richtigen
Zeile zurück. Ein Skript, das einen Würfel baute und ihn mit `bpy.ops.render.render` in 128 mal 128 Pixeln rendern
ließ, lieferte `ok` zurück und schrieb die PNG-Datei (0,7 s ohne Oberfläche, 27 s mit Oberfläche auf einem virtuellen
Display, wo EEVEE in Software rendert). Ein Aufruf, der die Szene nur liest oder bearbeitet, dauerte ohne Oberfläche
1 ms und mit Oberfläche etwa 70 ms (der Timer schaut alle 50 ms nach). Nicht Teil dieser Läufe: eine Brücke, die aus der
Konsole des Arbeitsbereichs *Scripting* einer vom Bediener geöffneten Oberfläche gestartet wurde, und eine Sitzung mit
Oberfläche ohne Display.

## Was es tut

Der Hub erreicht Blender im Modus `tcp`. Eine Brücke läuft in Blenders Python, lauscht nur auf `127.0.0.1` und führt die
Skripte aus, die der Hub ihr sendet:

- `execute_script` führt ein vollständiges Python-Skript in Blender aus und liefert zurück, was es ausgegeben hat, den
  Wert seiner Variablen `result` und, wenn es fehlschlug, die eigene Fehlerspur von Python.
- `get_state` liefert die aktive Szene, ihre Objekte, die ausgewählten Objekte, den Bereich der Einzelbilder und die
  Render-Engine. Eine Aufnahme liefert es nie (siehe unten).

Die Brücke ist die erzeugte Datei `bridges/generated/blender_bridge.py`: der gemeinsame Kern (`bridges/_core.py`),
gefolgt vom Blender-Adapter (`bridges/adapters/blender.py`), zusammengesetzt von `python3 scripts/build_bridges.py`. Sie
nutzt nur die Standardbibliothek von Python und lässt sich ab Python 3.9 laden. Sie hält kein Token, sondern liest die
Token-Datei beim Start.

Das Paket installiert dieselbe Datei als `/usr/share/libremodular-mcp/bridges/blender_bridge.py`. Die folgenden Befehle nennen die Brückendatei eines Klons und danach die des Pakets.

## Die Brücke starten

Die Brücke startet nie von selbst: Der Bediener startet sie jedes Mal von Hand.

1. Legen Sie das Token einmalig an, falls es noch nicht existiert:

   ```sh
   libremodular-mcp init
   ```

2. Aktivieren Sie Blender in der Konfiguration des Hubs, `~/.config/libremodular-mcp/config.toml`:

   ```toml
   [apps.blender]
   enabled = true
   mode = "tcp"
   port = 9303
   ```

3. Starten Sie Blender mit der Brücke. Die Brücke liest zwei Umgebungsvariablen:

   | Variable | Standardwert |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9303` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token` oder `~/.config/libremodular-mcp/token`, wenn `XDG_CONFIG_HOME` nicht gesetzt oder relativ ist |

   Mit den Standardwerten von `init` und der Konfiguration oben muss nichts gesetzt werden. Lassen Sie Blender die Datei
   entweder beim Start ausführen, mit Oberfläche:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9303 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   blender --python /home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py
   ```

   Mit dem installierten Paket:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9303 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   blender --python /usr/share/libremodular-mcp/bridges/blender_bridge.py
   ```

   oder führen Sie sie in einem bereits geöffneten Blender aus der Python-Konsole des Arbeitsbereichs *Scripting* aus:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py", encoding="utf-8").read())
   ```

   Mit dem installierten Paket:

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/blender_bridge.py", encoding="utf-8").read())
   ```

   Die Variablen lassen sich auch in dieser Konsole über `os.environ` setzen, vor der Zeile oben: Die Brücke liest sie,
   wenn ihre Datei ausgeführt wird.

4. Die Brücke schreibt eine Zeile in den Fehlerstrom von Blender (das Terminal, aus dem Blender gestartet wurde) und,
   wenn die Konsole diesen Strom umleitet, in die Konsole:

   ```text
   LMM bridge listening on 127.0.0.1:9303
   ```

   Die Brücke weigert sich zu starten, wenn die Token-Datei ein symbolischer Link ist, einem anderen Benutzer gehört,
   für die Gruppe oder andere lesbar oder beschreibbar ist, keine reguläre Datei ist oder kein Token enthält, das
   `libremodular-mcp init` angelegt hat. Um sie zu beenden, führen Sie `stop()` in der Konsole aus, in der die Datei
   ausgeführt wurde, oder beenden Sie Blender.

Wird Blender ohne Oberfläche gestartet (`blender --background --python ...`), beendet es sich, sobald die Datei
ausgeführt wurde, und die Brücke endet damit. Damit sie weiterläuft, führen Sie die Brückendatei aus einer Startdatei
aus, die den Hauptthread anschließend an die Brücke übergibt, indem sie `serve_main_thread()` aufruft:

```python
import runpy

runpy.run_path("/home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py")[
    "serve_main_thread"
]()
```

Mit dem installierten Paket:

```python
import runpy

runpy.run_path("/usr/share/libremodular-mcp/bridges/blender_bridge.py")[
    "serve_main_thread"
]()
```

`serve_main_thread()` kehrt zurück, wenn die Brücke endet (`stop()`), nach `seconds`, wenn Sie
`serve_main_thread(seconds=600)` übergeben, oder bei Strg+C; Blender beendet sich dann. In diesem Modus läuft kein
Timer, deshalb führt dieser Aufruf die Skripte im Hauptthread aus, so wie es die Befehlszeile von Blender selbst tut.
Eine Startdatei, die stattdessen nur wartet, hält die Brücke am Laufen, führt die Skripte aber im Netzwerk-Thread der
Brücke aus, wo `bpy.ops.render.render` hängen bleiben kann (siehe „Rendern“).

Um die Verbindung aus Sicht des Modells zu prüfen, rufen Sie `get_state` mit dem Ziel `hub` auf: Blender steht dann mit
`connected: true` und der Version in der Liste, die Blender gemeldet hat.

## Was ein Skript sieht

Jedes Skript läuft in einem frischen Namensraum, der Folgendes enthält:

| Name | Was es ist |
|---|---|
| `bpy` | Das Python-Modul von Blender |
| `LMM_WORK_DIR` | Das private Verzeichnis, das der Hub für die Dateien dieser Anwendung vorbereitet hat |

- Das Skript wird unter dem Namen `<libremodular>` kompiliert: Die Zeilennummern in einer Fehlerspur sind die Zeilen
  des Skripts.
- Was das Skript ausgibt, kommt in `stdout` und `stderr` zurück. Setzen Sie `result` auf einen Wert, den JSON
  darstellen kann, um ihn zurückzugeben; jeder andere Wert liefert `null`, und an `stderr` wird eine Warnung
  angehängt.
- Dateien, die das Skript während der Ausführung anlegt, sind privat (Erstellungsmaske `077`).
- `bpy` muss im Hauptthread von Blender verwendet werden. Läuft die Brückendatei im Hauptthread von Blender mit
  Oberfläche, registriert der Adapter dort einen dauerhaften Timer (`bpy.app.timers.register`), der alle 50
  Millisekunden nach Skripten in der Warteschlange sucht und sie ausführt; der Verbindungs-Thread wartet auf das
  Ergebnis. Im Hintergrundmodus laufen die Skripte im Hauptthread, solange dieser in `serve_main_thread()` steckt
  (siehe „Die Brücke starten“). In den übrigen Fällen (das eigenständige Paket `bpy`, eine aus einem anderen Thread
  gestartete Brücke oder eine Startdatei, die `serve_main_thread()` nie aufruft) würde nichts jemals ein Skript in
  der Warteschlange ausführen; deshalb werden Skripte direkt im Thread der Brücke aufgerufen. Beide Wege liefen in
  Blender 4.3.2 (siehe den Status oben).
- Ein Skript, das nie zurückkehrt, blockiert den Hauptthread von Blender; der Hub antwortet `timeout`, dann `busy`, bis
  das Skript endet, und sendet in der Zwischenzeit nie ein zweites Skript.
- Die Operatoren (`bpy.ops`) hängen vom Kontext ab, in dem sie laufen; aus einem Timer heraus gibt es keinen aktiven
  Bereich. `bpy.ops.mesh.primitive_cube_add` und `bpy.ops.render.render` liefen im Hintergrundmodus (siehe
  „Rendern“); `bpy.ops.render.render` lief auch mit Oberfläche auf einem virtuellen Display. Jeder andere Operator ist
  nicht ausprobiert: Die Daten-Schnittstelle (`bpy.data`) hängt nicht vom Kontext ab.

## Version

`app_version` ist das erste Wort von `bpy.app.version_string`, zum Beispiel `4.3.2`; eine Version mit Langzeitsupport
namens `4.2.3 LTS` ergibt `4.2.3`. Ohne `bpy` ist es `unknown`.

## Zustand

`get_state("blender")` liefert unter `state`:

| Schlüssel | Inhalt |
|---|---|
| `scene` | Der Name der aktiven Szene (`bpy.context.scene`) oder `null` ohne Szene |
| `objects` | Jedes Objekt der Szene: `name`, `type` (z. B. `MESH`, `CAMERA`, `LIGHT`) |
| `selected_objects` | Die Namen der ausgewählten Objekte; ein Objekt außerhalb der aktiven Ansichtsebene gilt als nicht ausgewählt |
| `frame_range` | Das erste und das letzte Einzelbild der Szene, `start` und `end`, oder `null` ohne Szene |
| `render_engine` | Die Render-Engine der Szene, z. B. `CYCLES`, oder `null` ohne Szene |

Namen von Szenen und Objekten sind Dokumentinhalt: Sie werden unverändert zurückgegeben und nie interpretiert, gleich
was sie sagen.

Lässt sich `bpy` nicht importieren (die Datei wurde außerhalb von Blender ausgeführt), ist `state` gleich `{}`,
`app_version` ist `unknown`, und `warnings` weist darauf hin.

## Keine Aufnahme

`screenshot` ist für Blender immer `null`. Ein Bild zu erzeugen hieße, stillschweigend ein Rendern zu starten, was eine
Anfrage zum Lesen des Zustands nie tun darf, und ohne Oberfläche gibt es keine sichere Kopie der 3D-Ansicht. Das Rendern
aus einem Skript beschreibt der nächste Abschnitt.

## Rendern

`bpy.ops.render.render(write_still=True)` funktioniert über die Brücke, wenn der Hauptthread das Skript ausführt: mit
Oberfläche (über den Timer) und in `blender -b`, wenn die Startdatei `serve_main_thread()` aufruft (siehe „Die Brücke
starten“). In Blender 4.3.2 lieferte das Rendern eines Würfels mit 128 mal 128 Pixeln in der Standard-Render-Engine
(`BLENDER_EEVEE_NEXT`) jedes Mal `ok` zurück, wenn es auf diese Weise ausprobiert wurde, und zwar in 0,6 bis 0,9 s (11
Läufe, mit und ohne Display-Variable und unter CPU-Last); `get_state("blender")` antwortete danach. Eine Startdatei, die
stattdessen nur wartet, lässt das Skript im Netzwerk-Thread der Brücke laufen. Dort lieferte dasselbe Rendern in 22 von
23 Läufen in 0,6 bis 0,9 s ein Ergebnis, und einmal kehrte es nie zurück: Der Hub antwortete nach 30 Sekunden
`timeout`, dann `busy`. Blender unterstützt `bpy.ops` außerhalb seines Hauptthreads nicht, deshalb kann dieser Thread bei
jedem Rendern hängen bleiben; führt man das Skript im Hauptthread aus, entfällt die Ursache. Setzen Sie `filepath`
unterhalb von `LMM_WORK_DIR`.

In einem neuen Profil ist das erste Rendern langsam. Fünf neue Profile lieferten das erste Rendern mit 128 mal 128
Pixeln in 30,0 bis 30,7 s, die folgenden in 0,12 bis 0,13 s. Die Standardfrist des Hubs beträgt 30 s: Vier der fünf
ersten Renderings lieferten `ok`, eines lieferte `timeout`, und die folgenden Aufrufe erhielten `busy`, bis es beendet
war. Erhöhen Sie `default_timeout_s` im Abschnitt `[hub]` von `config.toml`, oder rendern Sie einmal von Hand, bevor ein
Modell es tut.

Für ein Rendern über `bpy`, das pixelweise beurteilt wird, in der Render-Engine Workbench (gemessen in Blender 4.3.2,
jeder Punkt ein Bild, das ohne Fehlermeldung falsch herauskam):

- `scene.display.shading.color_type = "OBJECT"` liest `object.color`, standardmäßig Weiß: Alles wird weiß gerendert.
  Verwenden Sie `"MATERIAL"`.
- `material.diffuse_color` ist linear. Schreibt man den sRGB-Wert von `#c4553a`, wird `#E39C83` gerendert. Rechnen Sie
  jeden Kanal von sRGB in linear um, bevor Sie ihn zuweisen.
- Der Hintergrund eines Workbench-Bildes bleibt das Grau des Themes (`srgba(64,64,64,1)`), auch wenn eine Weltfarbe
  zugewiesen ist; `background_type = "WORLD"` ändert nichts, und `"CUSTOM"` schlägt im Hintergrundmodus fehl. Rendern
  Sie auf einen transparenten Film und setzen Sie den Hintergrund danach ein
  (`magick render.png -background "#f4efe8" -alpha remove -alpha off`).
- Eine Abschrägung, die nach `obj.scale` gesetzt wird, wird von der Skalierung überschrieben. Wenden Sie die
  Skalierung zuerst auf das Mesh an (`bpy.ops.object.transform_apply(scale=True)`) und fügen Sie dann die Abschrägung
  hinzu.
- Eine transparente PNG-Datei wirkt in einem Betrachter, der auf Weiß zusammensetzt, leer: Beurteilen Sie das Bild am
  zusammengesetzten Ergebnis und lesen Sie die tatsächlichen Farben mit einem Histogramm ab
  (`magick render.png +dither -colors 6 -format %c histogram:info:`).

In der Standard-Engine (`BLENDER_EEVEE_NEXT`, gemessen in Blender 4.3.2) ist die Ansichtstransformation einer neuen
Szene `AgX`; sie verändert die Farben auf dem Weg zur PNG-Datei: Eine als `#1c385c` geschriebene Weltfarbe wurde als
`#103759` gerendert, und die Antwort des Skripts lautete `ok`. Mit `scene.view_settings.view_transform = "Standard"`
wurde derselbe Hintergrund als `#1C385C` gerendert. Eine beleuchtete Fläche liest sich nie als die Farbe ihres
Materials, gleich welche Transformation gilt.

## Beispiel

Ein Skript, das der aktiven Szene mit der Daten-Schnittstelle ein Dreieck hinzufügt. Unverändert in der Standardszene
ausgeführt, liefert es `{"object": "Generated triangle", "objects": 4}` zurück.

```python
mesh = bpy.data.meshes.new("Generated mesh")
mesh.from_pydata([(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)], [], [(0, 1, 2)])
mesh.update()
item = bpy.data.objects.new("Generated triangle", mesh)
bpy.context.scene.collection.objects.link(item)
result = {"object": item.name, "objects": len(bpy.context.scene.objects)}
```

## Grenzen

- Die Skriptsperre des Hubs sieht die eigenen Speicher- und Exportfunktionen von Blender nicht: Ein Skript kann über
  `bpy.ops.wm.save_as_mainfile` eine Datei überschreiben. Die Sperre ist ein Sicherheitsnetz gegen Versehen, keine
  Sandbox.
- Ein Skript läuft mit den Rechten des Benutzers, im Python von Blender.
- Die Brücke bedient jeweils eine Verbindung.
- Ein Skript ersetzt die geöffnete Datei nicht. Während eines Skripts weist die Brücke `bpy.ops.wm.open_mainfile`,
  `wm.revert_mainfile`, `wm.read_homefile` und `wm.read_factory_settings` ab: Der Aufruf löst `RuntimeError` aus
  (`bpy.ops.wm.open_mainfile cannot run from a bridge script: it ends Blender. Start Blender on the file instead
  (blender -b <file>.blend --python <boot file>).`), der Hub antwortet `error`, und Blender läuft weiter. Ohne diese
  Abweisung beendeten diese Operatoren Blender 4.3.2 (der Hub antwortete `unavailable`). Um an einer anderen Datei zu
  arbeiten, starten Sie Blender darauf (`blender -b scene.blend --python <boot file>`).
- Cycles kann im Debian-Paket 4.3.2 nicht entrauschen: Mit `scene.cycles.use_denoising = True` löste
  `bpy.ops.render.render` den Fehler `Failed to denoise, build has no OpenImageDenoise support` aus. Lassen Sie es auf
  `False` und erhöhen Sie `scene.cycles.samples` (320 Samples bei 1360 mal 766 Pixeln dauerten in zwei Messungen 5 bis 23 s auf der CPU).
- Beim Rendern mit Workbench wird jedes Objekt als Vollkörper gezeichnet, was auch immer `object.display_type` sagt:
  `WIRE` änderte nichts. Ein `WIREFRAME`-Modifikator mit `use_replace = True` zeichnet die Kanten.
