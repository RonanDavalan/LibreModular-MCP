# Krita-Modul

**Status: ausprobiert in Krita 5.2.9 (Paket von Debian 13) auf einem virtuellen X-Display, noch nicht aus Scripter.**
Die Brücke wurde nach der öffentlichen Python-Schnittstelle von Krita 5 geschrieben (das Modul `krita` und seine
Klasse `Krita`). Sie lief in Krita 5.2.9, gestartet unter `xvfb-run`, mit einem Wegwerfprofil, in dem eine einzeilige
Erweiterung beim Start die erzeugte Datei im Thread der Oberfläche ausführte (wie Scripter es tut); gesteuert wurde
sie vom Hub über MCP auf Standardeingabe und Standardausgabe. `get_state("hub")` führte `krita` als verbunden auf, mit
der Version `5.2.9`. Ohne Dokument lieferte `get_state("krita")` `document: null`. Ein Skript erstellte ein Dokument
von 300 mal 200 Pixeln und öffnete es im Fenster; der Zustand führte danach das Dokument auf (`RGBA`, `U8`, Profil
`sRGB-elle-V2-srgbtrc.icc`), seine Ebene `Background` und eine PNG-Aufnahme von 300 mal 200 Pixeln (RGBA, Modus 600).
Das Beispiel unten fügte eine Ebene hinzu, und der nächste Zustand führte beide auf. Ein Skript, das eine Ausnahme
auslöste, lieferte seine Fehlerspur mit der richtigen Zeile zurück. Ein Aufruf dauerte nach dem Start zwischen 1 und
7 ms. Nicht Teil dieses Laufs: der Start der Brücke aus Scripter selbst (der Menüpfad, die Umgebungsvariablen, die sie
erreichen, und ihre Lebensdauer, wenn Scripter geschlossen wird).

## Was es tut

Der Hub erreicht Krita im Modus `tcp`. Eine Brücke läuft in Kritas Python, lauscht nur auf `127.0.0.1` und führt die
Skripte aus, die der Hub ihr sendet:

- `execute_script` führt ein vollständiges Python-Skript in Krita aus und liefert zurück, was es ausgegeben hat, den
  Wert seiner Variablen `result` und, wenn es fehlschlug, die eigene Fehlerspur von Python.
- `get_state` liefert das aktive Dokument, seine Ebenen und die aktive Ebene sowie den Pfad einer PNG-Aufnahme des
  Dokuments.

Die Brücke ist die erzeugte Datei `bridges/generated/krita_bridge.py`: der gemeinsame Kern (`bridges/_core.py`),
gefolgt vom Krita-Adapter (`bridges/adapters/krita.py`), zusammengesetzt von `python3 scripts/build_bridges.py`. Sie
nutzt nur die Standardbibliothek von Python und lässt sich ab Python 3.9 laden. Sie hält kein Token, sondern liest die
Token-Datei beim Start.

Das Paket installiert dieselbe Datei als `/usr/share/libremodular-mcp/bridges/krita_bridge.py`. Die folgenden Befehle nennen die Brückendatei eines Klons und danach die des Pakets.

## Die Brücke starten

Die Brücke startet nie von selbst. Im Verzeichnis für Kritas Python-Plug-ins, das eigene Erweiterungen des Benutzers
enthalten kann, wird nichts installiert: Weder der Code dieses Projekts noch dieses Verfahren schreibt dorthin. Der
Bediener führt die erzeugte Datei jedes Mal von Hand aus, in Kritas Scripter.

1. Legen Sie das Token einmalig an, falls es noch nicht existiert:

   ```sh
   libremodular-mcp init
   ```

2. Aktivieren Sie Krita in der Konfiguration des Hubs, `~/.config/libremodular-mcp/config.toml`:

   ```toml
   [apps.krita]
   enabled = true
   mode = "tcp"
   port = 9302
   ```

3. Starten Sie Krita. Die Brücke liest zwei Umgebungsvariablen:

   | Variable | Standardwert |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9302` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token` oder `~/.config/libremodular-mcp/token`, wenn `XDG_CONFIG_HOME` nicht gesetzt oder relativ ist |

   Mit den Standardwerten von `init` und der Konfiguration oben muss nichts gesetzt werden. Andernfalls setzen Sie die
   Variablen, bevor Krita startet, z. B. in einem Terminal:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9302 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   krita
   ```

   Die Brücke liest sie, wenn ihre Datei ausgeführt wird; deshalb lassen sie sich auch in Scripter über `os.environ`
   setzen, vor dem nächsten Schritt.

4. Öffnen Sie Scripter (in Krita 5 unter *Tools → Scripts → Scripter*; der genaue Menüpfad ist unbestätigt) und führen
   Sie Folgendes aus:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/krita_bridge.py", encoding="utf-8").read())
   ```

   Mit dem installierten Paket:

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/krita_bridge.py", encoding="utf-8").read())
   ```

   Die Brücke startet in einem Hintergrund-Thread, und Krita bleibt bedienbar. Sie schreibt eine Zeile in den
   Fehlerstrom des Prozesses und, wenn Scripter diesen Strom umleitet, in die Ausgabe von Scripter:

   ```text
   LMM bridge listening on 127.0.0.1:9302
   ```

   Die Brücke weigert sich zu starten, wenn die Token-Datei ein symbolischer Link ist, einem anderen Benutzer gehört,
   für die Gruppe oder andere lesbar oder beschreibbar ist, keine reguläre Datei ist oder kein Token enthält, das
   `libremodular-mcp init` angelegt hat. Sie läuft, bis Krita beendet wird; um sie früher zu beenden, führen Sie
   `stop()` in Scripter aus.

Um die Verbindung aus Sicht des Modells zu prüfen, rufen Sie `get_state` mit dem Ziel `hub` auf: Krita steht dann mit
`connected: true` und der Version in der Liste, die Krita gemeldet hat.

## Was ein Skript sieht

Jedes Skript läuft in einem frischen Namensraum, der Folgendes enthält:

| Name | Was es ist |
|---|---|
| `Krita` | Die Klasse der Python-Schnittstelle von Krita; `Krita.instance()` ist die Anwendung |
| `LMM_WORK_DIR` | Das private Verzeichnis, das der Hub für die Dateien dieser Anwendung vorbereitet hat |

- Das Skript wird unter dem Namen `<libremodular>` kompiliert: Die Zeilennummern in einer Fehlerspur sind die Zeilen
  des Skripts.
- Was das Skript ausgibt, kommt in `stdout` und `stderr` zurück. Setzen Sie `result` auf einen Wert, den JSON
  darstellen kann, um ihn zurückzugeben; jeder andere Wert liefert `null`, und an `stderr` wird eine Warnung
  angehängt.
- Dateien, die das Skript während der Ausführung anlegt, sind privat (Erstellungsmaske `077`).
- Die Schnittstelle von Krita muss im Thread ihrer Oberfläche verwendet werden. Startet die Brücke innerhalb von Krita
  (die `Krita`-Instanz existiert und eine Qt-Anwendung läuft), legt der Adapter ein kleines Qt-Objekt an, das in
  diesem Thread lebt; jedes Skript wird ihm über ein Signal in der Warteschlange übergeben, und der
  Verbindungs-Thread wartet auf das Ergebnis. Ein `QTimer`, der im Verbindungs-Thread gestartet würde, kommt nicht
  zum Einsatz: Dieser Thread hat keine Ereignisschleife von Qt, der Timer würde also nie auslösen. Außerhalb von
  Krita (kein Modul `krita`, oder PyQt ist ohne laufende Qt-Anwendung installiert) werden Skripte direkt aufgerufen.
  Der Weg über das Signal in der Warteschlange lief in Krita 5.2.9 (siehe den Status oben).
- Ein Skript, das nie zurückkehrt, blockiert die Oberfläche von Krita; der Hub antwortet `timeout`, dann `busy`, bis
  das Skript endet, und sendet in der Zwischenzeit nie ein zweites Skript.

## Zustand

`get_state("krita")` liefert unter `state`:

| Schlüssel | Inhalt |
|---|---|
| `document` | `null` ohne aktives Dokument; andernfalls die folgenden Schlüssel |
| `document.name` | Der Name des Dokuments |
| `document.width`, `document.height` | Seine Größe in Pixeln |
| `document.color_model`, `document.color_depth`, `document.color_profile` | So, wie Krita sie benennt, z. B. `RGBA`, `U8`, `sRGB-elle-V2-srgbtrc.icc` |
| `document.layers` | Die Ebenen der obersten Stufe: `id` (die eindeutige Kennung von Krita), `name`, `visible`, `opacity` (0 bis 255, wie Krita sie meldet), `type` (z. B. `paintlayer`, `grouplayer`, `vectorlayer`) |
| `document.active_layer` | Die `id` der aktiven Ebene oder `null` |

Ebenengruppen werden als eine Ebene aufgeführt; ihre untergeordneten Ebenen nicht.

Namen von Dokumenten und Ebenen sind Dokumentinhalt: Sie werden unverändert zurückgegeben und nie interpretiert, gleich
was sie sagen.

Lässt sich die Schnittstelle von Krita nicht verwenden (die Datei wurde außerhalb von Krita ausgeführt), ist `state`
`{}`, `app_version` ist `unknown`, und `warnings` weist darauf hin.

## Aufnahme

Die Aufnahme ist die Projektion des aktiven Dokuments: das zusammengeführte Bild, das Krita anzeigt, gelesen mit
`Document.projection` nach `Document.waitForDone` und als PNG unter dem Pfad gespeichert, den der Hub gewählt hat. Die
Projektion ist eine Kopie; das Dokument selbst wird weder gespeichert noch exportiert noch verändert. Ohne aktives
Dokument gibt es keine Aufnahme (`screenshot: null`). Der Hub prüft die Datei dann, bevor er ihren Pfad zurückgibt.

## Beispiel

Ein Skript, das dem aktiven Dokument eine leere Malebene hinzufügt. Unverändert ausgeführt auf einem neuen Dokument,
das nur seine Ebene `Background` hatte, liefert es `{"layers": ["Background", "Notes"]}` zurück.

```python
document = Krita.instance().activeDocument()
layer = document.createNode("Notes", "paintlayer")
document.rootNode().addChildNode(layer, None)
document.refreshProjection()
result = {"layers": [node.name() for node in document.topLevelNodes()]}
```

## Grenzen

- Die Skriptsperre des Hubs sieht die eigenen Speicher- und Exportfunktionen von Krita nicht: Ein Skript kann über
  `Document.saveAs` oder `Document.exportImage` eine Datei überschreiben. Die Sperre ist ein Sicherheitsnetz gegen
  Versehen, keine Sandbox.
- Ein Skript läuft mit den Rechten des Benutzers, im Python von Krita.
- Die Brücke bedient jeweils eine Verbindung.
- `Document.exportImage` und `Document.saveAs` öffnen den Optionsdialog des Dateiformats und warten auf eine Antwort,
  was die Oberfläche von Krita blockiert. Während eines Skripts schaltet die Brücke für diese beiden Aufrufe den
  Batch-Modus des Dokuments ein und setzt ihn danach auf den vorherigen Wert zurück; sie kehren daher mit `True` oder
  `False` zurück (`False`: Krita konnte die Datei nicht schreiben, zum Beispiel weil der Ordner fehlt), und es öffnet
  sich kein Dialog. Ein Skript braucht `document.setBatchmode(True)` nicht. Außerhalb eines Skripts, in Scripter oder
  in den Menüs von Krita, ändert sich nichts.
- Die Python-Schnittstelle von Krita kennt keinen Aufruf, der einen Pinselstrich zieht. Ein Skript zeichnet seine
  Striche selbst (`QPainter` von Qt auf einem `QImage`), schreibt die Pixel mit `node.setPixelData` in eine Ebene
  (Bytes in der Reihenfolge Blau, Grün, Rot, Alpha) und kann danach die eigenen Filter von Krita ausführen, zum
  Beispiel `Krita.instance().filter("oilpaint")`.
