---
title: LIBREMODULAR-LAUNCH
section: 1
header: LibreModular-MCP
---

# NAME

libremodular-launch - start LibreOffice and its LibreModular-MCP bridge

# SYNOPSIS

**libremodular-launch libreoffice --bridge-file** *path*

**libremodular-launch libreoffice --stop**

**libremodular-launch** {**-h**|**--help**}

# DESCRIPTION

**libremodular-launch libreoffice** starts LibreOffice listening for UNO on 127.0.0.1, then the bridge that connects it to the Hub, and returns. Both processes keep running after the command returns.
The argument list of each is fixed by the program; the token reaches the bridge as the path of its file, never as a value.

The ports come from the enabled **libreoffice-writer**, **libreoffice-calc** and **libreoffice-impress** targets of the configuration (**uno_port** and **bridge_port**). One LibreOffice and one bridge serve all of
them, so the enabled targets must name the same ports.

The command refuses to start when a LibreOffice of the same user already runs: it never attaches to an instance that may hold unsaved work, and never closes one it did not start. It also refuses to start a second bridge
while the bridge of an earlier launch still runs.

The other applications have no launcher: start their bridge by hand, as their guide says. **libremodular-launch gimp**, **krita**, **blender** and **inkscape** answer with that reminder and exit with 2.

# OPTIONS

**--bridge-file** *path*
:   The absolute path of *uno_bridge.py*. The file must be a regular file, not a link, owned by the current user or by root, and not writable by the group or by others.

    With the Debian package:

    */usr/share/libremodular-mcp/bridges/uno_bridge.py*

    In a clone of the sources it is *bridges/generated/uno_bridge.py*, and a mode of 664 must first be changed, for example with **chmod 600**.

**--stop**
:   End the bridge started by an earlier launch, after checking that the recorded process still is that bridge. LibreOffice is left running: closing it is up to you.

**-h**, **--help**
:   Print the usage and exit.

# FILES

*$XDG_RUNTIME_DIR/libremodular-mcp/launch/libreoffice.pids*
:   The processes of the last launch, mode 600.

*$XDG_CONFIG_HOME/libremodular-mcp/config.toml*
:   Created by **libremodular-mcp init**(1).

*$XDG_CONFIG_HOME/libremodular-mcp/token*
:   Created by **libremodular-mcp init**(1).

# EXIT STATUS

**0**
:   Done.

**1**
:   A failure: an unreadable configuration, no **XDG_RUNTIME_DIR**, a program that did not start.

**2**
:   A wrong usage, or a refusal you can fix: no enabled LibreOffice target, ports that disagree or are equal, a bridge file that cannot be trusted, a LibreOffice or a bridge already running.

# EXAMPLES

Start LibreOffice and its bridge from the Debian package, then stop the bridge:

```sh
libremodular-launch libreoffice --bridge-file /usr/share/libremodular-mcp/bridges/uno_bridge.py
libremodular-launch libreoffice --stop
```

# SEE ALSO

**libremodular-mcp**(1)
