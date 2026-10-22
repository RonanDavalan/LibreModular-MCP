---
title: LIBREMODULAR-MCP
section: 1
header: LibreModular-MCP
---

# NOMBRE

libremodular-mcp - servidor MCP que permite a un modelo de lenguaje manejar aplicaciones de escritorio ejecutando scripts en ellas

# SINOPSIS

**libremodular-mcp** [**serve**]

**libremodular-mcp init** [**--rotate**]

**libremodular-mcp** {**-h**|**--help**|**-V**|**--version**}

# DESCRIPCIÓN

**libremodular-mcp** es el Hub: un único binario que habla el Model Context Protocol por su entrada y su salida estándar, y da al modelo dos herramientas, **execute_script** y **get_state**. El Hub envía un
script al puente de la aplicación que nombra *target_app*, y devuelve lo que el script ha impreso, su resultado o la traza de error de la propia aplicación. Un puente es un archivo Python que se ejecuta dentro
de la aplicación o a su lado; el Hub no inicia ninguno, salvo el puente de LibreOffice, que **libremodular-launch**(1) inicia a petición.

Un cliente MCP inicia el Hub con el argumento **serve**. Ninguna aplicación está activada mientras usted no ponga **enabled = true** para ella en el archivo de configuración.

# COMANDOS

**serve**
:   Sirve MCP por la entrada y la salida estándar. Es el valor por defecto cuando no se da ningún comando. El Hub se niega a iniciarse cuando el token no existe, cuando sus permisos son demasiado amplios o
    cuando **XDG_RUNTIME_DIR** no está definida.

**init**
:   Crea el directorio de configuración, una configuración de ejemplo con todas las aplicaciones desactivadas y el token que los puentes comprueban. Se ejecuta una sola vez. Nunca sobrescribe la
    configuración y conserva un token existente.

# OPCIONES

**--rotate**
:   Con **init**: sustituye el token existente. Cada puente en ejecución conserva el token anterior y debe reiniciarse.

**-h**, **--help**
:   Muestra la ayuda y termina.

**-V**, **--version**
:   Muestra *libremodular-mcp* seguido de la versión y termina.

# ARCHIVOS

*$XDG_CONFIG_HOME/libremodular-mcp/config.toml*
:   La configuración, modo 600. La ubicación por defecto es *~/.config/libremodular-mcp/config.toml*.

*$XDG_CONFIG_HOME/libremodular-mcp/token*
:   El token de los puentes, modo 600. Nunca se muestra.

*$XDG_STATE_HOME/libremodular-mcp/audit.jsonl*
:   El diario de auditoría, una línea por llamada a una herramienta, modo 600. La ubicación por defecto es *~/.local/state/libremodular-mcp/audit.jsonl*.

*$XDG_RUNTIME_DIR/libremodular-mcp/*
:   Archivos volátiles: el directorio de trabajo de cada script y las capturas, modo 700.

*/usr/share/libremodular-mcp/bridges/*
:   Los puentes que instala el paquete: *gimp_bridge.py*, *krita_bridge.py*, *blender_bridge.py* y *uno_bridge.py*, propiedad de root y legibles por todos. La guía de cada aplicación, en
    */usr/share/doc/libremodular-mcp/docs/modules/*, dice cómo iniciar su puente.

*/usr/share/doc/libremodular-mcp/*
:   El README y las guías, entre ellas *docs/GUIDE_LLM.md*, que un modelo lee antes de usar las herramientas.

# ENTORNO

**XDG_RUNTIME_DIR**
:   La exige **serve**. Una sesión de escritorio la define.

**XDG_CONFIG_HOME**, **XDG_STATE_HOME**
:   Directorios base de la configuración y del diario. Cuando no están definidas, **HOME** da *~/.config* y *~/.local/state*.

# CÓDIGOS DE SALIDA

**0**
:   Éxito.

**1**
:   Un fallo: ningún directorio base utilizable, un token ausente o no seguro, un archivo que no se puede escribir.

**2**
:   Una línea de comandos incorrecta.

# EJEMPLOS

Crear la configuración y el token:

```sh
libremodular-mcp init
```

Declarar el Hub en un cliente MCP: el comando es */usr/bin/libremodular-mcp* con el argumento **serve**.

# DISPONIBILIDAD

Los paquetes nativos admiten Debian 12 y 13 y Ubuntu 22.04, 24.04 y 26.04 (amd64), Fedora 43, 44 y 45 y openSUSE Leap 16.0 y 16.1 (x86_64), y Arch Linux (x86_64); su instalación y su desinstalación se han validado en un contenedor. Instale el paquete de su familia con uno de estos comandos:

Debian y Ubuntu:

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

Para Windows 11 (x64) hay un instalador, construido en Windows y probado sin pantalla, todavía no probado en la máquina de un usuario; se entrega en el archivo *libremodular-mcp-0.1.0-windows.zip* e instala el Hub sin derechos de administrador. No instala estas páginas man. En PowerShell:

```powershell
tar -xf libremodular-mcp-0.1.0-windows.zip
.\libremodular-mcp-0.1.0-setup.exe /S
```

# VÉASE TAMBIÉN

**libremodular-launch**(1)
