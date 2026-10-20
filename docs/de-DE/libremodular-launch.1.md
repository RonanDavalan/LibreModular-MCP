---
title: LIBREMODULAR-LAUNCH
section: 1
header: LibreModular-MCP
---

# NAME

libremodular-launch – startet LibreOffice und seine LibreModular-MCP-Brücke

# ÜBERSICHT

**libremodular-launch libreoffice --bridge-file** *Pfad*

**libremodular-launch libreoffice --stop**

**libremodular-launch** {**-h**|**--help**}

# BESCHREIBUNG

**libremodular-launch libreoffice** startet LibreOffice mit einem UNO-Port auf 127.0.0.1, dann die Brücke, die es mit dem Hub verbindet, und kehrt zurück. Beide Prozesse laufen weiter, nachdem der Befehl
zurückgekehrt ist. Die Argumentliste jedes Prozesses legt das Programm fest; das Token erreicht die Brücke als Pfad seiner Datei, nie als Wert.

Die Ports stammen aus den aktivierten Zielen **libreoffice-writer**, **libreoffice-calc** und **libreoffice-impress** der Konfiguration (**uno_port** und **bridge_port**). Ein LibreOffice und eine Brücke
bedienen alle; die aktivierten Ziele müssen deshalb dieselben Ports nennen.

Der Befehl startet nicht, wenn bereits ein LibreOffice desselben Benutzers läuft: Er verbindet sich nie mit einer Instanz, die ungespeicherte Arbeit enthalten kann, und schließt nie eine Instanz, die er nicht
gestartet hat. Er startet auch keine zweite Brücke, solange die Brücke eines früheren Starts noch läuft.

Die übrigen Anwendungen haben keinen Starter: Starten Sie ihre Brücke von Hand, wie ihr Leitfaden es sagt. **libremodular-launch gimp**, **krita**, **blender** und **inkscape** antworten mit diesem Hinweis und
enden mit dem Exit-Code 2.

# OPTIONEN

**--bridge-file** *Pfad*
:   Der absolute Pfad von *uno_bridge.py*. Die Datei muss eine gewöhnliche Datei sein, kein Link, dem aktuellen Benutzer oder root gehören und weder für die Gruppe noch für andere schreibbar sein.

    Beim Debian-Paket:

    */usr/share/libremodular-mcp/bridges/uno_bridge.py*

    In einem Klon der Quellen ist es *bridges/generated/uno_bridge.py*, dessen Modus 664 zuerst geändert werden muss, zum Beispiel mit **chmod 600**.

**--stop**
:   Beendet die Brücke, die ein früherer Start gestartet hat, nachdem geprüft wurde, dass der aufgezeichnete Prozess noch diese Brücke ist. LibreOffice läuft weiter: Es zu schließen ist Ihre Sache.

**-h**, **--help**
:   Gibt die Verwendung aus und beendet das Programm.

# DATEIEN

*$XDG_RUNTIME_DIR/libremodular-mcp/launch/libreoffice.pids*
:   Die Prozesse des letzten Starts, Modus 600.

*$XDG_CONFIG_HOME/libremodular-mcp/config.toml*
:   Von **libremodular-mcp init**(1) angelegt.

*$XDG_CONFIG_HOME/libremodular-mcp/token*
:   Von **libremodular-mcp init**(1) angelegt.

# EXIT-CODES

**0**
:   Erledigt.

**1**
:   Ein Fehlschlag: eine nicht lesbare Konfiguration, kein **XDG_RUNTIME_DIR**, ein Programm, das nicht gestartet ist.

**2**
:   Eine falsche Verwendung oder eine Ablehnung, die Sie beheben können: kein aktiviertes LibreOffice-Ziel, Ports, die voneinander abweichen oder gleich sind, eine Brückendatei, der man nicht trauen kann, ein
    bereits laufendes LibreOffice oder eine bereits laufende Brücke.

# BEISPIELE

LibreOffice und seine Brücke aus dem Debian-Paket starten und dann die Brücke beenden:

```sh
libremodular-launch libreoffice --bridge-file /usr/share/libremodular-mcp/bridges/uno_bridge.py
libremodular-launch libreoffice --stop
```

# SIEHE AUCH

**libremodular-mcp**(1)
