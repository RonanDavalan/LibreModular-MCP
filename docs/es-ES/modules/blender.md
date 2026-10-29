# Módulo de Blender

**Situación: probado en Blender 4.3.2 (paquete de Debian 13), en ambas formas de iniciarlo.** El puente se escribió a partir de la API pública de Python de Blender (`bpy`). Se ejecutó en Blender 4.3.2 iniciado sin su interfaz (`blender --background --python <boot file>`, con el archivo de arranque ejecutando el puente y cediéndole después el hilo principal, véase «Iniciar el puente») y con su interfaz en una pantalla X virtual (`xvfb-run blender --python <bridge file>`), dirigido por el Hub mediante MCP por stdio. En ambos, `get_state("hub")` mostró `blender` como conectado con la versión `4.3.2`; `get_state("blender")` devolvió la escena predeterminada (`Cube`, `Light`, `Camera`, fotogramas 1 a 250, motor de renderizado `BLENDER_EEVEE_NEXT`, `Cube` seleccionado); el ejemplo de abajo devolvió `{"object": "Generated triangle", "objects": 4}`, tras lo cual la lista de objetos tenía cuatro entradas; un script que lanzó una excepción devolvió su traza de error con la línea correcta; un script que construyó un cubo y lo renderizó a 128 por 128 con `bpy.ops.render.render` devolvió `ok` y escribió el PNG (0,7 s sin la interfaz, 27 s con ella en una pantalla X virtual, donde EEVEE renderiza por software). Una llamada que solo lee o edita la escena tardó 1 ms sin la interfaz y unos 70 ms con ella (el temporizador comprueba cada 50 ms). No formaron parte de esas ejecuciones: un puente iniciado desde la consola del espacio de trabajo *Scripting* de una interfaz que abrió el operador, y una sesión sin pantalla con la interfaz.

## Qué hace

El Hub llega a Blender en modo `tcp`. Un puente se ejecuta dentro del Python de Blender, escucha
solo en `127.0.0.1` y ejecuta los scripts que envía el Hub:

- `execute_script` ejecuta un script de Python completo dentro de Blender y devuelve lo que
  imprimió, el valor de su variable `result` y la traza de error propia de Python si falló.
- `get_state` devuelve la escena activa, sus objetos, los objetos seleccionados, el intervalo de fotogramas
  y el motor de renderizado. Nunca devuelve una captura (véase abajo).

El puente es el archivo generado `bridges/generated/blender_bridge.py`: el núcleo común
(`bridges/_core.py`) seguido del adaptador de Blender (`bridges/adapters/blender.py`),
ensamblado por `python3 scripts/build_bridges.py`. Usa solo la biblioteca estándar de Python
y se carga en Python 3.9 o posterior. No guarda ningún token: lee el archivo de token al iniciarse.

El paquete instala el mismo archivo como `/usr/share/libremodular-mcp/bridges/blender_bridge.py`. Los comandos siguientes nombran el archivo del puente de un clon y, a continuación, el del paquete.

## Iniciar el puente

El puente nunca se inicia solo: el operador lo inicia a mano, cada vez.

1. Cree el token una vez, si aún no existe:

   ```sh
   libremodular-mcp init
   ```

2. Habilite Blender en la configuración del Hub, `~/.config/libremodular-mcp/config.toml`:

   ```toml
   [apps.blender]
   enabled = true
   mode = "tcp"
   port = 9303
   ```

3. Inicie Blender con el puente. El puente lee dos variables de entorno:

   | Variable | Predeterminado |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9303` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token`, o `~/.config/libremodular-mcp/token` cuando `XDG_CONFIG_HOME` no está definida o es relativa |

   Con los valores predeterminados de `init` y de la configuración anterior, no hace falta definir nada.
   Deje que Blender ejecute el archivo al iniciarse, con su interfaz:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9303 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   blender --python /home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py
   ```

   Con el paquete instalado:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9303 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   blender --python /usr/share/libremodular-mcp/bridges/blender_bridge.py
   ```

   o, en un Blender que ya está abierto, ejecútelo desde la consola de Python del
   espacio de trabajo *Scripting*:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py", encoding="utf-8").read())
   ```

   Con el paquete instalado:

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/blender_bridge.py", encoding="utf-8").read())
   ```

   Las variables también pueden definirse en esa consola, antes de la línea anterior, mediante
   `os.environ`: el puente las lee cuando se ejecuta su archivo.

4. El puente escribe una línea en el flujo de error de Blender (el terminal desde el que se inició
   Blender) y, cuando la consola redirige ese flujo, en la consola:

   ```text
   LMM bridge listening on 127.0.0.1:9303
   ```

   El puente se niega a iniciarse si el archivo de token es un enlace simbólico, pertenece a otro
   usuario, es legible o escribible por el grupo o por otros, no es un archivo normal, o no
   contiene un token creado por `libremodular-mcp init`. Para detenerlo, ejecute `stop()` en la
   consola donde se ejecutó el archivo, o cierre Blender.

Iniciado sin interfaz (`blender --background --python ...`), Blender termina en cuanto
se ha ejecutado el archivo, y el puente termina con él. Para mantenerlo activo, ejecute el archivo del puente desde
un archivo de arranque que después ceda el hilo principal al puente llamando a `serve_main_thread()`:

```python
import runpy

runpy.run_path("/home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py")[
    "serve_main_thread"
]()
```

Con el paquete instalado:

```python
import runpy

runpy.run_path("/usr/share/libremodular-mcp/bridges/blender_bridge.py")[
    "serve_main_thread"
]()
```

`serve_main_thread()` regresa cuando el puente se detiene (`stop()`), después de `seconds` si pasa
`serve_main_thread(seconds=600)`, o con Ctrl+C; Blender termina entonces. En ese modo no se ejecuta ningún
temporizador, por lo que esta llamada es la que ejecuta los scripts, en el hilo principal, como lo hace la propia línea de comandos
de Blender. Un archivo de arranque que se limita a esperar, en lugar de llamarla, mantiene el puente activo pero ejecuta
los scripts en el hilo de red del puente, donde `bpy.ops.render.render` puede bloquearse (véase
«Renderizado»).

Para comprobar la conexión desde el lado del modelo, llame a `get_state` con el destino `hub`:
Blender aparece con `connected: true` y la versión que indicó Blender.

## Lo que ve un script

Cada script se ejecuta en un espacio de nombres nuevo que contiene:

| Nombre | Qué es |
|---|---|
| `bpy` | El módulo Python de Blender |
| `LMM_WORK_DIR` | El directorio privado que el Hub preparó para los archivos de esta aplicación |

- El script se compila con el nombre `<libremodular>`: los números de línea de una traza de error son
  las líneas del script.
- Todo lo que imprime el script se devuelve en `stdout` y `stderr`. Asigne a `result` un valor
  que JSON pueda representar para devolverlo; cualquier otro valor devuelve `null` con una advertencia
  añadida a `stderr`.
- Los archivos creados mientras se ejecuta el script son privados (máscara de creación `077`).
- `bpy` debe usarse desde el hilo principal de Blender. Cuando el archivo del puente se ejecuta en el hilo
  principal de Blender con la interfaz, el adaptador registra allí un temporizador persistente
  (`bpy.app.timers.register`), que busca scripts en cola cada 50 milisegundos y
  los ejecuta; el hilo de conexión espera el resultado. En modo en segundo plano, los scripts se ejecutan
  en el hilo principal mientras este se encuentra dentro de `serve_main_thread()` (véase «Iniciar el puente»).
  En caso contrario (el paquete independiente `bpy`, un puente iniciado desde otro hilo o un
  archivo de arranque que nunca llama a `serve_main_thread()`), nada ejecutaría nunca un script en cola,
  por lo que los scripts se llaman directamente en el hilo del puente. Ambas vías se ejecutaron en
  Blender 4.3.2 (véase la situación al principio de esta guía).
- Un script que nunca devuelve el control bloquea el hilo principal de Blender; el Hub responde `timeout`, luego
  `busy` hasta que termina el script, y nunca envía un segundo script mientras tanto.
- Los operadores de Blender (`bpy.ops`) dependen del contexto en el que se ejecutan; desde un temporizador no hay ninguna área
  activa. `bpy.ops.mesh.primitive_cube_add` y `bpy.ops.render.render` se ejecutaron en modo en segundo plano
  (véase «Renderizado»); `bpy.ops.render.render` también se ejecutó con la interfaz en una pantalla X virtual.
  Cualquier otro operador de Blender no se ha probado: la API de datos (`bpy.data`) no depende del
  contexto.

## Versión

`app_version` es la primera palabra de `bpy.app.version_string`, por ejemplo `4.3.2`; una versión de
soporte a largo plazo llamada `4.2.3 LTS` se lee como `4.2.3`. Sin `bpy` es `unknown`.

## Estado

`get_state("blender")` devuelve, en `state`:

| Clave | Contenido |
|---|---|
| `scene` | El nombre de la escena activa (`bpy.context.scene`), o `null` sin escena |
| `objects` | Todos los objetos de la escena: `name`, `type` (por ejemplo `MESH`, `CAMERA`, `LIGHT`) |
| `selected_objects` | Los nombres de los objetos seleccionados; un objeto fuera de la capa de vista activa cuenta como no seleccionado |
| `frame_range` | Los fotogramas `start` y `end` de la escena, o `null` sin escena |
| `render_engine` | El motor de renderizado de la escena, por ejemplo `CYCLES`, o `null` sin escena |

Los nombres de las escenas y de los objetos son contenido del documento: se devuelven tal cual y nunca se interpretan, digan lo que digan.

Cuando `bpy` no se puede importar (el archivo se ejecutó fuera de Blender), `state` es `{}`, `app_version` es `unknown` y `warnings` lo indica.

## Sin captura

`screenshot` es siempre `null` para Blender. Producir una imagen significaría iniciar un renderizado
implícitamente, lo que una solicitud de lectura del estado nunca debe hacer, y no existe una copia segura de
la vista 3D sin la interfaz. El renderizado desde un script se describe más abajo.

## Renderizado

`bpy.ops.render.render(write_still=True)` funciona a través del puente cuando el hilo principal
ejecuta el script: con la interfaz (el temporizador), y en `blender -b` cuando el archivo de arranque
llama a `serve_main_thread()` (véase «Iniciar el puente»). En Blender 4.3.2, un renderizado de 128 por 128 de un cubo
en el motor de renderizado predeterminado (`BLENDER_EEVEE_NEXT`) devolvió `ok` entre 0,6 y
0,9 s cada vez que se probó así (11 ejecuciones, con y sin variable de pantalla, y
bajo carga de CPU), y `get_state("blender")` respondió después. Un archivo de arranque que se limita a esperar
mantiene el script en el hilo de red del puente. Allí el mismo renderizado devolvió
entre 0,6 y 0,9 s en 22 de 23 ejecuciones, y una vez nunca volvió: el Hub respondió `timeout`
tras 30 s, después `busy`. Blender no admite `bpy.ops` fuera de su hilo principal,
por lo que ese hilo puede colgarse en cualquier renderizado; ejecutar el script en el hilo principal elimina la
causa. Fije `filepath` dentro de `LMM_WORK_DIR`.

En un perfil nuevo, el primer renderizado es lento. Cinco perfiles nuevos dieron un primer renderizado de 128 por 128 en
30,0 a 30,7 s, y los siguientes en 0,12 a 0,13 s. El plazo predeterminado del Hub es de 30 s: cuatro de los cinco primeros
renderizados devolvieron `ok`, uno devolvió `timeout`, y las llamadas siguientes recibieron `busy` hasta que terminó.
Suba `default_timeout_s` en la sección `[hub]` de `config.toml`, o haga un renderizado a mano antes de que lo haga un
modelo.

Para un renderizado dirigido por `bpy` y juzgado píxel a píxel, en el motor de renderizado Workbench (medido en
Blender 4.3.2, cada punto es un renderizado que salió mal sin ningún error):

- `scene.display.shading.color_type = "OBJECT"` lee `object.color`, blanco de forma predeterminada:
  todo aparece en blanco en el renderizado. Use `"MATERIAL"`.
- `material.diffuse_color` es lineal. Escribir el valor sRGB de `#c4553a` produce
  `#E39C83`. Convierta cada canal de sRGB a lineal antes de asignar.
- El fondo de un renderizado de Workbench sigue siendo el gris del tema (`srgba(64,64,64,1)`) incluso
  con un color del mundo asignado; `background_type = "WORLD"` no cambia nada, y
  `"CUSTOM"` falla en modo en segundo plano. Renderice sobre una película transparente y componga el
  fondo después (`magick render.png -background "#f4efe8" -alpha remove -alpha off`).
- Un bisel definido después de `obj.scale` queda anulado por la escala. Aplique primero la escala a la malla
  (`bpy.ops.object.transform_apply(scale=True)`) y añada después el bisel.
- Un PNG transparente aparece vacío en un visor que compone sobre blanco: juzgue el renderizado
  sobre la imagen compuesta y lea los colores reales con un histograma
  (`magick render.png +dither -colors 6 -format %c histogram:info:`).

En el motor predeterminado (`BLENDER_EEVEE_NEXT`, medido en Blender 4.3.2), la transformación de vista de una escena nueva
es `AgX`, que cambia los colores de camino al PNG: un color de mundo escrito `#1c385c` se renderizó como `#103759`, y la
respuesta del script fue `ok`. Con `scene.view_settings.view_transform = "Standard"`, el mismo fondo se renderizó como
`#1C385C`. Una superficie iluminada nunca se lee como el color de su material, sea cual sea la transformación.

## Ejemplo

Un script que añade un triángulo a la escena activa con la API de datos. Ejecutado tal cual en
la escena predeterminada, devuelve `{"object": "Generated triangle", "objects": 4}`.

```python
mesh = bpy.data.meshes.new("Generated mesh")
mesh.from_pydata([(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)], [], [(0, 1, 2)])
mesh.update()
item = bpy.data.objects.new("Generated triangle", mesh)
bpy.context.scene.collection.objects.link(item)
result = {"object": item.name, "objects": len(bpy.context.scene.objects)}
```

## Límites

- La compuerta de scripts del Hub no ve las funciones propias de guardado y exportación de Blender: un script
  puede sobrescribir un archivo mediante `bpy.ops.wm.save_as_mainfile`. La compuerta es una red de seguridad
  contra accidentes, no un entorno aislado.
- Un script se ejecuta con los derechos del usuario, dentro del Python de Blender.
- El puente atiende una conexión a la vez.
- Un script no reemplaza el archivo abierto. Durante un script, el puente rechaza `bpy.ops.wm.open_mainfile`,
  `wm.revert_mainfile`, `wm.read_homefile` y `wm.read_factory_settings`: la llamada lanza `RuntimeError`
  (`bpy.ops.wm.open_mainfile cannot run from a bridge script: it ends Blender. Start Blender on the file instead
  (blender -b <file>.blend --python <boot file>).`), el Hub responde `error` y Blender sigue en pie. Sin ese rechazo,
  estos operadores cerraban Blender 4.3.2 (el Hub respondía `unavailable`). Para trabajar sobre otro archivo, inicie
  Blender directamente sobre él (`blender -b scene.blend --python <boot file>`).
- Cycles no puede eliminar el ruido en el paquete de Debian 4.3.2: con `scene.cycles.use_denoising = True`,
  `bpy.ops.render.render` lanzó `Failed to denoise, build has no OpenImageDenoise support`. Déjelo en `False` y suba
  `scene.cycles.samples` (320 muestras a 1360 por 766 píxeles tardaron 23 s en la CPU).
- Un renderizado de Workbench dibuja cada objeto como sólido, diga lo que diga `object.display_type`: `WIRE` no cambió
  nada. Un modificador `WIREFRAME` con `use_replace = True` dibuja las aristas.
