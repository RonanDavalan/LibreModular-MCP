---
title: LIBREMODULAR-MCP
section: 1
header: LibreModular-MCP
---

# NAME

libremodular-mcp – MCP-Server, mit dem ein Sprachmodell Desktop-Anwendungen steuert, indem es Skripte in ihnen ausführt

# ÜBERSICHT

**libremodular-mcp** [**serve**]

**libremodular-mcp init** [**--rotate**]

**libremodular-mcp** {**-h**|**--help**|**-V**|**--version**}

# BESCHREIBUNG

**libremodular-mcp** ist der Hub: ein einziges Programm, das das Model Context Protocol über seine Standardeingabe und Standardausgabe spricht und dem Modell zwei Werkzeuge gibt, **execute_script** und
**get_state**. Der Hub schickt ein Skript an die Brücke der Anwendung, die *target_app* nennt, und liefert zurück, was das Skript ausgegeben hat, sein Ergebnis oder die Fehlerspur der Anwendung selbst. Eine
Brücke ist eine Python-Datei, die in der Anwendung oder neben ihr läuft; der Hub startet keine davon, außer der Brücke von LibreOffice, die **libremodular-launch**(1) auf Anforderung startet.

Ein MCP-Client startet den Hub mit dem Argument **serve**. Keine Anwendung ist aktiviert, solange Sie für sie in der Konfigurationsdatei nicht **enabled = true** setzen.

# BEFEHLE

**serve**
:   Bedient MCP über die Standardeingabe und Standardausgabe. Das ist der Standard, wenn kein Befehl angegeben wird. Der Hub startet nicht, wenn das Token nicht existiert, wenn seine Berechtigungen zu weit
    gefasst sind oder wenn **XDG_RUNTIME_DIR** nicht gesetzt ist.

**init**
:   Legt das Konfigurationsverzeichnis, eine Beispielkonfiguration, in der jede Anwendung deaktiviert ist, und das Token an, das die Brücken prüfen. Einmal ausführen. Der Befehl überschreibt die Konfiguration
    nie und behält ein vorhandenes Token.

# OPTIONEN

**--rotate**
:   Mit **init**: ersetzt das vorhandene Token. Jede laufende Brücke hält noch das bisherige Token und muss neu gestartet werden.

**-h**, **--help**
:   Gibt die Hilfe aus und beendet das Programm.

**-V**, **--version**
:   Gibt *libremodular-mcp* gefolgt von der Version aus und beendet das Programm.

# DATEIEN

*$XDG_CONFIG_HOME/libremodular-mcp/config.toml*
:   Die Konfiguration, Modus 600. Der Standardort ist *~/.config/libremodular-mcp/config.toml*.

*$XDG_CONFIG_HOME/libremodular-mcp/token*
:   Das Token der Brücken, Modus 600. Es wird nie ausgegeben.

*$XDG_STATE_HOME/libremodular-mcp/audit.jsonl*
:   Das Audit-Journal, eine Zeile pro Werkzeugaufruf, Modus 600. Der Standardort ist *~/.local/state/libremodular-mcp/audit.jsonl*.

*$XDG_RUNTIME_DIR/libremodular-mcp/*
:   Flüchtige Dateien: das Arbeitsverzeichnis jedes Skripts und die Aufnahmen, Modus 700.

*/usr/share/libremodular-mcp/bridges/*
:   Die vom Paket installierten Brücken: *gimp_bridge.py*, *krita_bridge.py*, *blender_bridge.py* und *uno_bridge.py*, Eigentum von root und für alle lesbar. Der Leitfaden jeder Anwendung in
    */usr/share/doc/libremodular-mcp/docs/modules/* sagt, wie ihre Brücke gestartet wird.

*/usr/share/doc/libremodular-mcp/*
:   Die README und die Leitfäden, darunter *docs/GUIDE_LLM.md*, den ein Modell liest, bevor es die Werkzeuge benutzt.

# UMGEBUNG

**XDG_RUNTIME_DIR**
:   Wird von **serve** verlangt. Eine Desktop-Sitzung setzt sie.

**XDG_CONFIG_HOME**, **XDG_STATE_HOME**
:   Basisverzeichnisse der Konfiguration und des Journals. Sind sie nicht gesetzt, ergibt **HOME** *~/.config* und *~/.local/state*.

# EXIT-CODES

**0**
:   Erfolg.

**1**
:   Ein Fehlschlag: kein brauchbares Basisverzeichnis, ein fehlendes oder unsicheres Token, eine Datei, die sich nicht schreiben lässt.

**2**
:   Eine falsche Befehlszeile.

# BEISPIELE

Konfiguration und Token anlegen:

```sh
libremodular-mcp init
```

Den Hub in einem MCP-Client eintragen: Der Befehl ist */usr/bin/libremodular-mcp* mit dem Argument **serve**.

# VERFÜGBARKEIT

Die nativen Pakete unterstützen Debian 12 und 13 sowie Ubuntu 22.04, 24.04 und 26.04 (amd64), Fedora 43, 44 und 45 sowie openSUSE Leap 16.0 und 16.1 (x86_64) und Arch Linux (x86_64); ihre Installation und ihre Entfernung wurden in einem Container validiert. Installieren Sie das Paket Ihrer Familie mit einem dieser Befehle:

Debian und Ubuntu:

```sh
sudo apt install ./libremodular-mcp_*_amd64.deb
```

Fedora:

```sh
sudo dnf install ./libremodular-mcp-*.fc*.x86_64.rpm
```

openSUSE Leap:

```sh
sudo zypper install --allow-unsigned-rpm ./libremodular-mcp-*.leap*.x86_64.rpm
```

Arch Linux:

```sh
sudo pacman -U ./libremodular-mcp-*-x86_64.pkg.tar.zst
```

# SIEHE AUCH

**libremodular-launch**(1)
