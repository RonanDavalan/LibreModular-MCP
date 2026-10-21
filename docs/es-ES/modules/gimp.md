# Módulo de GIMP

**Situación: probado en GIMP 3.0.4 iniciado sin su interfaz, todavía no en la consola interactiva de Python.** El puente se escribió a partir de la API pública de GIMP 3.0 (GObject Introspection: `gi.repository.Gimp`, `Gio`, `GLib`). Se ejecutó en GIMP 3.0.4 (paquete de Debian 13) iniciado con `gimp-3.0 --new-instance --no-interface --batch-interpreter
python-fu-eval -b`, el comando por lotes que ejecuta el archivo del puente y llama después a `serve_main_thread()` (véase «Iniciar el puente»). En esa ejecución, dirigida por el Hub por MCP, `get_state("hub")` listó `gimp` como conectado con la versión `3.0.4`; un script creó una imagen y una capa, rellenó la capa con `layer.fill(Gimp.FillType.FOREGROUND)` y exportó la imagen a `LMM_WORK_DIR` (`status: ok`, un PNG de 128 por 128 píxeles), un segundo script hizo lo mismo después y el puente seguía conectado; `get_state("gimp")` devolvió las claves de abajo y una captura PNG de 200 por 100 píxeles en modo 600; un script que falló devolvió su traza de error con la línea correcta del script. El mismo comando con la interfaz en una pantalla virtual (`xvfb-run`) dio los mismos resultados. Las afirmaciones marcadas como no confirmadas más abajo no formaron parte de esas ejecuciones.

## Qué hace

El Hub llega a GIMP en modo `tcp`. Un puente se ejecuta dentro del Python de GIMP, escucha solo en
`127.0.0.1` y ejecuta los scripts que envía el Hub:

- `execute_script` ejecuta un script completo de Python dentro de GIMP y devuelve lo que imprimió,
  el valor de su variable `result` y la traza de error propia de Python si falló.
- `get_state` devuelve las imágenes abiertas, las capas de la imagen activa, la selección,
  y la ruta de una captura PNG de la imagen activa.

El puente es el archivo generado `bridges/generated/gimp_bridge.py`: el núcleo común
(`bridges/_core.py`) seguido del adaptador de GIMP (`bridges/adapters/gimp.py`), ensamblado
por `python3 scripts/build_bridges.py`. Usa solo la biblioteca estándar de Python y
se carga en Python 3.9 o posterior. No guarda ningún token: lee el archivo de token al iniciarse.

El paquete instala el mismo archivo como `/usr/share/libremodular-mcp/bridges/gimp_bridge.py`. Los comandos siguientes nombran el archivo del puente de un clon y, a continuación, el del paquete.

## Iniciar el puente

El puente nunca se inicia por sí solo. No se coloca en un directorio de complemento ni de carga automática: el operador lo inicia a mano, cada vez, en la consola de Python de GIMP, o desde el comando por lotes de un GIMP iniciado sin su interfaz (véase «Inicio sin interfaz»).

1. Cree el token una vez, si aún no existe:

   ```sh
   libremodular-mcp init
   ```

2. Active GIMP en la configuración del Hub, `~/.config/libremodular-mcp/config.toml`:

   ```toml
   [apps.gimp]
   enabled = true
   mode = "tcp"
   port = 9301
   ```

3. Inicie GIMP. El puente lee dos variables de entorno, que GIMP transmite a sus complementos, incluida la consola de Python (la ejecución anterior no pasó por ahí: las definió en el entorno del proceso de la instancia por lotes):

   | Variable | Valor predeterminado |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9301` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token`, o `~/.config/libremodular-mcp/token` cuando `XDG_CONFIG_HOME` no está definida o es relativa |

   Con los valores predeterminados de `init` y de la configuración anterior, no es necesario definir nada. En caso contrario, defínalas antes de que se inicie GIMP, por ejemplo desde un terminal:

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9301 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   gimp-3.0
   ```

   El puente las lee cuando se ejecuta su archivo, no cuando se inicia GIMP, por lo que también pueden definirse en la propia consola, antes del paso siguiente:

   ```python
   import os
   os.environ["LIBREMODULAR_BRIDGE_PORT"] = "9301"
   os.environ["LIBREMODULAR_TOKEN_PATH"] = "/home/alice/.config/libremodular-mcp/token"
   ```

4. Abra la consola de Python (en GIMP 3.0, en *Filters → Development → Python-Fu*; la ruta exacta del menú no está confirmada) y ejecute el archivo generado:

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/gimp_bridge.py", encoding="utf-8").read())
   ```

   Con el paquete instalado:

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/gimp_bridge.py", encoding="utf-8").read())
   ```

   El puente se inicia en un hilo en segundo plano y la consola sigue siendo utilizable. Escribe una línea en el flujo de error del proceso (visible cuando GIMP se inició desde un terminal) y, cuando la consola redirige ese flujo, en la consola (no confirmado):

   ```text
   LMM bridge listening on 127.0.0.1:9301
   ```

   El puerto es aquel al que el puente se ha vinculado realmente. El puente se niega a iniciarse, con un mensaje en la consola, si el archivo de token es un enlace simbólico, pertenece a otro usuario, es legible o escribible por el grupo u otros, no es un archivo regular, o no contiene un token creado por `libremodular-mcp init`.

5. Mantenga la consola abierta. En GIMP 3, la consola de Python es un complemento que se ejecuta en su propio proceso; el puente vive en ese proceso y se detiene cuando se cierra la consola (no confirmado). Para detener el puente sin cerrar la consola, ejecute `stop()` en esta.

### Inicio sin interfaz

Con `--no-interface`, GIMP no tiene bucle de eventos y el comando por lotes es toda la vida del proceso que ejecuta el puente: cuando termina el comando, ese proceso, y con él el puente, termina (el propio GIMP sigue en marcha). Ejecute el archivo del puente en el comando por lotes y ceda el hilo principal al puente llamando a `serve_main_thread()`, que el archivo del puente define:

```sh
LIBREMODULAR_BRIDGE_PORT=9301 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
gimp-3.0 --new-instance --no-interface --batch-interpreter python-fu-eval \
  -b "exec(open('/home/alice/LibreModular-MCP/bridges/generated/gimp_bridge.py', encoding='utf-8').read()); serve_main_thread()"
```

Con el paquete instalado:

```sh
LIBREMODULAR_BRIDGE_PORT=9301 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
gimp-3.0 --new-instance --no-interface --batch-interpreter python-fu-eval \
  -b "exec(open('/usr/share/libremodular-mcp/bridges/gimp_bridge.py', encoding='utf-8').read()); serve_main_thread()"
```

`--batch-interpreter python-fu-eval` es obligatorio: `python-eval` no es un intérprete de GIMP 3.0.4 y `--batch-interpreter-args` no existe. `serve_main_thread()` hace girar el bucle principal de GLib en el hilo que lo llamó, para que los scripts que el puente pone en cola se ejecuten allí. Vuelve cuando el puente se detiene (`stop()`), tras `seconds` si usted pasa `serve_main_thread(seconds=600)`, o con Ctrl+C; entonces termina el comando por lotes y con él el puente. Un comando por lotes que no lo llama, o que termina su propio bucle, cierra el puerto 9301 en cuanto vuelve. En las ejecuciones anteriores se pasó además `--no-data --no-fonts` y se usó un directorio de perfil propio, para no tocar el perfil del operador.

Sin su interfaz, GIMP 3.0.4 sigue necesitando una pantalla X a la que pueda acceder: sin pantalla, el mismo comando se detuvo con `Gtk-WARNING: cannot open display` y el código de salida 1, y el puerto 9301 nunca se abrió. En una máquina sin pantalla, déle un servidor X virtual.

Para comprobar la conexión desde el lado del modelo, llame a `get_state` con el destino `hub`: GIMP figura en la lista con `connected: true` y la versión que comunicó GIMP.

## Lo que ve un script

Cada script se ejecuta en un espacio de nombres nuevo que contiene:

| Nombre | Qué es |
|---|---|
| `Gimp`, `Gio`, `GLib` | Los módulos de introspección, cuando se pueden importar |
| `images()` | Las imágenes abiertas en GIMP, tal como las enumera `Gimp.get_images()` |
| `LMM_WORK_DIR` | El directorio privado que el Hub preparó para los archivos de esta aplicación |

- El script se compila con el nombre `<libremodular>`: los números de línea en una traza de error son
  las líneas del script.
- Todo lo que imprime el script se devuelve en `stdout` y `stderr`. Asigne a `result` un valor
  que JSON pueda representar para devolverlo; cualquier otro valor devuelve `null` con una advertencia
  añadida a `stderr`.
- Los archivos creados mientras se ejecuta el script son privados (máscara de creación `077`).
- El puente entrega cada script al hilo principal de GIMP mediante `GLib.idle_add` y espera
  a que termine: la API de GIMP no está pensada para llamarse desde otro hilo. Esto funcionó en GIMP 3.0.4
  sin la interfaz, con el hilo principal dentro de `serve_main_thread()`; en la
  consola interactiva no se ha probado.
  Fuera de GIMP, incluso en una máquina donde `gi` y el typelib de GIMP están instalados, la llamada
  se realiza directamente en el propio hilo del puente, porque ningún bucle principal llegaría a ejecutarla.
- Un script que nunca devuelve el control bloquea el hilo principal de GIMP; el Hub responde `timeout`, luego
  `busy` hasta que termina el script, y mientras tanto nunca envía un segundo script.
- `sys.exit()` en un script termina el script con un error `SystemExit`, no el puente.

## Estado

`get_state("gimp")` devuelve, bajo `state`:

| Clave | Contenido |
|---|---|
| `images` | Cada imagen abierta: `id`, `name`, `width`, `height`, `color_mode` (el nombre corto que GIMP da al tipo base: se observó `rgb`; se esperan `gray` y `indexed`, no confirmado) |
| `active_image` | `null` cuando no hay ninguna imagen abierta; en caso contrario, la primera imagen que devuelve `Gimp.get_images()`, con las claves siguientes |
| `active_image.id`, `active_image.name` | Como en `images` |
| `active_image.layers` | Las capas de nivel superior: `id`, `name`, `visible`, `opacity` (0 a 100), `blend_mode` (por ejemplo `normal`, `multiply`) |
| `active_image.active_layer` | El `id` de la primera capa seleccionada, o `null` |
| `active_image.selection` | `null` sin selección, en caso contrario su cuadro delimitador: `x`, `y`, `width`, `height` |

Qué imagen enumera GIMP primero y, por tanto, a cuál se llama activa, solo está confirmado con una imagen abierta; con varias, no está confirmado. No se probó una selección no vacía: solo se observó `selection: null`. Un grupo de capas se enumera como una sola capa; sus hijas no.

Los nombres de las imágenes y de las capas son contenido del documento: se devuelven tal cual y nunca se interpretan, digan lo que digan.

Cuando no se puede usar la API de GIMP (el archivo se ejecutó fuera de GIMP, con o sin `gi` instalado), `state` es `{}`, `app_version` es `unknown` y `warnings` lo indica.

## Captura

La captura es un PNG de la imagen activa. El puente duplica la imagen, combina las
capas visibles de la copia, exporta la copia con `Gimp.file_save` a la ruta que el Hub
eligió y después elimina la copia. Exportar la propia imagen del usuario cambiaría el nombre de archivo que
GIMP recuerda para las exportaciones posteriores; la copia lo deja intacto. Sin una imagen abierta
no hay captura (`screenshot: null`). El Hub comprueba entonces el archivo antes de devolver
su ruta.

## Ejemplo

Un script que añade una capa transparente a la imagen activa. No se ha ejecutado tal
como está escrito (necesita una imagen abierta y `Gimp.displays_flush()` no se probó); un
script similar que creó su propia imagen y capa se ejecutó en GIMP 3.0.4 y devolvió
`{'layers': ['<name of the layer>']}`.

```python
image = images()[0]
layer = Gimp.Layer.new(
    image,
    "Notes",
    image.get_width(),
    image.get_height(),
    Gimp.ImageType.RGBA_IMAGE,
    100.0,
    Gimp.LayerMode.NORMAL,
)
image.insert_layer(layer, None, 0)
Gimp.displays_flush()
result = {"layer": layer.get_id(), "layers": [item.get_name() for item in image.get_layers()]}
```

## GIMP 3.0 API: llamadas renombradas y fallos silenciosos

Observado en GIMP 3.0.4. Un script escrito de memoria a partir de GIMP 2.10 falla con `AttributeError` o `TypeError`; lea `dir()` en el GIMP en ejecución en lugar de concluir que la API está rota.

| Qué hacer | GIMP 2.10 | GIMP 3.0 |
|---|---|---|
| Crear una imagen | `Gimp.Image.new(w, h, Gimp.ImageType.RGBA_IMAGE)` | `Gimp.Image.new(w, h, Gimp.ImageBaseType.RGB)`: el tercer argumento es un tipo base |
| Crear un color | `Gimp.RGBA(r, g, b, a)` | `Gimp.color_parse_hex("#14181c")`, que devuelve un `Gegl.Color`; `Gimp.RGBA` y `Gimp.RGB` no existen |
| Seleccionar un rectángulo | `Gimp.image_select_rectangle(image, Gimp.ChannelOp.REPLACE, ...)` | `image.select_rectangle(Gimp.ChannelOps.REPLACE, ...)`: un método de instancia, y `ChannelOp` pasó a ser `ChannelOps` |
| Rellenar con el primer plano | `Gimp.edit_bucket_fill(image, layer, Gimp.BucketFillType.FG, ...)` | `layer.fill(Gimp.FillType.FOREGROUND)`; `BucketFillType` pasó a ser `FillType` |
| Cargar un archivo | `Gimp.File.load(gfile)` | `Gimp.file_load(Gimp.RunMode.NONINTERACTIVE, gfile)` |
| Establecer la capa activa | `Gimp.image_set_active_layer(image, layer)` | no existe |

Cuatro llamadas fallan sin lanzar ningún error:

- `image.insert_layer(layer, None, 0)` con una capa que pertenece a otra imagen no hace nada: `image.get_layers()` queda vacía. Cópiela primero con `Gimp.Layer.new_from_drawable(layer, image)` y después inserte la copia.
- `layer.edit_bucket_fill(...)` dejó una capa de una imagen `Gimp.ImageBaseType.RGB` totalmente transparente (la imagen aplanada leía `srgba(0,0,0,0)`), mientras que `layer.fill(Gimp.FillType.FOREGROUND)` la rellenó. El puente no garantiza una capa activa, que el relleno con cubo de pintura parece necesitar.
- `layer.fill(Gimp.FillType.FOREGROUND)` ignora la selección y rellena toda la capa. Con una elipse seleccionada, una capa transparente rellenada así leía alfa 1,0 en su esquina; `layer.edit_fill(Gimp.FillType.FOREGROUND)` se mantuvo dentro de la selección (alfa 0,0 en la esquina, 1,0 en el centro). Para rellenar una selección, use `edit_fill`.
- `layer.scale(width, height, local_origin)` con una altura negativa (`-1`, «mantener la proporción») no hace nada; con dos dimensiones positivas cambia el tamaño de la capa.

## Límites

- La compuerta de scripts del Hub no ve las funciones propias de guardado y exportación de GIMP: un script puede
  sobrescribir un archivo mediante `Gimp.file_save`. La compuerta es una red de seguridad contra accidentes,
  no un entorno aislado.
- Un script se ejecuta con los derechos del usuario, dentro del Python de GIMP.
- El puente atiende una conexión a la vez. Una conexión que llega mientras se atiende otra
  se cierra de inmediato, incluida una reconexión realizada unos milisegundos después de que
  se cerrara la conexión anterior, antes de que el puente lo notara.
