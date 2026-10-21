---
title: LIBREMODULAR-LAUNCH
section: 1
header: LibreModular-MCP
---

# NOMBRE

libremodular-launch - inicia LibreOffice y su puente de LibreModular-MCP

# SINOPSIS

**libremodular-launch libreoffice --bridge-file** *ruta*

**libremodular-launch libreoffice --stop**

**libremodular-launch** {**-h**|**--help**}

# DESCRIPCIÓN

**libremodular-launch libreoffice** inicia LibreOffice a la escucha de UNO en 127.0.0.1, luego el puente que lo conecta con el Hub, y devuelve el control. Los dos procesos siguen ejecutándose cuando el comando
termina. La lista de argumentos de cada uno la fija el programa; el token llega al puente como la ruta de su archivo, nunca como un valor.

Los puertos proceden de los destinos **libreoffice-writer**, **libreoffice-calc** y **libreoffice-impress** activados en la configuración (**uno_port** y **bridge_port**). Un solo LibreOffice y un solo puente
los atienden a todos; los destinos activados deben, por tanto, nombrar los mismos puertos.

El comando se niega a iniciarse cuando ya se ejecuta un LibreOffice del mismo usuario: nunca se conecta a una instancia que pueda contener trabajo sin guardar, y nunca cierra una instancia que no ha iniciado.
Tampoco inicia un segundo puente mientras siga en ejecución el puente de un inicio anterior.

Las demás aplicaciones no tienen lanzador: inicie su puente a mano, como dice su guía. **libremodular-launch gimp**, **krita**, **blender** e **inkscape** responden con ese aviso y terminan con el código de
salida 2.

# OPCIONES

**--bridge-file** *ruta*
:   La ruta absoluta de *uno_bridge.py*. El archivo debe ser un archivo ordinario, no un enlace, pertenecer al usuario actual o a root, y no poder ser escrito por el grupo ni por otros.

    Con el paquete instalado:

    */usr/share/libremodular-mcp/bridges/uno_bridge.py*

    En un clon de las fuentes es *bridges/generated/uno_bridge.py*, cuyo modo 664 hay que cambiar antes, por ejemplo con **chmod 600**.

**--stop**
:   Termina el puente iniciado por un inicio anterior, tras comprobar que el proceso registrado sigue siendo ese puente. LibreOffice sigue abierto: cerrarlo le corresponde a usted.

**-h**, **--help**
:   Muestra el uso y termina.

# ARCHIVOS

*$XDG_RUNTIME_DIR/libremodular-mcp/launch/libreoffice.pids*
:   Los procesos del último inicio, modo 600.

*$XDG_CONFIG_HOME/libremodular-mcp/config.toml*
:   Lo crea **libremodular-mcp init**(1).

*$XDG_CONFIG_HOME/libremodular-mcp/token*
:   Lo crea **libremodular-mcp init**(1).

# CÓDIGOS DE SALIDA

**0**
:   Hecho.

**1**
:   Un fallo: una configuración ilegible, ninguna **XDG_RUNTIME_DIR**, un programa que no se inició.

**2**
:   Un uso incorrecto o una negativa que usted puede resolver: ningún destino de LibreOffice activado, puertos que difieren o son iguales, un archivo de puente en el que no se puede confiar, un LibreOffice o
    un puente que ya se ejecutan.

# EJEMPLOS

Iniciar LibreOffice y su puente desde el paquete instalado y después detener el puente:

```sh
libremodular-launch libreoffice --bridge-file /usr/share/libremodular-mcp/bridges/uno_bridge.py
libremodular-launch libreoffice --stop
```

# VÉASE TAMBIÉN

**libremodular-mcp**(1)
