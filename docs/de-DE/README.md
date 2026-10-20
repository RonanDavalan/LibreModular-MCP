# LibreModular-MCP

[English](../../README.md) | [Français](../fr-FR/README.md) | **Deutsch** | [Español](../es-ES/README.md)

## Universeller MCP-Server in Rust, mit einem Hub, der nur zwei Werkzeuge anbietet

Ein MCP-Server, mit dem ein Sprachmodell Desktop-Anwendungen steuern kann, indem es Skripte in ihnen ausführt.

## Schnellstart

Bauen Sie den Hub, lassen Sie ihn seine Konfiguration und sein Token anlegen und tragen Sie ihn dann in Ihrem MCP-Client ein.

```sh
cd /home/alice/LibreModular-MCP
cargo build --release --locked
target/release/libremodular-mcp init
```

Der Eintrag für den MCP-Client startet das gebaute Programm mit dem Argument `serve`:

```json
{
  "mcpServers": {
    "libremodular": {
      "command": "/home/alice/LibreModular-MCP/target/release/libremodular-mcp",
      "args": ["serve"]
    }
  }
}
```

Was anzupassen ist: `/home/alice/LibreModular-MCP/` ist der absolute Pfad Ihres Klons. In welche Datei dieser
Eintrag gehört und wie seine Schlüssel heißen, hängt von Ihrem Client ab: Die obige Form ist verbreitet, und der
Hub wurde mit einem für seine Tests geschriebenen Client (JSON-RPC-Zeilen auf Standardeingabe und
Standardausgabe) ausprobiert, nicht mit einem benannten MCP-Client. Der Hub braucht die Variable
`XDG_RUNTIME_DIR`, die eine Desktop-Sitzung setzt; ohne sie weigert er sich zu starten.

`init` legt `~/.config/libremodular-mcp/config.toml` und daneben die Token-Datei an, beide mit Modus 600. Es
überschreibt nie die Konfiguration und ersetzt das Token nur mit `init --rotate`. Danach ist nichts aktiviert:
Setzen Sie `enabled = true` für die gewünschte Anwendung in `config.toml` und starten Sie dann ihre Brücke, wie ihr
Leitfaden es sagt (siehe die Tabelle unten).

## Was es ist

Der Hub ist ein einziges Programm, das dem Modell genau zwei Werkzeuge gibt, `execute_script` und `get_state`.
Das Modell schreibt ein vollständiges Skript; der Hub schickt es an eine Brücke, die es in der Anwendung, neben
ihr oder als einen Prozess je Skript ausführt, und das Modell erhält, was das Skript ausgegeben hat, sein
Ergebnis oder die eigene Fehlerspur der Anwendung zum Korrigieren. Der Hub kennt die Oberfläche keiner Anwendung:
Jede Anwendung hat ein Modul auf der Hub-Seite und eine Brücke auf der Anwendungsseite, und die Brücken sind
Python-Dateien, deren einzige Abhängigkeiten die Standardbibliothek und die eigene Python-Schnittstelle der
Anwendung sind.

## Die zwei Werkzeuge

`execute_script(target_app: str, script_content: str) -> str` führt ein vollständiges Python-Skript in der von
`target_app` benannten Anwendung aus. Es liefert den Text eines JSON-Objekts mit `status` (`ok`, `error`,
`timeout`, `busy`, `refused` oder `unavailable`), `app`, `duration_ms`, `stdout`, `stderr`, `result`, `error`
(`null`, oder `type`, `message` und `traceback`, die der Anwendung), `truncated` und `reason`, wenn der Hub selbst
das Skript abgelehnt hat oder es nicht ausführen konnte.

`get_state(target_app: str) -> dict` liefert `app`, `connected`, `app_version`, `bridge_version`, `captured_at`,
`state` (was für die Anwendung spezifisch ist), `content_origin`, `screenshot` (der Pfad einer PNG-Datei oder
`null`) und `warnings`; die Antwort erreicht den Client als Text eines JSON-Objekts. Das Ziel `hub` liefert die
Liste der konfigurierten Anwendungen, wie jede erreicht wird und ob sie verbunden ist, ohne eine Anwendung zu
kontaktieren.

`target_app` muss der genaue Name einer aktivierten Anwendung sein. Keines der beiden Werkzeuge akzeptiert etwas anderes.

## Anwendungen

Nur Linux: Die Zielplattform ist Debian 13, und das Calc-Beispiel unten lief auch unter Ubuntu 24.04. Noch ist kein
Paket veröffentlicht; der Hub wird aus den Quellen gebaut (Schnellstart). Die Versionen sind die, die die
Leitfäden als ausprobiert nennen; der Leitfaden jeder Anwendung sagt, unter welchen Bedingungen und was nicht Teil
des Laufs war.

| Ziel | Anwendung | Modus | Ausprobierte Version | Leitfaden |
|---|---|---|---|---|
| `gimp` | GIMP | `tcp` | 3.0.4, ohne Oberfläche gestartet | [docs/modules/gimp.md](../modules/gimp.md) |
| `krita` | Krita | `tcp` | 5.2.9, nicht aus Scripter gestartet | [docs/modules/krita.md](../modules/krita.md) |
| `blender` | Blender | `tcp` | 4.3.2 | [docs/modules/blender.md](../modules/blender.md) |
| `inkscape` | Inkscape | `cli` | 1.4 | [docs/modules/inkscape.md](../modules/inkscape.md) |
| `libreoffice-writer`, `libreoffice-calc`, `libreoffice-impress` | LibreOffice | `uno` | 24.2.7.2 und 25.2.3.2 | [docs/modules/libreoffice.md](../modules/libreoffice.md) |

Die Modi: `tcp`, eine in die Anwendung geladene Brücke lauscht an einem lokalen Port; `uno`, eine eigenständige
Brücke verbindet sich mit dem UNO-Port, an dem LibreOffice lauscht; `cli`, ein Prozess führt jedes Skript aus.

In den Modi `tcp` und `uno` startet, schließt oder beendet der Hub nie eine Anwendung auf Wunsch des Modells:
Brücken und Anwendungen starten auf eine ausdrückliche Geste des Bedieners, die in jedem Leitfaden beschrieben
ist, und `libremodular-launch libreoffice` startet LibreOffice und seine Brücke (die anderen Anwendungen werden von
Hand gestartet). Im Modus `cli` startet der Hub einen Python-Prozess je Skript und ruft `inkscape` einmal auf, um
eine Aufnahme zu exportieren; ein vom Bediener geöffnetes Inkscape-Fenster berührt er nie.

## Beispiel

Ein Calc-Skript, das das Dokument anlegt, wenn keines offen ist, zwei Zellen schreibt und zurückgibt, was es
geschrieben hat, und die Antwort des Hubs. Es lief wie geschrieben, ohne offenes Dokument, in LibreOffice 24.2.7.2
(die Pakete von Ubuntu 24.04, mit einem Fenster auf einem virtuellen X-Display), über das echte `libremodular-mcp`.
Dasselbe Beispiel steht in [docs/modules/libreoffice.md](../modules/libreoffice.md).

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

## Was die Maschine verlässt

Alles, was der Hub zurückgibt, geht an das Sprachmodell und damit an den, der es betreibt.

| Was | Verlässt die Maschine |
|---|---|
| Das Skript, das das Modell geschrieben hat | Ja: das Modell ist sein Autor |
| `stdout`, `stderr` und `result` eines Skripts | Ja: sie können den Inhalt eines Dokuments enthalten |
| Die Fehlerspur | Ja: sie enthält Pfade der Maschine |
| `state` | Ja: er enthält Inhalte aus den geöffneten Dokumenten |
| Eine Aufnahme | Nur wenn das Modell die Datei liest |
| Das Audit-Journal, das Token und die Konfiguration | Nie |

Wer in einer verbundenen Anwendung ein vertrauliches Dokument öffnet, weiß, dass sein Inhalt vom Modell gelesen
werden kann.

Der Hub öffnet keine anderen Verbindungen als die zu Brücken auf `127.0.0.1`: Ein Test liest den Quelltext nach
jeder anderen (`cargo test --locked --test invariants`).

## Grenzen

- Der Hub ist keine Sandbox. Ein Skript läuft mit den Rechten des Benutzers, in einer Anwendung, deren Python alles
  tun kann, was der Benutzer kann.
- Die Skriptsperre hält ein ungeschicktes Skript auf, kein entschlossenes. Sie liest den Text eines Skripts vor dem
  Senden; sie sieht weder `runpy`, `pickle`, `http.server`, die Speicherfunktionen der Anwendungen noch einen als
  Parameter an eine Funktion übergebenen Pfad.
- Der UNO-Port von LibreOffice hat keine Authentifizierung: Jeder Prozess, der ihn auf dieser Maschine erreicht,
  steuert LibreOffice ohne Token, Sperre und Audit-Journal.
- Ein anderer Prozess desselben Benutzers kann die Token-Datei lesen.
- In den Modi `tcp` und `uno` hält ein Skript, das in einer Schleife läuft, die Anwendung (bei LibreOffice ihre
  Brücke) beschäftigt, bis sie geschlossen wird: Der Hub antwortet `timeout`, dann `busy`, und hält es nicht an. Nur
  der Modus `cli` beendet ein Skript, das seine Zeit überschreitet.
- `libremodular-launch` lehnt eine Brückendatei ab, die die Gruppe oder andere beschreiben können. Ein Klon mit
  einer `umask` von `002` ergibt Modus 664: Führen Sie einmal `chmod 600` auf der Brückendatei aus. Modus 600 und
  Modus 644 werden akzeptiert.

## Tests

```sh
cargo test --locked
python3 -m unittest discover -s tests/bridges
python3 scripts/build_bridges.py --check
```

Tests, die eine echte Anwendung brauchen, sind mit `#[ignore]` und ihrem Grund markiert und werden gezielt
ausgeführt, zum Beispiel `cargo test --locked --test libreoffice_real -- --ignored --nocapture`.

## Danksagungen

LibreModular-MCP entsteht in einer asymmetrischen Zusammenarbeit von Mensch und LLM. Die Rollen sind so
festgehalten, wie sie tatsächlich ausgeübt wurden; die vollständige Liste steht in
[CONTRIBUTORS.md](../../CONTRIBUTORS.md).

- **Ronan Davalan**: Architekt und Schiedsrichter. Initiative, Prioritäten, Sicherheitsanforderungen, Abnahme und Tests.
- **Claude Code (Anthropic)**: Systemingenieur und Hauptentwickler. Arbeitsrahmen, Durchsicht und Integration des Codes, Dokumentation.
- **Claude Cloud (Anthropic)**: schrieb den Code auf Branches nach Vorgaben, vor jeder Zusammenführung geprüft.
- **Gemini (Google)**: Architekturberater und Synthetisierer; schrieb die Gründungsnotiz und schlug den Namen des Projekts vor.
- **Muse Spark 1.3 (Meta)**: Grafikdesigner; zeichnete das Logo und die Favicons.

## Sicherheit und Lizenz

Melden Sie eine Schwachstelle, wie [SECURITY.md](../../SECURITY.md) es sagt. Der Code steht unter der MIT-Lizenz,
siehe [LICENSE](../../LICENSE).
