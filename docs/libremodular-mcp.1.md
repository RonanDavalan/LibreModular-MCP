---
title: LIBREMODULAR-MCP
section: 1
header: LibreModular-MCP
---

# NAME

libremodular-mcp - MCP server that lets a language model drive desktop applications by running scripts in them

# SYNOPSIS

**libremodular-mcp** [**serve**]

**libremodular-mcp init** [**--rotate**]

**libremodular-mcp** {**-h**|**--help**|**-V**|**--version**}

# DESCRIPTION

**libremodular-mcp** is the Hub: one binary that speaks the Model Context Protocol on its standard input and standard output, and gives the model two tools, **execute_script** and **get_state**. The
Hub sends a script to the bridge of the application named by *target_app*, and returns what the script printed, its result, or the application's own error trace. A bridge is a Python file
that runs inside the application or next to it; the Hub starts none of them, apart from the LibreOffice bridge that **libremodular-launch**(1) starts on request.

An MCP client starts the Hub with the argument **serve**. No application is enabled until you set **enabled = true** for it in the configuration file.

# COMMANDS

**serve**
:   Serve MCP on standard input and output. This is the default when no command is given. The Hub refuses to start when the token does not exist, when its permissions are too wide, or when
    **XDG_RUNTIME_DIR** is not set.

**init**
:   Create the configuration directory, an example configuration with every application disabled, and the token the bridges check. Run it once. It never overwrites the configuration, and it keeps an
    existing token.

# OPTIONS

**--rotate**
:   With **init**: replace the existing token. Every running bridge still holds the previous token and must be restarted.

**-h**, **--help**
:   Print the help and exit.

**-V**, **--version**
:   Print *libremodular-mcp* followed by the version, and exit.

# FILES

*$XDG_CONFIG_HOME/libremodular-mcp/config.toml*
:   The configuration, mode 600. The default location is *~/.config/libremodular-mcp/config.toml*.

*$XDG_CONFIG_HOME/libremodular-mcp/token*
:   The token of the bridges, mode 600. It is never printed.

*$XDG_STATE_HOME/libremodular-mcp/audit.jsonl*
:   The audit journal, one line per tool call, mode 600. The default location is *~/.local/state/libremodular-mcp/audit.jsonl*.

*$XDG_RUNTIME_DIR/libremodular-mcp/*
:   Volatile files: the working directory of each script and the screenshots, mode 700.

*/usr/share/libremodular-mcp/bridges/*
:   The bridges installed by the Debian package: *gimp_bridge.py*, *krita_bridge.py*, *blender_bridge.py* and *uno_bridge.py*, owned by root and readable by everyone. The guide of each application,
    in */usr/share/doc/libremodular-mcp/docs/modules/*, says how to start its bridge.

*/usr/share/doc/libremodular-mcp/*
:   The README and the guides, including *docs/GUIDE_LLM.md*, which a model reads before using the tools.

# ENVIRONMENT

**XDG_RUNTIME_DIR**
:   Required by **serve**. A desktop session sets it.

**XDG_CONFIG_HOME**, **XDG_STATE_HOME**
:   Base directories of the configuration and of the journal. When they are not set, **HOME** gives *~/.config* and *~/.local/state*.

# EXIT STATUS

**0**
:   Success.

**1**
:   A failure: no usable base directory, a missing or unsafe token, a file that cannot be written.

**2**
:   A wrong command line.

# EXAMPLES

Create the configuration and the token:

```sh
libremodular-mcp init
```

Declare the Hub in an MCP client: the command is */usr/bin/libremodular-mcp* with the argument **serve**.

# AVAILABILITY

The Debian package supports Debian 12 and 13 and Ubuntu 22.04, 24.04 and 26.04, on amd64; its installation and its removal were validated in a container. Install it with:

```sh
sudo apt install ./libremodular-mcp_*_amd64.deb
```

# SEE ALSO

**libremodular-launch**(1)
