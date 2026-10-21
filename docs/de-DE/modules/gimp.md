# GIMP-Modul

**Status: ausprobiert in GIMP 3.0.4, ohne Oberfläche gestartet, noch nicht in der interaktiven Python-Konsole.**
Die Brücke wurde nach der öffentlichen Schnittstelle von GIMP 3.0 geschrieben (GObject Introspection:
`gi.repository.Gimp`, `Gio`, `GLib`). Sie lief in GIMP 3.0.4 (Paket von Debian 13), gestartet mit
`gimp-3.0 --new-instance --no-interface --batch-interpreter
python-fu-eval -b`, wobei der Batch-Befehl die Brückendatei ausführte und danach `serve_main_thread()` aufrief (siehe
„Die Brücke starten“). In diesem Lauf, gesteuert vom Hub über MCP, führte `get_state("hub")` `gimp` als verbunden auf,
mit der Version `3.0.4`. Ein Skript erstellte ein Bild und eine Ebene, füllte die Ebene mit
`layer.fill(Gimp.FillType.FOREGROUND)` und exportierte das Bild nach `LMM_WORK_DIR` (`status: ok`, eine PNG-Datei von 128
mal 128 Pixeln); ein zweites Skript tat danach dasselbe, und die Brücke war weiterhin verbunden. `get_state("gimp")`
lieferte die unten genannten Schlüssel und eine PNG-Aufnahme von 200 mal 100 Pixeln im Modus 600. Ein Skript, das
fehlschlug, lieferte seine Fehlerspur mit der richtigen Zeile des Skripts zurück. Derselbe Befehl mit Oberfläche auf
einem virtuellen Bildschirm (`xvfb-run`) ergab dieselben Ergebnisse. Aussagen, die unten als unbestätigt gekennzeichnet
sind, waren nicht Teil dieser Läufe.

## Was es tut

Der Hub erreicht GIMP im Modus `tcp`. Eine Brücke läuft im Python von GIMP, lauscht nur auf `127.0.0.1` und führt die
Skripte aus, die der Hub ihr sendet:

- `execute_script` führt ein vollständiges Python-Skript in GIMP aus und liefert zurück, was es ausgegeben hat, den Wert
  seiner Variablen `result` und, wenn es fehlschlug, die eigene Fehlerspur von Python.
- `get_state` liefert die geöffneten Bilder, die Ebenen des aktiven Bildes, die Auswahl und den Pfad einer PNG-Aufnahme
  des aktiven Bildes.

Die Brücke ist die erzeugte Datei `bridges/generated/gimp_bridge.py`: der gemeinsame Kern (`bridges/_core.py`), gefolgt
vom GIMP-Adapter (`bridges/adapters/gimp.py`), zusammengesetzt von `python3 scripts/build_bridges.py`. Sie nutzt nur die
Standardbibliothek von Python und lässt sich ab Python 3.9 laden. Sie hält kein Token, sondern liest die Token-Datei beim
Start.

Das Paket installiert dieselbe Datei als `/usr/share/libremodular-mcp/bridges/gimp_bridge.py`. Die folgenden Befehle nennen die Brückendatei eines Klons und danach die des Pakets.

## Die Brücke starten

Die Brücke startet nie von selbst. Sie liegt in keinem Verzeichnis für Plug-ins oder automatisch geladene Dateien: Der
Bediener startet sie jedes Mal von Hand, in der Python-Konsole von GIMP oder über den Batch-Befehl eines GIMP, das ohne
Oberfläche gestartet wurde (siehe „Ohne Oberfläche gestartet“).

1. Legen Sie das Token einmalig an, falls es noch nicht existiert:

   ```sh
   libremodular-mcp init
   ```

2. Aktivieren Sie GIMP in der Konfiguration des Hubs, `~/.config/libremodular-mcp/config.toml`:

   ```toml
   [apps.gimp]
   enabled = true
   mode = "tcp"
   port = 9301
   ```

3. Starten Sie GIMP. Die Brücke liest zwei Umgebungsvariablen, die GIMP an seine Plug-ins weitergibt, auch an die
   Python-Konsole (im oben beschriebenen Lauf nicht Teil des Versuchs: Dort waren sie in der Prozessumgebung der
   Batch-Instanz gesetzt):

   | Variable | Standardwert |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9301` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token` oder `~/.config/libremodular-mcp/token`, wenn `XDG_CONFIG_HOME` nicht gesetzt oder relativ ist |

   Mit den Standardwerten von `init` und der Konfiguration oben muss nichts gesetzt werden. Andernfalls setzen Sie die
   Variablen, bevor GIMP startet, z. B. in einem Terminal:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9301 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   gimp-3.0
   ```

   Die Brücke liest sie, wenn ihre Datei ausgeführt wird, nicht wenn GIMP startet; deshalb lassen sie sich auch in der
   Konsole selbst setzen, vor dem nächsten Schritt:

   ```python
   import os
   os.environ["LIBREMODULAR_BRIDGE_PORT"] = "9301"
   os.environ["LIBREMODULAR_TOKEN_PATH"] = "/home/alice/.config/libremodular-mcp/token"
   ```

4. Öffnen Sie die Python-Konsole (in GIMP 3.0 unter *Filters → Development → Python-Fu*; der genaue Menüpfad ist
   unbestätigt) und führen Sie die erzeugte Datei aus:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/gimp_bridge.py").read())
   ```

   Mit dem installierten Paket:

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/gimp_bridge.py").read())
   ```

   Die Brücke startet in einem Hintergrund-Thread, und die Konsole bleibt bedienbar. Sie schreibt eine Zeile in den
   Fehlerstrom des Prozesses (sichtbar, wenn GIMP aus einem Terminal gestartet wurde) und, wenn die Konsole diesen
   Strom umleitet, in die Konsole (unbestätigt):

   ```text
   LMM bridge listening on 127.0.0.1:9301
   ```

   Der Port ist der, der tatsächlich gebunden wurde. Die Brücke weigert sich zu starten, mit einer Meldung in der
   Konsole, wenn die Token-Datei ein symbolischer Link ist, einem anderen Benutzer gehört, für die Gruppe oder andere
   lesbar oder beschreibbar ist, keine reguläre Datei ist oder kein Token enthält, das `libremodular-mcp init`
   angelegt hat.

5. Lassen Sie die Konsole geöffnet. In GIMP 3 ist die Python-Konsole ein Plug-in, das in einem eigenen Prozess läuft;
   die Brücke lebt in diesem Prozess und endet, wenn die Konsole geschlossen wird (unbestätigt). Um die Brücke zu
   beenden, ohne die Konsole zu schließen, führen Sie darin `stop()` aus.

### Ohne Oberfläche gestartet

Mit `--no-interface` hat GIMP keine Ereignisschleife, und der Batch-Befehl ist die ganze Lebensdauer des Prozesses, der
die Brücke ausführt: Endet der Befehl, endet dieser Prozess und mit ihm die Brücke (GIMP selbst bleibt am Laufen).
Führen Sie die Brückendatei im Batch-Befehl aus und übergeben Sie den Hauptthread an die Brücke, indem Sie
`serve_main_thread()` aufrufen, das die Brückendatei definiert:

```sh
LIBREMODULAR_BRIDGE_PORT=9301 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
gimp-3.0 --new-instance --no-interface --batch-interpreter python-fu-eval \
  -b "exec(open('/home/alice/LibreModular-MCP/bridges/generated/gimp_bridge.py').read()); serve_main_thread()"
```

Mit dem installierten Paket:

```sh
LIBREMODULAR_BRIDGE_PORT=9301 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
gimp-3.0 --new-instance --no-interface --batch-interpreter python-fu-eval \
  -b "exec(open('/usr/share/libremodular-mcp/bridges/gimp_bridge.py').read()); serve_main_thread()"
```

`--batch-interpreter python-fu-eval` ist erforderlich: `python-eval` ist in GIMP 3.0.4 kein Interpreter, und
`--batch-interpreter-args` gibt es nicht. `serve_main_thread()` lässt die Hauptschleife von GLib in dem Thread laufen,
der es aufgerufen hat, damit die Skripte, die die Brücke in die Warteschlange stellt, dort ausgeführt werden. Es kehrt
zurück, wenn die Brücke endet (`stop()`), nach `seconds`, wenn Sie `serve_main_thread(seconds=600)` übergeben, oder bei
Strg+C; der Batch-Befehl endet dann, und mit ihm die Brücke. Ein Batch-Befehl, der es nicht aufruft oder der seine
eigene Schleife beendet, schließt den Port 9301, sobald er zurückkehrt. Die oben genannten Läufe übergaben außerdem
`--no-data --no-fonts` und verwendeten ein eigenes Profilverzeichnis, damit das Profil des Bedieners unberührt blieb.

Um die Verbindung aus Sicht des Modells zu prüfen, rufen Sie `get_state` mit dem Ziel `hub` auf: GIMP steht dann mit
`connected: true` und der Version in der Liste, die GIMP gemeldet hat.

## Was ein Skript sieht

Jedes Skript läuft in einem frischen Namensraum, der Folgendes enthält:

| Name | Was es ist |
|---|---|
| `Gimp`, `Gio`, `GLib` | Die Introspektionsmodule, sofern sie sich importieren lassen |
| `images()` | Die in GIMP geöffneten Bilder, so wie `Gimp.get_images()` sie aufführt |
| `LMM_WORK_DIR` | Das private Verzeichnis, das der Hub für die Dateien dieser Anwendung vorbereitet hat |

- Das Skript wird unter dem Namen `<libremodular>` kompiliert: Die Zeilennummern in einer Fehlerspur sind die Zeilen
  des Skripts.
- Was das Skript ausgibt, kommt in `stdout` und `stderr` zurück. Setzen Sie `result` auf einen Wert, den JSON
  darstellen kann, um ihn zurückzugeben; jeder andere Wert liefert `null`, und an `stderr` wird eine Warnung
  angehängt.
- Dateien, die das Skript während der Ausführung anlegt, sind privat (Erstellungsmaske `077`).
- Die Brücke übergibt jedes Skript über `GLib.idle_add` an den Hauptthread von GIMP und wartet darauf: Die
  Schnittstelle von GIMP ist nicht dafür gedacht, aus einem anderen Thread aufgerufen zu werden. Das funktionierte in
  GIMP 3.0.4 ohne Oberfläche, mit dem Hauptthread in `serve_main_thread()`; in der interaktiven Konsole ist es nicht
  ausprobiert. Außerhalb von GIMP, auch auf einer Maschine, auf der `gi` und die GIMP-Typelib installiert sind, erfolgt
  der Aufruf direkt im eigenen Thread der Brücke, weil keine Hauptschleife ihn je ausführen würde.
- Ein Skript, das nie zurückkehrt, blockiert den Hauptthread von GIMP; der Hub antwortet `timeout`, dann `busy`, bis das
  Skript endet, und sendet in der Zwischenzeit nie ein zweites Skript.
- `sys.exit()` in einem Skript beendet das Skript mit dem Fehler `SystemExit`, nicht die Brücke.

## Zustand

`get_state("gimp")` liefert unter `state`:

| Schlüssel | Inhalt |
|---|---|
| `images` | Jedes geöffnete Bild: `id`, `name`, `width`, `height`, `color_mode` (der Kurzname, den GIMP dem Basistyp gibt: `rgb` wurde beobachtet; `gray` und `indexed` werden erwartet, unbestätigt) |
| `active_image` | `null`, wenn kein Bild geöffnet ist; andernfalls das erste Bild, das `Gimp.get_images()` zurückgibt, mit den folgenden Schlüsseln |
| `active_image.id`, `active_image.name` | Wie in `images` |
| `active_image.layers` | Die Ebenen der obersten Stufe: `id`, `name`, `visible`, `opacity` (0 bis 100), `blend_mode` (z. B. `normal`, `multiply`) |
| `active_image.active_layer` | Die `id` der ersten ausgewählten Ebene oder `null` |
| `active_image.selection` | `null` ohne Auswahl, andernfalls ihr umschließendes Rechteck: `x`, `y`, `width`, `height` |

Welches Bild GIMP als erstes aufführt und welches deshalb als aktiv gilt, ist nur bei einem geöffneten Bild bestätigt;
bei mehreren ist es unbestätigt. Eine nicht leere Auswahl wurde nicht geprüft: Beobachtet wurde nur `selection: null`.
Ebenengruppen werden als eine Ebene aufgeführt; ihre untergeordneten Ebenen nicht.

Namen von Bildern und Ebenen sind Dokumentinhalt: Sie werden unverändert zurückgegeben und nie interpretiert, gleich, was
sie sagen.

Lässt sich die Schnittstelle von GIMP nicht verwenden (die Datei wurde außerhalb von GIMP ausgeführt, mit oder ohne
installiertes `gi`), ist `state` gleich `{}`, `app_version` ist `unknown`, und `warnings` weist darauf hin.

## Aufnahme

Die Aufnahme ist eine PNG-Datei des aktiven Bildes. Die Brücke dupliziert das Bild, führt die sichtbaren Ebenen der
Kopie zusammen, exportiert die Kopie mit `Gimp.file_save` unter dem Pfad, den der Hub gewählt hat, und löscht die Kopie
danach. Würde das eigene Bild des Benutzers exportiert, änderte sich der Dateiname, den GIMP sich für spätere Exporte
merkt; die Kopie lässt ihn unberührt. Ohne geöffnetes Bild gibt es keine Aufnahme (`screenshot: null`). Der Hub prüft die
Datei dann, bevor er ihren Pfad zurückgibt.

## Beispiel

Ein Skript, das dem aktiven Bild eine transparente Ebene hinzufügt. Es wurde nicht so ausgeführt, wie es dasteht (es
braucht ein geöffnetes Bild, und `Gimp.displays_flush()` wurde nicht ausprobiert); ein ähnliches Skript, das sein
eigenes Bild und seine eigene Ebene erzeugte, lief in GIMP 3.0.4 und lieferte `{'layers': ['<name of the layer>']}`
zurück.

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

## GIMP-3.0-Schnittstelle: umbenannte Aufrufe und stille Fehlschläge

Beobachtet in GIMP 3.0.4. Ein Skript, das aus der Erinnerung an GIMP 2.10 geschrieben wurde, scheitert mit einem
`AttributeError` oder einem `TypeError`; lesen Sie `dir()` im laufenden GIMP, statt zu folgern, die Schnittstelle sei
kaputt.

| Aufgabe | GIMP 2.10 | GIMP 3.0 |
|---|---|---|
| Ein Bild anlegen | `Gimp.Image.new(w, h, Gimp.ImageType.RGBA_IMAGE)` | `Gimp.Image.new(w, h, Gimp.ImageBaseType.RGB)`: Das dritte Argument ist ein Basistyp |
| Eine Farbe erzeugen | `Gimp.RGBA(r, g, b, a)` | `Gimp.color_parse_hex("#14181c")`, das ein `Gegl.Color` zurückgibt; `Gimp.RGBA` und `Gimp.RGB` gibt es nicht |
| Ein Rechteck auswählen | `Gimp.image_select_rectangle(image, Gimp.ChannelOp.REPLACE, ...)` | `image.select_rectangle(Gimp.ChannelOps.REPLACE, ...)`: eine Instanzmethode, und aus `ChannelOp` wurde `ChannelOps` |
| Mit dem Vordergrund füllen | `Gimp.edit_bucket_fill(image, layer, Gimp.BucketFillType.FG, ...)` | `layer.fill(Gimp.FillType.FOREGROUND)`; aus `BucketFillType` wurde `FillType` |
| Eine Datei laden | `Gimp.File.load(gfile)` | `Gimp.file_load(Gimp.RunMode.NONINTERACTIVE, gfile)` |
| Die aktive Ebene setzen | `Gimp.image_set_active_layer(image, layer)` | gibt es nicht |

Drei Aufrufe schlagen fehl, ohne einen Fehler auszulösen:

- `image.insert_layer(layer, None, 0)` mit einer Ebene, die zu einem anderen Bild gehört, bewirkt nichts:
  `image.get_layers()` bleibt leer. Kopieren Sie die Ebene zuerst mit `Gimp.Layer.new_from_drawable(layer, image)` und
  fügen Sie dann die Kopie ein.
- `layer.edit_bucket_fill(...)` ließ eine Ebene eines Bildes vom Typ `Gimp.ImageBaseType.RGB` völlig transparent (das
  zusammengefügte Bild las sich als `srgba(0,0,0,0)`), während `layer.fill(Gimp.FillType.FOREGROUND)` sie füllte. Die
  Brücke stellt keine aktive Ebene sicher, die der Farbeimer offenbar braucht.
- `layer.scale(width, height, local_origin)` mit einer negativen Höhe (`-1`, „Seitenverhältnis beibehalten“) bewirkt
  nichts; mit zwei positiven Maßen ändert es die Größe der Ebene.

## Grenzen

- Die Skriptsperre des Hubs sieht die eigenen Speicher- und Exportfunktionen von GIMP nicht: Ein Skript kann über
  `Gimp.file_save` eine Datei überschreiben. Die Sperre ist ein Sicherheitsnetz gegen Versehen, keine Sandbox.
- Ein Skript läuft mit den Rechten des Benutzers, im Python von GIMP.
- Die Brücke bedient jeweils eine Verbindung. Eine Verbindung, die eintrifft, während eine andere bedient wird, wird
  sofort geschlossen, auch eine erneute Verbindung wenige Millisekunden nach dem Schließen der vorigen, bevor die Brücke
  es bemerkt hat.
