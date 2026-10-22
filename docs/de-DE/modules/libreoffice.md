# LibreOffice-Modul

**Status: ausprobiert in LibreOffice 24.2.7.2 und 25.2.3.2.** Der Adapter und der Starter wurden nach der öffentlichen
UNO-Schnittstelle und nach Messungen an LibreOffice 25.2.3.2 geschrieben; danach liefen sie gegen LibreOffice 24.2.7.2
(die Ubuntu-24.04-Pakete `libreoffice-core`, `-writer`, `-calc`, `-impress` und `python3-uno`, mit dem System-Python
`/usr/bin/python3` 3.11.15) und gegen LibreOffice 25.2.3.2 (die Pakete von Debian 13, mit dem System-Python
`/usr/bin/python3` 3.13.5), und zwar auf zwei Wegen:

- ohne Oberfläche, über `cargo test --test libreoffice_real -- --ignored --nocapture`; dieser Test startet ein eigenes
  LibreOffice auf einem Wegwerfprofil und einem eigenen Port, startet die erzeugte Brücke mit der Umgebung, die der
  Starter ihr gibt, und steuert die drei Ziele über den Hub an (bestanden auf beiden Versionen);
- mit einem Fenster auf einem virtuellen X-Display, gestartet vom echten `libremodular-launch` und gesteuert vom
  echten `libremodular-mcp` über MCP auf Standardeingabe und Standardausgabe: `get_state("hub")` führte die drei Ziele
  als verbunden im Modus `uno` auf, mit der Version von LibreOffice (`24.2.7.2`, dann `25.2.3.2`); nachdem im
  Fenster ein Calc-Dokument geöffnet worden war, schrieb ein Skript zwei Zellen, `get_state` beschrieb das
  Tabellenblatt und lieferte eine PNG-Aufnahme, ein Skript für Writer wurde mit `WrongDocumentType` abgelehnt, und
  `--stop` beendete die Brücke und ließ LibreOffice laufen.

Was ein Skript tut, wenn kein Dokument geöffnet ist (es läuft, mit `doc` gleich `None`, und kann das erste Dokument
erstellen), kam nach diesen beiden Läufen hinzu. Es lief nur gegen LibreOffice 24.2.7.2, ohne Oberfläche
(`cargo test --test libreoffice_real`) und mit einem Fenster auf einem virtuellen X-Display (das Beispiel am Ende dieser
Seite). Eine Brücke, die vor LibreOffice startet, und die Version, die sie dann meldet, liefen nur gegen 24.2.7.2 und
ohne Oberfläche: siehe *Grenzen*.

Nicht Teil dieser Läufe: eine echte Desktop-Sitzung, ein Dokument mit mehreren Tabellenblättern oder Seiten (was seine
Aufnahme zeigt), Dokumente anderer Art, die neben den drei Familien geöffnet sind, und ein LibreOffice, das länger zum
Starten braucht, als die Brücke wartet (siehe die Version unter *Grenzen*). Die Unit-Tests des Adapters verwenden ein
nachgebildetes `uno` und laufen gleich, ob LibreOffice installiert ist oder nicht.

## Was es tut

Der Hub erreicht LibreOffice im Modus `uno`, über **drei Ziele, die sich eine Brücke und eine Verbindung teilen**:

| Ziel | Familie | Dokumente, die es bedient |
|---|---|---|
| `libreoffice-writer` | `writer` | Textdokumente |
| `libreoffice-calc` | `calc` | Tabellenkalkulationen |
| `libreoffice-impress` | `impress` | Präsentationen |

Die Brücke ist ein eigenständiger Python-Prozess, kein Plug-in: Sie verbindet sich für jede Anfrage mit dem UNO-Port,
auf dem LibreOffice lauscht, und führt das Skript dort aus. Der Hub spricht nie UNO: Er spricht mit der Brücke über
`bridge_port` wie mit jeder anderen Brücke und nennt in jeder Anfrage die Familie des Ziels. Läuft ein Skript auf einem
Ziel, antwortet eine Anfrage an ein anderes Ziel mit `busy`, denn die Brücke führt jeweils nur ein Skript aus.

- `execute_script` führt ein vollständiges Python-Skript für das **aktuelle Dokument** der Familie des Ziels aus, oder
  mit `doc` gleich `None`, wenn LibreOffice kein aktuelles Dokument hat, und liefert zurück, was es ausgegeben hat, den
  Wert seiner Variablen `result` und, wenn es fehlschlug, die Fehlerspur.
- `get_state` führt die geöffneten Dokumente auf, beschreibt das aktive Dokument und liefert den Pfad einer PNG-Aufnahme
  davon.

Die Brücke ist die erzeugte Datei `bridges/generated/uno_bridge.py`: der gemeinsame Kern (`bridges/_core.py`), gefolgt
vom Adapter (`bridges/adapters/uno.py`), zusammengesetzt von `python3 scripts/build_bridges.py`. Sie nutzt nur die
Standardbibliothek von Python, abgesehen von `uno`, das sie importiert, wenn eine Anfrage es braucht; sie lässt sich ab
Python 3.9 laden, mit oder ohne `uno`. Sie hält kein Token.

Das Paket installiert dieselbe Datei als `/usr/share/libremodular-mcp/bridges/uno_bridge.py`. Die folgenden Befehle nennen die Brückendatei eines Klons und danach die des Pakets.

## Die Grenze, die Sie zuerst kennen müssen

**Der UNO-Port von LibreOffice hat keine Authentifizierung.** Wer sich mit ihm verbinden kann, kann LibreOffice
steuern: Dokumente öffnen, ändern, speichern und schließen und mit den Rechten des Benutzers alles erreichen, was das
Python von LibreOffice erreichen kann. Der Port braucht kein Token, durchläuft keine Skriptsperre und hinterlässt nichts
im Audit-Journal. Nur die Brücke ist durch das Token geschützt, und nur gegenüber der Seite des Hubs.

- LibreOffice wird angewiesen, nur auf `127.0.0.1` zu lauschen, sodass nichts außerhalb der Maschine es erreicht. Jeder
  andere Prozess, der auf dieser Maschine eine Verbindung zu diesem Port öffnen kann, erreicht es.
- Ob LibreOffice Verbindungen nach Benutzer filtert, ist unbestätigt: Die Läufe oben haben sich als derselbe Benutzer
  verbunden. Gehen Sie davon aus, dass es nicht filtert.
- Der Port besteht so lange, wie LibreOffice mit `--accept` gestartet wurde. Schließen Sie diese Instanz, wenn Sie sie
  nicht brauchen; der Starter tut das nie für Sie.
- Die Skriptsperre des Hubs prüft nur die Skripte, die über `execute_script` gesendet werden. Ein Programm, das
  unmittelbar mit dem UNO-Port spricht, wird von ihr nicht gesehen.

## Starten

LibreOffice und die Brücke werden nie vom Hub gestartet und nie auf das Wort des Modells hin. Beide starten auf eine
ausdrückliche Geste des Bedieners.

Aktivieren Sie die Ziele in `~/.config/libremodular-mcp/config.toml`. Die drei Ziele sind ein LibreOffice und eine
Brücke: Aktivierte Ziele müssen dieselben Ports nennen, sonst weigert sich der Starter. Die Werte unten sind die
Standardwerte.

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

`uno_port` ist der Port, auf dem LibreOffice lauscht, und nur der Starter verwendet ihn; `bridge_port` ist der Port der
Brücke, mit dem sich der Hub verbindet.

### Mit dem Starter

Legen Sie das Token einmalig an, falls es noch nicht existiert (`libremodular-mcp init`), und führen Sie dann aus:

```sh
libremodular-launch libreoffice --bridge-file /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
```

Mit dem installierten Paket:

```sh
libremodular-launch libreoffice --bridge-file /usr/share/libremodular-mcp/bridges/uno_bridge.py
```

Er startet LibreOffice mit seinem Fenster, danach die Brücke, und kehrt zurück. Ausgabe eines Laufs:

```text
LibreOffice started (process 32212), listening for UNO on 127.0.0.1:2002
bridge started (process 32213), listening on 127.0.0.1:9304
a script runs on the current document of its target's family, or with doc set to None when none is open
stop the bridge with 'libremodular-launch libreoffice --stop'; LibreOffice is yours to close
```

Was der Starter tut und was nicht:

- LibreOffice wird als `/usr/bin/soffice --accept=socket,host=127.0.0.1,port=2002;urp;
  --norestore --nologo --nodefault` gestartet, aus einer festen Argumentliste, ohne Shell und ohne `--headless`. Es
  erhält die Umgebung des Starters, mit Ausnahme der Variablen, deren Name mit `LIBREMODULAR_` beginnt.
- Die Brücke wird als `/usr/bin/python3 <the bridge file>` gestartet, mit genau diesen Variablen: `PATH`, `HOME`, `LANG`
  und `LC_ALL`, so wie der Starter sie hat, `LD_LIBRARY_PATH` (`/usr/lib/libreoffice/program`) und `PYTHONPATH`
  (`/usr/lib/python3/dist-packages`), damit das System-Python `uno` findet, sowie `LIBREMODULAR_UNO_PORT`,
  `LIBREMODULAR_BRIDGE_PORT` und `LIBREMODULAR_TOKEN_PATH`. Das Token selbst steht in keinem Argument und in keiner
  Variablen: Die Brücke liest die Datei.
- Beide Prozesse werden abgekoppelt, jeweils in einer eigenen Gruppe, und ihre Ein- und Ausgabe geht ins Leere. Sie
  laufen weiter, wenn der Starter zurückkehrt.
- Die Brückendatei muss ein absoluter Pfad zu einer regulären Datei sein, kein Link, Ihnen oder root gehören und für die
  Gruppe und andere nicht beschreibbar sein. Ein frischer Klon, der mit einer `umask` von `002` angelegt wurde (der
  Voreinstellung vieler Debian-Konten), ergibt den Modus `664`, den der Starter ablehnt: Führen Sie einmalig
  `chmod 600 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py` aus. Die vom Paket installierte Datei
  gehört root mit dem Modus `644`: Es ist nichts zu ändern. Das Verzeichnis, das die Datei enthält,
  wird nicht geprüft.
- **Er weigert sich zu starten, wenn schon ein LibreOffice von Ihnen läuft** (ein Prozess `soffice.bin` Ihres Benutzers,
  aus `/proc` gelesen). Er verbindet sich nie mit einer Instanz, die ungespeicherte Arbeit enthalten kann, und
  schließt nie eine, die er nicht gestartet hat. Schließen Sie LibreOffice und führen Sie den Starter erneut aus, oder
  starten Sie LibreOffice selbst, wie unten gezeigt.
- Er weigert sich, eine zweite Brücke zu starten, solange die Brücke eines früheren Starts noch läuft.
- Die Prozesse, die er gestartet hat, werden in `$XDG_RUNTIME_DIR/libremodular-mcp/launch/libreoffice.pids` festgehalten
  (Verzeichnis `700`, Datei `600`):

  ```text
  soffice 32212
  bridge 32213 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
  ```

  Die erste Zahl ist der Prozess, den der Starter gestartet hat; es ist nicht der Prozess `soffice.bin`, den das System
  auflistet (Prozess 32240 in jenem Lauf).

`libremodular-launch libreoffice --stop` beendet die Brücke, **erst nachdem es geprüft hat, dass der festgehaltene
Prozess noch die Brücke ist** (eines seiner Argumente ist genau die Brückendatei): Eine Nummer, die inzwischen ein
anderes Programm übernommen hat, erhält nie ein Signal. Es schließt LibreOffice nicht. Eine Instanz zu schließen, die
Arbeit enthalten kann, ist Ihre Geste, und die Meldung sagt das.

Exit-Codes: `0` erledigt; `2` eine falsche Verwendung oder eine Ablehnung, die Sie beheben können (kein aktiviertes
Ziel, Ports, die nicht übereinstimmen oder gleich sind, eine Brückendatei, der man nicht vertrauen kann, ein
LibreOffice oder eine Brücke, die schon läuft); `1` ein Fehlschlag (eine unlesbare Konfiguration, kein
`XDG_RUNTIME_DIR`, ein Programm, das nicht gestartet ist). Die anderen Anwendungen werden von Hand gestartet und
antworten mit `start the bridge by hand, see docs/modules/<name>.md` und dem Code `2`.

Lässt sich die Brücke nicht starten, nachdem LibreOffice gestartet wurde, sagt der Starter das, hält LibreOffice
allein fest und lässt es laufen.

### Von Hand

Starten Sie LibreOffice mit der Option `--accept`. Als Host dient die Nummer `127.0.0.1`, nie ein Name, der sich in
eine Adresse auflösen könnte, auf der LibreOffice nicht lauscht:

```sh
soffice --accept="socket,host=127.0.0.1,port=2002;urp;" --norestore --nologo --nodefault
```

Starten Sie dann, sobald LibreOffice läuft, die Brücke mit dem System-Python, das `uno` mitbringt:

```sh
LD_LIBRARY_PATH=/usr/lib/libreoffice/program \
PYTHONPATH=/usr/lib/python3/dist-packages \
LIBREMODULAR_UNO_PORT=2002 \
LIBREMODULAR_BRIDGE_PORT=9304 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
/usr/bin/python3 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
```

Mit dem installierten Paket:

```sh
LD_LIBRARY_PATH=/usr/lib/libreoffice/program \
PYTHONPATH=/usr/lib/python3/dist-packages \
LIBREMODULAR_UNO_PORT=2002 \
LIBREMODULAR_BRIDGE_PORT=9304 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
/usr/bin/python3 /usr/share/libremodular-mcp/bridges/uno_bridge.py
```

Die Brücke schreibt eine Zeile in ihren Fehlerstrom, `LMM bridge listening on 127.0.0.1:9304`, und läuft, bis der
Prozess beendet wird. Sie weigert sich zu starten, wenn die Token-Datei ein Link ist, einem anderen Benutzer gehört, für
die Gruppe oder andere zugänglich ist oder kein Token ist, das `libremodular-mcp init` angelegt hat.

### Das erste Dokument

**Ein Skript läuft auch dann, wenn kein Dokument geöffnet ist.** Dann ist `doc` gleich `None`, und `uno`, `ctx` und
`desktop` stehen wie gewohnt zur Verfügung, sodass das Skript das Dokument selbst erstellen kann:

```python
if doc is None:
    doc = desktop.loadComponentFromURL('private:factory/scalc', '_blank', 0, ())
```

Das Dokument, das es erstellt, wird zum aktuellen Dokument von LibreOffice: Die nächste Anfrage findet es als ihr
`doc`. Die Namen der Fabriken sind `private:factory/swriter`, `private:factory/scalc` und `private:factory/simpress`. Das
Skript eines Ziels erstellt ein Dokument jeder Familie; die nächste Anfrage wird mit `WrongDocumentType` abgelehnt, wenn
die Familie des Ziels nicht die des aktuellen Dokuments ist (siehe *Fehler*).

Um die Verbindung aus Sicht des Modells zu prüfen, rufen Sie `get_state` mit dem Ziel `hub` auf: Die drei Ziele stehen
dann mit `connected: true` und der Version in der Liste, die LibreOffice gemeldet hat.

## Was ein Skript sieht

Ein Skript läuft in der Brücke, die ein eigener Prozess ist, in einem frischen Namensraum, der Folgendes enthält:

| Name | Was es ist |
|---|---|
| `uno` | Das Modul `uno` |
| `ctx` | Der entfernte Komponentenkontext von LibreOffice |
| `desktop` | `com.sun.star.frame.Desktop`, die Anwendung |
| `doc` | Das aktuelle Dokument, aus der Familie des Ziels; `None`, wenn LibreOffice kein aktuelles Dokument hat |
| `LMM_WORK_DIR` | Das private Verzeichnis, das der Hub für die Dateien dieser Anwendung vorbereitet hat |

- Das Skript wird unter dem Namen `<libremodular>` kompiliert: Die Zeilennummern in einer Fehlerspur sind die Zeilen
  des Skripts.
- Was das Skript ausgibt, kommt in `stdout` und `stderr` zurück. Setzen Sie `result` auf einen Wert, den JSON
  darstellen kann, um ihn zurückzugeben; jeder andere Wert liefert `null`, und an `stderr` wird eine Warnung
  angehängt. UNO-Objekte sind kein JSON: Geben Sie einfache Werte zurück.
- Die Verbindung zu LibreOffice wird für jede Anfrage hergestellt und bis zu fünf Sekunden lang erneut versucht, solange
  LibreOffice noch nicht fertig gestartet ist.
- Die Familie wird geprüft, bevor etwas vom Skript läuft, sofern es ein aktuelles Dokument gibt. Das Dokument wird nie
  umgewandelt: Ein Calc-Dokument wird einem Writer-Skript nicht angeboten.
- Eine Ausnahme, die UNO auslöst, wird mit ihrem vollen UNO-Namen gemeldet, zum Beispiel
  `com.sun.star.container.NoSuchElementException`.
- Was das Skript über UNO tut (speichern, schließen, exportieren), geschieht mit Ihren Rechten; die Skriptsperre des
  Hubs sieht die Speicher- und Exportfunktionen der Anwendung nicht.

## Zustand

`get_state("libreoffice-calc")`, und ebenso für die beiden anderen Ziele, liefert unter `state`:

| Schlüssel | Inhalt |
|---|---|
| `documents` | Jedes geöffnete Dokument der drei Familien, in der Reihenfolge, in der LibreOffice sie auflistet: `title` und `family` (`writer`, `calc` oder `impress`). Zeichnungen und andere Komponenten werden nicht aufgeführt |
| `active` | `null` oder die Beschreibung des aktiven Dokuments, siehe unten |

`active` ist `null`, mit einer Warnung, wenn LibreOffice kein aktuelles Dokument hat oder sein aktuelles Dokument nicht
zur Familie des Ziels gehört. Das ist kein Fehler. Die Brücke kann die beiden Fälle nicht unterscheiden, deshalb nennt
die Warnung beide.

| Familie | Inhalt von `active` |
|---|---|
| `writer` | `paragraphs`: die Anzahl der Absätze (Tabellen werden daneben aufgeführt und nicht mitgezählt); `pages`: die Seitenzahl oder `null` für ein Dokument ohne Controller |
| `calc` | `sheets`: eine Liste aus `name`, `last_column` und `last_row`, den nullbasierten Indizes der letzten benutzten Zelle, so wie UNO sie liefert. Ein leeres Tabellenblatt meldet `0` und `0`, genau wie ein Tabellenblatt, dessen einziger Inhalt `A1` ist |
| `impress` | `slides`: die Anzahl der Folien |

Titel und Namen von Tabellenblättern sind Dokumentinhalt: Sie werden unverändert zurückgegeben und nie interpretiert,
gleich, was sie sagen.

Lässt sich `uno` vom Interpreter, der die Brücke ausführt, nicht importieren, ist `state` gleich `{}`, `app_version` ist
`unknown`, und `warnings` weist darauf hin. Dasselbe gilt, wenn LibreOffice die Verbindung nicht annimmt, mit dem Grund
in der Warnung.

## Aufnahme

Die Aufnahme ist ein Export des aktiven Dokuments der Familie, den LibreOffice mit seinem PNG-Filter
(`writer_png_Export`, `calc_png_Export` oder `impress_png_Export`) unter dem Pfad erstellt, den der Hub gewählt hat.
Das Dokument wird weder gespeichert noch geschlossen noch geändert: Nach einer Aufnahme ist `hasLocation()` weiterhin
`False`, und das Dokument ist weiterhin geändert. Nichts in der Brücke ruft `store`, `storeSelf`, `storeAsURL`, `close`,
`dispose` oder `terminate` auf, und die Tests schlagen fehl, wenn doch.

- Die Datei wird vom eigenen Prozess von LibreOffice geschrieben, mit der Erstellungsmaske dieses Prozesses. Der Hub
  prüft sie anschließend und macht sie in seinem eigenen privaten Verzeichnis privat (Modus `600`).
- Ohne ein aktives Dokument der Familie gibt es keine Aufnahme (`screenshot: null`).
- Was die PNG-Datei eines Dokuments mit mehreren Tabellenblättern oder Seiten zeigt, ist unbestätigt.

## Fehler

| `error.type` | Wann |
|---|---|
| `LibreOfficeUnavailable` | `uno` lässt sich vom Interpreter der Brücke nicht importieren, oder LibreOffice nimmt innerhalb der Wartezeit keine Verbindung an. Die Meldung sagt, welcher Fall vorliegt |
| `WrongDocumentType` | Das aktuelle Dokument gehört nicht zur Familie des Ziels. Vom Skript wurde nichts ausgeführt |
| `UnknownFamily` | Die Anfrage nannte keine Familie, die die Brücke bedient. Der Hub nennt immer eine, deshalb heißt das nur, dass der Hub und die Brückendatei nicht zusammenpassen |

Die ersten beiden löst die Brücke selbst aus, deshalb zeigt ihre Fehlerspur die eigenen Aufrufe der Brücke und den Pfad
der Brückendatei, nicht Zeilen des Skripts. Zu lesen ist die Meldung.

## Beispiel

Ein Calc-Skript, das das Dokument erstellt, wenn keines geöffnet ist, zwei Zellen schreibt und zurückgibt, was es
geschrieben hat. Es lief unverändert, ohne geöffnetes Dokument, in LibreOffice 24.2.7.2 (die Pakete von Ubuntu 24.04,
mit einem Fenster auf einem virtuellen X-Display), über das echte `libremodular-mcp`; die Antwort unten ist das, was der
Hub in diesem Lauf zurückgegeben hat, mit den Feldern in der Reihenfolge, in der der Hub sie schreibt. Ein weiterer
Lauf, bei dem das Dokument nun geöffnet ist, liefert dieselben Werte.

```python
if doc is None:
    doc = desktop.loadComponentFromURL('private:factory/scalc', '_blank', 0, ())
sheet = doc.Sheets.getByIndex(0)
sheet.getCellByPosition(0, 0).setValue(42)
sheet.getCellByPosition(1, 1).setString('hello')
print('wrote A1 and B2')
result = {'sheet': sheet.Name, 'a1': sheet.getCellByPosition(0, 0).getValue(), 'b2': sheet.getCellByPosition(1, 1).getString()}
```

```json
{
  "app": "libreoffice-calc",
  "duration_ms": 170,
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

Das darauf folgende `get_state` lieferte unter `state`:

```json
{
  "active": {"sheets": [{"last_column": 1, "last_row": 1, "name": "Sheet1"}]},
  "documents": [{"family": "calc", "title": "Untitled 1"}]
}
```

## Grenzen

- **Die Version wird erneut gelesen, solange sie `unknown` ist.** Die Brücke liest sie beim Start und macht einen
  einzigen Versuch, ohne zu warten. Lauscht LibreOffice noch nicht, meldet sie `unknown` und fragt bei jeder neuen
  Verbindung des Hubs und bei jedem `get_state` erneut, bis sie die Version hat; die Warnung, dass sich die
  Schnittstelle nicht importieren lässt, erscheint dabei weiterhin. Der Hub übernimmt die Version aus dem `hello` seiner
  aktuellen Verbindung, deshalb zeigen `get_state` und `get_state("hub")` `unknown` an, bis sich der Hub erneut
  verbindet, obwohl die Brücke die Version dann schon kennt. In einem Testlauf antwortete LibreOffice 1,3 s nach seinem
  Start über UNO; beim allerersten Start in einer anderen Umgebung (kalter Cache) hatte es nach 8 s noch nicht
  geantwortet und nach 14 s schon. Ausprobiert gegen LibreOffice 24.2.7.2: eine Brücke vor LibreOffice gestartet,
  danach LibreOffice gestartet, dann den Zustand erneut abgefragt und den Hub erneut verbunden
  (`cargo test --test libreoffice_real -- --ignored`).
- Nur das **aktuelle** Dokument ist erreichbar. Sind mehrere Dokumente einer Familie geöffnet, erhält ein Skript dasjenige,
  das LibreOffice als aktuell meldet; die anderen stehen in `documents` und werden nicht angeboten. Welches Dokument
  aktuell ist, wenn der Fokus auf einem anderen Fenster liegt (einem Dialog, der Basic-IDE), ist unbestätigt.
- Nur Writer, Calc und Impress. Ein Dokument von Draw, Math oder Base gehört zu keiner Familie: Es wird nicht
  aufgeführt, und ein Skript für jedes Ziel wird mit `WrongDocumentType` abgelehnt, solange es das aktuelle Dokument ist.
- Wird ein sehr großes Writer-Dokument beschrieben, zählt die Brücke seine Absätze einzeln über UNO; die Zeit ist nur
  durch das Zeitlimit des Hubs begrenzt.
- Die Skriptsperre des Hubs ist eine Heuristik gegen Versehen, keine Sandbox. Sie sieht weder die Speicher-, Export- und
  Schließfunktionen von LibreOffice noch einen Pfad, der als Funktionsparameter übergeben wird, noch irgendetwas, das
  ein Programm tut, indem es selbst mit dem UNO-Port spricht.
- Die Brücke bedient jeweils eine Verbindung und jeweils ein Skript für alle drei Ziele zusammen.
- Die Variante der UNO-Verbindung über eine benannte Pipe wird nicht unterstützt.
