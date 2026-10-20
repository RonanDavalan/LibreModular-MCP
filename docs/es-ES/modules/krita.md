# Módulo de Krita

**Situación: probado en Krita 5.2.9 (paquete de Debian 13) en una pantalla X virtual, aún no desde Scripter.** El puente se escribió a partir de la API pública de Python de Krita 5 (el módulo `krita` y su clase `Krita`). Se ejecutó en Krita 5.2.9 iniciado bajo `xvfb-run`, con un perfil desechable en el que una extensión de arranque de una línea ejecutaba el archivo generado en el hilo de la interfaz (como hace Scripter), dirigido por el Hub mediante MCP por stdio. `get_state("hub")` enumeró `krita` como conectado con la versión `5.2.9`; sin documento, `get_state("krita")` devolvió `document: null`; un script creó un documento de 300 por 200 y lo abrió en la ventana; el estado enumeró entonces el documento (`RGBA`, `U8`, perfil `sRGB-elle-V2-srgbtrc.icc`), su capa `Background` y una captura PNG de 300 por 200 píxeles (RGBA, modo 600); el ejemplo siguiente añadió una capa y el estado siguiente enumeró las dos; un script que lanzó una excepción devolvió su traza de error con la línea correcta. Una llamada tardó entre 1 y 7 ms una vez en marcha. No formó parte de esa ejecución: el arranque del puente desde el propio Scripter (la ruta del menú, las variables de entorno que le llegan, su vida útil cuando se cierra Scripter).

## Qué hace

El Hub llega a Krita en modo `tcp`. Un puente se ejecuta dentro del Python de Krita, escucha solo en
`127.0.0.1` y ejecuta los scripts que envía el Hub:

- `execute_script` ejecuta un script de Python completo dentro de Krita y devuelve lo que imprimió,
  el valor de su variable `result` y la traza de error propia de Python si falló.
- `get_state` devuelve el documento activo, sus capas y la capa activa, y la ruta
  de una captura PNG del documento.

El puente es el archivo generado `bridges/generated/krita_bridge.py`: el núcleo común
(`bridges/_core.py`) seguido del adaptador de Krita (`bridges/adapters/krita.py`),
ensamblado por `python3 scripts/build_bridges.py`. Usa solo la biblioteca estándar de Python
y se carga en Python 3.9 o posterior. No contiene ningún token: lee el archivo de token al iniciarse.

El paquete Debian instala el mismo archivo como `/usr/share/libremodular-mcp/bridges/krita_bridge.py`. Los comandos siguientes nombran el archivo del puente de un clon y, a continuación, el del paquete.

## Iniciar el puente

El puente nunca se inicia solo. No se instala nada en el directorio de complementos de Python de Krita, que puede contener extensiones del propio usuario: ni el código de este proyecto ni este procedimiento escriben allí. El operador ejecuta el archivo generado a mano, cada vez, desde el Scripter de Krita.

1. Cree el token una vez, si aún no existe:

   ```sh
   libremodular-mcp init
   ```

2. Habilite Krita en la configuración del Hub, `~/.config/libremodular-mcp/config.toml`:

   ```toml
   [apps.krita]
   enabled = true
   mode = "tcp"
   port = 9302
   ```

3. Inicie Krita. El puente lee dos variables de entorno:

   | Variable | Predeterminado |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9302` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token`, o `~/.config/libremodular-mcp/token` cuando `XDG_CONFIG_HOME` no está definida o es relativa |

   Con los valores predeterminados de `init` y de la configuración anterior, no hace falta definir nada.
   En caso contrario, defínalas antes de que se inicie Krita, por ejemplo desde un terminal:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9302 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   krita
   ```

   El puente las lee cuando se ejecuta su archivo, por lo que también se pueden definir en Scripter, antes del paso siguiente, mediante `os.environ`.

4. Abra Scripter (en Krita 5, en *Tools → Scripts → Scripter*; la ruta exacta del menú no está confirmada) y ejecute:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/krita_bridge.py").read())
   ```

   Con el paquete Debian:

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/krita_bridge.py").read())
   ```

   El puente se inicia en un hilo en segundo plano y Krita sigue siendo utilizable. Escribe una línea en el flujo de error del proceso y, cuando Scripter redirige ese flujo, en la salida de Scripter:

   ```text
   LMM bridge listening on 127.0.0.1:9302
   ```

   El puente se niega a iniciarse si el archivo del token es un enlace simbólico, pertenece a otro usuario, es legible o escribible por el grupo o por otros, no es un archivo regular o no contiene un token creado por `libremodular-mcp init`. Funciona hasta que Krita se cierra; para detenerlo antes, ejecute `stop()` en Scripter.

Para comprobar la conexión desde el lado del modelo, llame a `get_state` con el destino `hub`: Krita figura en la lista con `connected: true` y la versión que Krita comunicó.

## Lo que ve un script

Cada script se ejecuta en un espacio de nombres nuevo que contiene:

| Nombre | Qué es |
|---|---|
| `Krita` | La clase de la API de Python de Krita; `Krita.instance()` es la aplicación |
| `LMM_WORK_DIR` | El directorio privado que el Hub preparó para los archivos de esta aplicación |

- El script se compila con el nombre `<libremodular>`: los números de línea de una traza de error son
  las líneas del script.
- Todo lo que imprime el script se devuelve en `stdout` y `stderr`. Asigne a `result` un valor
  que JSON pueda representar para devolverlo; cualquier otro valor devuelve `null` con una advertencia
  añadida a `stderr`.
- Los archivos creados mientras se ejecuta el script son privados (máscara de creación `077`).
- La API de Krita debe usarse desde el hilo de su interfaz. Cuando el puente se inicia dentro de
  Krita (la instancia `Krita` existe y una aplicación Qt está en ejecución), el adaptador crea
  un pequeño objeto Qt que vive en ese hilo; cada script se le entrega mediante una
  señal en cola, y el hilo de conexión espera el resultado. No se usa un `QTimer` iniciado desde
  el hilo de conexión: ese hilo no tiene bucle de eventos Qt, por lo que el temporizador nunca
  llegaría a dispararse. Fuera de Krita (sin módulo `krita`, o con PyQt instalado sin una aplicación Qt
  en ejecución), los scripts se llaman directamente. La ruta de señal en cola se ejecutó en Krita 5.2.9
  (véase la situación al principio de esta guía).
- Un script que nunca devuelve el control bloquea la interfaz de Krita; el Hub responde `timeout`, luego
  `busy` hasta que termina el script, y nunca envía un segundo script mientras tanto.

## Estado

`get_state("krita")` devuelve, en `state`:

| Clave | Contenido |
|---|---|
| `document` | `null` sin documento activo; en caso contrario, las claves siguientes |
| `document.name` | El nombre del documento |
| `document.width`, `document.height` | Su tamaño en píxeles |
| `document.color_model`, `document.color_depth`, `document.color_profile` | Como los nombra Krita, por ejemplo `RGBA`, `U8`, `sRGB-elle-V2-srgbtrc.icc` |
| `document.layers` | Las capas de nivel superior: `id` (identificador único de Krita), `name`, `visible`, `opacity` (de 0 a 255, como lo indica Krita), `type` (por ejemplo `paintlayer`, `grouplayer`, `vectorlayer`) |
| `document.active_layer` | El `id` de la capa activa, o `null` |

Un grupo de capas se enumera como una sola capa; sus hijas no.

Los nombres de los documentos y de las capas son contenido del documento: se devuelven tal cual y nunca se interpretan, digan lo que digan.

Cuando no se puede usar la API de Krita (el archivo se ejecutó fuera de Krita), `state` es `{}`, `app_version` es `unknown` y `warnings` lo indica.

## Captura

La captura es la proyección del documento activo: la imagen combinada que muestra Krita, leída con `Document.projection` después de `Document.waitForDone`, y guardada como PNG en la ruta que eligió el Hub. La proyección es una copia; el propio documento no se guarda, no se exporta ni se modifica. Sin un documento activo no hay captura (`screenshot: null`). El Hub comprueba después el archivo antes de devolver su ruta.

## Ejemplo

Un script que añade una capa de pintura vacía al documento activo. Ejecutado tal cual en un
documento nuevo que solo tenía su capa `Background`, devuelve `{"layers": ["Background", "Notes"]}`.

```python
document = Krita.instance().activeDocument()
layer = document.createNode("Notes", "paintlayer")
document.rootNode().addChildNode(layer, None)
document.refreshProjection()
result = {"layers": [node.name() for node in document.topLevelNodes()]}
```

## Límites

- La compuerta de scripts del Hub no ve las funciones propias de guardado y exportación de Krita: un script puede
  sobrescribir un archivo mediante `Document.saveAs` o `Document.exportImage`. La compuerta es una
  red de seguridad contra accidentes, no un entorno aislado.
- Un script se ejecuta con los derechos del usuario, dentro del Python de Krita.
- El puente atiende una conexión a la vez.
