# Módulo de Inkscape

**Situación: probado en Inkscape 1.4 (paquete de Debian 13), dirigido por el Hub mediante MCP por stdio.**
Con `/usr/bin/python3` (3.13.5) y `/usr/share/inkscape/extensions` en `PYTHONPATH`, el ejemplo siguiente se ejecutó y devolvió `{"layers": ["Layer 1", "Notes"]}` en 141 ms; `get_state` enumeró después las dos capas y una captura PNG (RGBA, modo 600); un script que lanzó una excepción devolvió su traza de error con la línea correcta; un segundo script encontró el documento como lo había dejado el primero. Esa ejecución sacó a la luz un defecto, ya corregido: `import inkex` importa numpy, y en una máquina con muchos núcleos su biblioteca BLAS iniciaba un hilo por núcleo, y esos hilos nunca terminaban de iniciar bajo el límite de 2 GiB de espacio de direcciones (la llamada terminó en `timeout` tras 30 s). El módulo fija ahora `OPENBLAS_NUM_THREADS=1` y `OMP_NUM_THREADS=1`.

## Qué hace

El Hub llega a Inkscape en modo `cli`: no hay puente ni proceso de larga duración. Cada `execute_script` inicia un proceso de Python que ejecuta el script y termina; cada `get_state` inicia un proceso de Python que lee el documento de trabajo y después ejecuta `inkscape` una vez para exportar una captura.

El documento de trabajo es `current.svg` en el directorio de trabajo privado del Hub para Inkscape, `<runtime>/libremodular-mcp/work/inkscape/`. El primer script lo crea, privado (modo 600), como un SVG vacío con una capa llamada `Layer 1`. Cada script parte de ese archivo tal como lo dejó el script anterior: nada más persiste entre scripts. Si `current.svg` es un enlace simbólico, pertenece a otro usuario o es accesible al grupo o a otros, el Hub se niega a usarlo y responde `unavailable`.

## Configuración

```toml
[apps.inkscape]
enabled = true
mode = "cli"
```

No hay ningún puerto. Los programas que inicia el módulo quedan fijados cuando se inicia el Hub, y
ni un script ni la configuración pueden cambiarlos:

| Programa | Ruta |
|---|---|
| Intérprete de Python | `/usr/bin/python3` |
| Extensiones de Inkscape, puestas en `PYTHONPATH` para que funcione `import inkex` | `/usr/share/inkscape/extensions` |
| Inkscape, para su versión y para las capturas | `/usr/bin/inkscape` |

En un Debian 13 recién instalado, el paquete `inkscape` no instala `python3-tinycss2`, que `import inkex` necesita: instale ambos con `sudo apt install inkscape python3-tinycss2`. Sin él, un script que importa `inkex` termina con `ModuleNotFoundError: No module named 'tinycss2'`.

## Lo que ve un script

El script se escribe en un archivo privado del directorio de trabajo, nunca se pasa en la
línea de comandos y se elimina cuando termina el proceso. El comando es una lista de argumentos
sin intérprete de comandos: `python3 -c <fixed bootstrap> <script file> <limits>`. El código de arranque
aplica los límites siguientes, y compila después el script con el nombre `<libremodular>`, por lo que
los números de línea de una traza de error son las líneas del script.

El proceso se inicia en el directorio de trabajo, en un grupo de procesos propio, con exactamente
este entorno y nada más del Hub (en particular nunca el token):

| Variable | Valor |
|---|---|
| `PATH`, `HOME`, `LANG`, `LC_ALL` | Los valores del Hub, cuando el Hub los tiene |
| `PYTHONPATH` | `/usr/share/inkscape/extensions` |
| `LMM_DOCUMENT` | La ruta de `current.svg` |
| `LMM_WORK_DIR` | El directorio de trabajo |
| `OPENBLAS_NUM_THREADS`, `OMP_NUM_THREADS` | `1`: numpy, que `inkex` importa, iniciaría de lo contrario un hilo por núcleo |

Los archivos que crea el script siguen la máscara de creación del Hub, `077`: son privados. Un
script que sustituye `current.svg` por un archivo nuevo deja por tanto un archivo privado que el
siguiente script puede usar; uno que escribe un archivo legible por otros hace que el Hub lo rechace.

## Resultado y errores

- Para devolver un valor, un script imprime `LMM_RESULT: ` seguido de JSON como **última línea**
  de su salida estándar. El Hub retira esa línea de `stdout` y devuelve el valor
  como `result`. Sin esa línea, o cuando su JSON no es válido (la línea permanece entonces en
  `stdout`), `result` es `null`. Si la salida supera `max_output_bytes`, el final de la
  salida, y con él la línea de resultado, se pierde, y la respuesta lleva `"truncated": true`.
- Cuando el script lanza una excepción (`SystemExit` incluido), el código de arranque escribe
  `LMM_ERROR: ` seguido de `{"type", "message", "traceback"}` como última línea del
  error estándar. El Hub retira esa línea de `stderr` y la devuelve como `error`, con
  la traza de error propia de Python.
- Un proceso que termina con un código distinto de cero o con una señal sin esa línea (por ejemplo
  terminado por un límite) da `error.type` `ProcessError`, y un mensaje con el código de salida
  o la señal; no hay traza de error.

## Límites

| Límite | Valor |
|---|---|
| Tiempo real transcurrido | `default_timeout_s` del Hub (30 s por defecto). Cuando se agota, todos los procesos del grupo, hijos incluidos, se matan con `SIGKILL` y la respuesta es `timeout`. |
| `RLIMIT_CPU` | El mismo número de segundos |
| `RLIMIT_AS` (espacio de direcciones) | 2 GiB: una asignación mayor falla con `MemoryError` o termina el proceso |
| `RLIMIT_NOFILE` | 256 archivos abiertos |
| `RLIMIT_CORE` | 0: ningún volcado de memoria |
| Salida | `max_output_bytes` por flujo; el resto se lee y se descarta, para que el proceso nunca se bloquee en una tubería llena |

Después de que termina un proceso, todo lo que siga en ejecución en su grupo también se mata, y el Hub responde cuando el grupo ha desaparecido (dos segundos como máximo). Un proceso
que abandona su grupo a propósito (con `setsid`, por ejemplo) no se sigue. Solo se ejecuta un script
a la vez: una segunda solicitud mientras tanto responde `busy`.

## Estado

`get_state("inkscape")` lee `current.svg` en un proceso de Python iniciado con el mismo
entorno y los mismos límites, y devuelve, bajo `state`:

| Clave | Contenido |
|---|---|
| `width`, `height` | Los atributos del elemento raíz, tal como están escritos (por ejemplo `210mm`), o `null` |
| `element_count` | El número de elementos, raíz incluida |
| `layers` | Cada elemento `g` cuyo `inkscape:groupmode` sea `layer`, a cualquier profundidad: `id`, `label` (`inkscape:label`) |

Mientras `current.svg` no exista, `state` es `{}` y una advertencia indica que el documento
aún no existe; consultar el estado nunca lo crea. Un `current.svg` que existe se
lee, se haya ejecutado o no un script desde que se inició el Hub, de modo que se describe un documento dejado por una
ejecución anterior del Hub. Un documento que declara entidades se rechaza: `state` es `{}` y una advertencia
lo indica. Nada del documento se resuelve ni se descarga, y un `DOCTYPE` sin declaraciones de
entidades se lee sin descargar su DTD. El propio Hub no analiza ningún XML.

Los nombres de capa son contenido del documento: se devuelven tal como están y nunca se
interpretan, digan lo que digan. Una capa creada con `inkex.Layer.new` no tiene `id` hasta que el
script le asigne uno, y se enumera con `id: null`.

## Captura

`inkscape --export-type=png --export-filename=<path chosen by the Hub> current.svg`, con
el mismo entorno y su propio grupo de procesos, y 30 s para terminar. Los límites
de recursos de los scripts no se aplican al propio Inkscape. Si falta Inkscape o falla,
`screenshot` es `null` y una advertencia indica el motivo. El Hub comprueba entonces el archivo antes
de devolver su ruta.

## Conexión y versión

`get_state("hub")` muestra Inkscape como conectado cuando `/usr/bin/python3` existe y
`inkscape --version` responde en menos de 5 s; la versión es la palabra que sigue a `Inkscape`
en esa respuesta (por ejemplo `1.2.2`). El módulo no conserva ningún proceso: liberarlo no
hace nada y nunca se toca una ventana de Inkscape que el operador haya abierto.

## Ejemplo

Un script que añade una capa con un rectángulo mediante `inkex`, guarda el documento y
devuelve las etiquetas de las capas. Ejecutado tal cual en un documento nuevo, devuelve
`{"layers": ["Layer 1", "Notes"]}`.

```python
import json
import os

import inkex

document = inkex.load_svg(os.environ["LMM_DOCUMENT"])
svg = document.getroot()
layer = svg.add(inkex.Layer.new("Notes"))
layer.add(inkex.Rectangle.new(10, 10, 80, 40))
document.write(os.environ["LMM_DOCUMENT"])
labels = [node.label for node in svg.descendants().filter(inkex.Layer)]
print("LMM_RESULT: " + json.dumps({"layers": labels}))
```

## `inkex` 1.4: trampas que no lanzan ningún error

Observado en Inkscape 1.4.

- `inkex.Circle.new(a, b)` y `inkex.Ellipse.new(a, b)` toman dos argumentos, y el
  segundo no es el centro: `inkex.Circle.new(64, 64)` da `cx="64.0" cy="0.0" r="64.0"`,
  un círculo pegado al borde superior del documento. Un tercer argumento lanza un `TypeError`.
  Cree el elemento y asigne `cx` y `cy` de forma explícita.
- `inkex.Path` no tiene métodos de segmento (`M`, `L`, `C`, `Z`, `moveTo`, `lineTo`). En su lugar, asigne el
  atributo `d` de un elemento `path`, por ejemplo con `lxml.etree.SubElement`
  y `.set("d", ...)`.
- El documento se conserva entre dos scripts: lo que un script añadió sigue allí para el
  siguiente. Un script que reconstruye todo el dibujo elimina primero los hijos de la raíz
  (`for child in list(svg): svg.remove(child)`).

## Límites de la protección

- La compuerta de scripts del Hub no ve los medios propios de `inkex` para escribir o ejecutar nada. La
  compuerta es una red de seguridad contra accidentes, no un entorno aislado.
- Un script se ejecuta con los derechos del usuario.
