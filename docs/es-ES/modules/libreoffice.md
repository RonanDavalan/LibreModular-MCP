# Módulo de LibreOffice

**Situación: probado en LibreOffice 24.2.7.2 y 25.2.3.2.** El adaptador y el lanzador se escribieron
a partir de la API pública UNO y de mediciones realizadas en LibreOffice 25.2.3.2; después
se ejecutaron contra LibreOffice 24.2.7.2 (los paquetes de Ubuntu 24.04 `libreoffice-core`, `-writer`,
`-calc`, `-impress` y `python3-uno`, con `/usr/bin/python3` 3.11.15 del sistema) y contra
LibreOffice 25.2.3.2 (los paquetes de Debian 13, con `/usr/bin/python3` 3.13.5 del sistema), de dos
maneras:

- sin interfaz, mediante `cargo test --test libreoffice_real -- --ignored --nocapture`,
  que inicia su propio LibreOffice en un perfil y un puerto desechables, inicia el puente generado
  con el entorno que le da el lanzador y dirige los tres destinos a través
  del Hub (pasa en ambas versiones);
- con una ventana en una pantalla X virtual, iniciada por el `libremodular-launch` real, dirigida por
  el `libremodular-mcp` real mediante MCP por stdio: `get_state("hub")` mostró los tres destinos
  como conectados en modo `uno` con la versión de LibreOffice (`24.2.7.2` y después `25.2.3.2`);
  después de abrir un documento de Calc en la ventana, un script escribió dos celdas, `get_state`
  describió la hoja y devolvió una captura PNG, un script para Writer fue rechazado con
  `WrongDocumentType`, `--stop` detuvo el puente y dejó LibreOffice en ejecución.

Lo que hace un script cuando no hay ningún documento abierto (se ejecuta, con `doc` igual a `None`, y puede
crear el primer documento) se añadió después de esas dos ejecuciones. Se ejecutó solo contra LibreOffice
24.2.7.2, sin interfaz (`cargo test --test libreoffice_real`) y con una ventana en una
pantalla X virtual (el ejemplo al final de esta página). Un puente que se inicia antes de que
LibreOffice escuche, y la versión que entonces comunica, se ejecutaron solo contra 24.2.7.2 sin interfaz:
véase *Límites*.

No formaron parte de esas ejecuciones: una sesión de escritorio real, un documento con varias hojas o páginas (lo que
muestra su captura), documentos de otros tipos abiertos junto a las tres familias, y un LibreOffice
que tarda más en iniciarse de lo que espera el puente (véase la versión en *Límites*). Las
pruebas unitarias del adaptador usan un `uno` falso y se ejecutan igual esté o no instalado LibreOffice.

## Qué hace

El Hub llega a LibreOffice en modo `uno`, a través de **tres destinos que comparten un
puente y una conexión**:

| Destino | Familia | Documentos a los que sirve |
|---|---|---|
| `libreoffice-writer` | `writer` | Documentos de texto |
| `libreoffice-calc` | `calc` | Hojas de cálculo |
| `libreoffice-impress` | `impress` | Presentaciones |

El puente es un proceso independiente de Python, no un complemento: se conecta, en cada solicitud, al puerto
UNO en el que escucha LibreOffice, y ejecuta allí el script. El Hub nunca habla
UNO: habla con el puente en `bridge_port` como con cualquier otro puente, y nombra la familia
del destino en cada solicitud. Un script que se ejecuta en un destino hace que una solicitud en otro
responda `busy`, ya que el puente ejecuta un script a la vez.

- `execute_script` ejecuta un script completo de Python contra el **documento actual** de la
  familia del destino, o con `doc` establecido en `None` cuando LibreOffice no tiene documento actual, y
  devuelve lo que imprimió, el valor de su variable `result` y la traza de error si
  falló.
- `get_state` enumera los documentos abiertos y describe el documento activo, y devuelve la ruta
  de una captura PNG de este.

El puente es el archivo generado `bridges/generated/uno_bridge.py`: el núcleo común
(`bridges/_core.py`) seguido del adaptador (`bridges/adapters/uno.py`), ensamblado por
`python3 scripts/build_bridges.py`. Usa solo la biblioteca estándar de Python, aparte de
`uno`, que importa cuando una solicitud lo necesita; se carga en Python 3.9 o posterior, con o
sin `uno`. No guarda ningún token.

El paquete instala el mismo archivo como `/usr/share/libremodular-mcp/bridges/uno_bridge.py`. Los comandos siguientes nombran el archivo del puente de un clon y, a continuación, el del paquete.

## El límite que debe conocer primero

**El puerto UNO de LibreOffice no tiene autenticación.** Quien pueda conectarse a él puede controlar
LibreOffice: abrir, modificar, guardar y cerrar documentos, y alcanzar todo lo que el Python de LibreOffice
puede alcanzar, con los derechos del usuario. No necesita ningún token, no pasa por ninguna compuerta de scripts
y no deja nada en el diario de auditoría. Solo el puente está protegido por el token, y únicamente
frente al lado del Hub.

- A LibreOffice se le indica que escuche solo en `127.0.0.1`, de modo que nada fuera de la máquina lo alcanza.
  Cualquier otro proceso que pueda abrir una conexión a ese puerto en esta máquina puede hacerlo.
- No está confirmado que LibreOffice filtre las conexiones por usuario: las ejecuciones anteriores se conectaron como
  el mismo usuario. Suponga que no lo hace.
- El puerto existe mientras dure una instancia de LibreOffice iniciada con `--accept`. Cierre esa
  instancia cuando no la necesite; el lanzador nunca lo hace por usted.
- La compuerta de scripts del Hub solo examina los scripts enviados a través de `execute_script`. Un
  programa que se comunica directamente con el puerto UNO queda fuera de su vista.

## Inicio

El Hub nunca inicia LibreOffice ni el puente, y nunca porque el modelo se lo pida.
Ambos se inician con un gesto explícito del operador.

Habilite los destinos en `~/.config/libremodular-mcp/config.toml`. Los tres destinos son un
solo LibreOffice y un solo puente: los destinos habilitados deben nombrar los mismos puertos, o el lanzador
se niega a iniciar. Los valores siguientes son los predeterminados.

```toml
[apps.libreoffice-writer]
enabled = true
mode = "uno"
uno_port = 2002
bridge_port = 9304

[apps.libreoffice-calc]
enabled = true

[apps.libreoffice-impress]
enabled = true
```

`uno_port` es el puerto en el que escucha LibreOffice y que solo usa el lanzador; `bridge_port`
es el puerto del puente, al que se conecta el Hub.

### Con el lanzador

Cree el token una vez, si aún no existe (`libremodular-mcp init`), y después:

```sh
libremodular-launch libreoffice --bridge-file /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
```

Con el paquete instalado:

```sh
libremodular-launch libreoffice --bridge-file /usr/share/libremodular-mcp/bridges/uno_bridge.py
```

Inicia LibreOffice con su ventana, después el puente, y termina. Salida de una ejecución:

```text
LibreOffice started (process 32212), listening for UNO on 127.0.0.1:2002
bridge started (process 32213), listening on 127.0.0.1:9304
a script runs on the current document of its target's family, or with doc set to None when none is open
stop the bridge with 'libremodular-launch libreoffice --stop'; LibreOffice is yours to close
```

Lo que el lanzador hace y no hace:

- LibreOffice se inicia como `/usr/bin/soffice --accept=socket,host=127.0.0.1,port=2002;urp;
  --norestore --nologo --nodefault`, desde una lista fija de argumentos, sin intérprete de comandos y sin
  `--headless`. Recibe el entorno del lanzador, salvo las variables cuyo nombre
  empieza por `LIBREMODULAR_`.
- El puente se inicia como `/usr/bin/python3 <the bridge file>`, con exactamente estas
  variables: `PATH`, `HOME`, `LANG` y `LC_ALL` tal como las tiene el lanzador, `LD_LIBRARY_PATH`
  (`/usr/lib/libreoffice/program`) y `PYTHONPATH` (`/usr/lib/python3/dist-packages`) para
  que el Python del sistema encuentre `uno`, `LIBREMODULAR_UNO_PORT`, `LIBREMODULAR_BRIDGE_PORT` y
  `LIBREMODULAR_TOKEN_PATH`. El propio token no está en ningún argumento ni en ninguna variable: el puente
  lee el archivo.
- Ambos procesos quedan desacoplados, cada uno en su propio grupo, con la entrada y la salida enviadas
  a ninguna parte. Siguen en marcha cuando el lanzador termina.
- El archivo del puente debe ser una ruta absoluta a un archivo normal, no un enlace, que le pertenezca a usted o a root y
  no sea modificable por el grupo ni por otros. Un clon nuevo hecho con una `umask` de `002` (la predeterminada
  de muchas cuentas de Debian) da el modo `664`, que el lanzador rechaza: ejecute
  `chmod 600 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py` una vez. El archivo que instala el
  paquete pertenece a root con el modo `644`: no hay nada que cambiar. El directorio
  que contiene el archivo no se comprueba.
- **Se niega a iniciar nada si ya hay un LibreOffice suyo en ejecución** (un proceso `soffice.bin`
  de su usuario, leído de `/proc`). Nunca se conecta a una instancia que pueda contener
  trabajo sin guardar, y nunca cierra una que no haya iniciado. Cierre LibreOffice y ejecútelo de nuevo,
  o inicie LibreOffice usted mismo como se muestra abajo.
- Se niega a iniciar un segundo puente mientras el puente de un inicio anterior siga en ejecución.
- Los procesos que inició quedan registrados en `$XDG_RUNTIME_DIR/libremodular-mcp/launch/libreoffice.pids`
  (directorio `700`, archivo `600`):

  ```text
  soffice 32212
  bridge 32213 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
  ```

  El primer número es el proceso que inició el lanzador; no es el proceso `soffice.bin`
  que lista el sistema (el número 32240 en esa ejecución).

`libremodular-launch libreoffice --stop` termina el puente, **solo después de comprobar que el
proceso registrado sigue siendo el puente** (uno de sus argumentos es exactamente el archivo del puente): nunca
se envía una señal a un número que otro programa haya ocupado desde entonces. No cierra LibreOffice.
Cerrar una instancia que pueda contener trabajo es su gesto, y el mensaje lo dice.

Códigos de salida: `0` hecho; `2` un uso incorrecto, o una negativa que usted puede corregir (ningún destino activado, puertos
que no coinciden o son iguales, un archivo del puente en el que no se puede confiar, un LibreOffice o un puente
ya en ejecución); `1` un fallo (una configuración ilegible, sin `XDG_RUNTIME_DIR`, un programa
que no se inició). Las demás aplicaciones se inician a mano y responden
`start the bridge by hand, see docs/modules/<name>.md` con el código `2`.

Si el puente no se puede iniciar después de haber iniciado LibreOffice, el lanzador lo dice, registra
solo LibreOffice y lo deja en ejecución.

### A mano

Inicie LibreOffice con la opción `--accept`. El host es el número `127.0.0.1`, nunca un
nombre, que puede resolverse en una dirección en la que LibreOffice no escucha:

```sh
soffice --accept="socket,host=127.0.0.1,port=2002;urp;" --norestore --nologo --nodefault
```

Después, cuando LibreOffice esté en marcha, inicie el puente con el Python del sistema, que tiene `uno`:

```sh
LD_LIBRARY_PATH=/usr/lib/libreoffice/program \
PYTHONPATH=/usr/lib/python3/dist-packages \
LIBREMODULAR_UNO_PORT=2002 \
LIBREMODULAR_BRIDGE_PORT=9304 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
/usr/bin/python3 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
```

Con el paquete instalado:

```sh
LD_LIBRARY_PATH=/usr/lib/libreoffice/program \
PYTHONPATH=/usr/lib/python3/dist-packages \
LIBREMODULAR_UNO_PORT=2002 \
LIBREMODULAR_BRIDGE_PORT=9304 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
/usr/bin/python3 /usr/share/libremodular-mcp/bridges/uno_bridge.py
```

El puente escribe una línea en su flujo de error, `LMM bridge listening on 127.0.0.1:9304`,
y se ejecuta hasta que se detenga el proceso. Se niega a iniciarse si el archivo de token es un enlace,
pertenece a otro usuario, es accesible al grupo o a otros, o no es un token creado por
`libremodular-mcp init`.

### El primer documento

**Un script se ejecuta incluso cuando no hay ningún documento abierto.** Entonces `doc` es `None`, y `uno`, `ctx` y
`desktop` se proporcionan como de costumbre, para que el script pueda crear él mismo el documento:

```python
if doc is None:
    doc = desktop.loadComponentFromURL('private:factory/scalc', '_blank', 0, ())
```

El documento que crea pasa a ser el documento actual de LibreOffice: la siguiente solicitud lo encuentra como
su `doc`. Los nombres de fábrica son `private:factory/swriter`, `private:factory/scalc` y
`private:factory/simpress`. Un script de un destino crea un documento de cualquier familia; la siguiente
solicitud se rechaza con `WrongDocumentType` cuando la familia del destino no es la del
documento actual (véase *Errores*).

Para comprobar la conexión desde el lado del modelo, llame a `get_state` con el destino `hub`: los
tres destinos figuran en la lista con `connected: true` y la versión que LibreOffice comunicó.

## Lo que ve un script

Un script se ejecuta en el puente, que es un proceso separado, en un espacio de nombres nuevo que contiene:

| Nombre | Qué es |
|---|---|
| `uno` | El módulo `uno` |
| `ctx` | El contexto remoto de componentes de LibreOffice |
| `desktop` | `com.sun.star.frame.Desktop`, la aplicación |
| `doc` | El documento actual, de la familia del destino; `None` cuando LibreOffice no tiene documento actual |
| `LMM_WORK_DIR` | El directorio privado que el Hub preparó para los archivos de esta aplicación |

- El script se compila con el nombre `<libremodular>`: los números de línea en una traza de error son las
  líneas del script.
- Todo lo que imprime el script se devuelve en `stdout` y `stderr`. Asigne a `result` un valor
  que JSON pueda representar para devolverlo; cualquier otro valor devuelve `null` con una advertencia
  añadida a `stderr`. Los objetos UNO no son JSON: devuelva valores simples.
- La conexión con LibreOffice se establece para cada solicitud y se reintenta durante un máximo de cinco
  segundos mientras LibreOffice no ha terminado de iniciarse.
- Se comprueba la familia antes de que se ejecute nada del script, cuando hay un documento
  actual. El documento nunca se convierte: un documento de Calc no se ofrece a un script
  de Writer.
- Una excepción lanzada por UNO se informa con su nombre UNO completo, por ejemplo
  `com.sun.star.container.NoSuchElementException`.
- Todo lo que el script hace a través de UNO (guardar, cerrar, exportar) se hace con sus derechos; la
  compuerta de scripts del Hub no ve las funciones de guardar y exportar de la aplicación.

## Estado

`get_state("libreoffice-calc")`, y lo mismo para los otros dos destinos, devuelve, en
`state`:

| Clave | Contenido |
|---|---|
| `documents` | Todos los documentos abiertos de las tres familias, en el orden en que los enumera LibreOffice: `title` y `family` (`writer`, `calc` o `impress`). Los dibujos y los demás componentes no se enumeran |
| `active` | `null`, o la descripción del documento activo, más abajo |

`active` es `null`, con una advertencia, cuando LibreOffice no tiene documento actual o su documento
actual no es de la familia del destino. No se trata de un error. El puente no puede distinguir ambos
casos, por lo que la advertencia nombra los dos.

| Familia | Contenido de `active` |
|---|---|
| `writer` | `paragraphs`: el número de párrafos (las tablas se enumeran junto a ellos y no se cuentan); `pages`: el número de páginas, o `null` para un documento sin controlador |
| `calc` | `sheets`: una lista de `name`, `last_column` y `last_row`, los índices basados en cero de la última celda utilizada, tal como los entrega UNO. Una hoja vacía indica `0` y `0`, como una hoja cuyo único contenido es `A1` |
| `impress` | `slides`: el número de diapositivas |

Los títulos y los nombres de hoja son contenido del documento: se devuelven tal cual y nunca
se interpretan, digan lo que digan.

Cuando el intérprete que ejecuta el puente no puede importar `uno`, `state` es `{}`,
`app_version` es `unknown` y `warnings` lo indica. Lo mismo ocurre cuando LibreOffice no acepta
la conexión, con el motivo en la advertencia.

## Captura

La captura es una exportación del documento activo de la familia, realizada por LibreOffice con su filtro PNG (`writer_png_Export`, `calc_png_Export` o `impress_png_Export`) a la ruta que eligió el Hub. El documento no se guarda, ni se cierra, ni se modifica: después de una captura `hasLocation()` sigue siendo `False` y el documento sigue modificado. Nada en el puente llama a `store`, `storeSelf`, `storeAsURL`, `close`, `dispose` o `terminate`, y las pruebas fallan si lo hace.

- El archivo lo escribe el propio proceso de LibreOffice, con la máscara de creación de ese proceso. El Hub lo comprueba después y lo hace privado (modo `600`) en su propio directorio privado.
- Sin un documento activo de la familia no hay captura (`screenshot: null`).
- No está confirmado lo que muestra el PNG de un documento con varias hojas o páginas.

## Errores

| `error.type` | Cuándo |
|---|---|
| `LibreOfficeUnavailable` | `uno` no se puede importar con el intérprete del puente, o LibreOffice no acepta una conexión dentro de la espera. El mensaje indica cuál |
| `WrongDocumentType` | El documento actual no es de la familia del destino. No se ejecutó nada del script |
| `UnknownFamily` | La solicitud no nombra ninguna familia servida por el puente. El Hub siempre nombra una, por lo que esto solo significa que el Hub y el archivo del puente no coinciden |

Los dos primeros los genera el propio puente, por lo que su traza de error muestra las líneas propias del puente y
la ruta de su archivo, no líneas del script. El mensaje es la parte que debe leer.

## Ejemplo

Un script de Calc que crea el documento cuando no hay ninguno abierto, escribe dos celdas y devuelve lo que
escribió. Se ejecutó tal como está escrito, sin ningún documento abierto, en LibreOffice 24.2.7.2 (los paquetes de Ubuntu 24.04,
con una ventana en una pantalla X virtual), a través del verdadero `libremodular-mcp`; la
respuesta siguiente es lo que devolvió el Hub en esa ejecución, con los campos en el orden en que los escribe
el Hub. Ejecutado de nuevo con el documento ya abierto, devuelve los mismos valores.

```python
if doc is None:
    doc = desktop.loadComponentFromURL('private:factory/scalc', '_blank', 0, ())
sheet = doc.Sheets.getByIndex(0)
sheet.getCellByPosition(0, 0).setValue(42)
sheet.getCellByPosition(1, 1).setString('hello')
print('wrote A1 and B2')
result = {'sheet': sheet.Name, 'a1': sheet.getCellByPosition(0, 0).getValue(), 'b2': sheet.getCellByPosition(1, 1).getString()}
```

```json
{
  "app": "libreoffice-calc",
  "duration_ms": 170,
  "error": null,
  "result": {
    "a1": 42.0,
    "b2": "hello",
    "sheet": "Sheet1"
  },
  "status": "ok",
  "stderr": "",
  "stdout": "wrote A1 and B2\n",
  "truncated": false
}
```

El `get_state` que siguió devolvió, en `state`:

```json
{
  "active": {"sheets": [{"last_column": 1, "last_row": 1, "name": "Sheet1"}]},
  "documents": [{"family": "calc", "title": "Untitled 1"}]
}
```

## Límites

- **La versión se vuelve a leer mientras sea `unknown`.** El puente la lee al iniciarse y
  hace un único intento, sin esperar. Si LibreOffice aún no escucha, indica
  `unknown` y vuelve a consultarla en cada nueva conexión del Hub y en cada `get_state`, hasta tener
  la versión; la advertencia de que la API no se puede importar la acompaña. El Hub conserva la
  versión del `hello` de su conexión actual, por lo que `get_state` y `get_state("hub")`
  muestran `unknown` hasta que el Hub se conecta de nuevo, aunque para entonces el puente ya
  conoce la versión. LibreOffice respondió en UNO 1,3 s después de iniciarse en una ejecución de prueba; en su
  primer inicio en otro entorno (cachés frías) no había respondido tras 8 s y ya lo había hecho
  a los 14 s. Probado con LibreOffice 24.2.7.2: un puente iniciado antes que LibreOffice, luego
  LibreOffice iniciado, luego el estado pedido de nuevo y el Hub conectado de nuevo
  (`cargo test --test libreoffice_real -- --ignored`).
- Solo el documento **actual** es accesible. Con varios documentos de una familia abiertos, un
  script recibe el que LibreOffice indica como actual; los demás aparecen en
  `documents` y no se ofrecen. Qué documento es el actual cuando el foco está en otra
  ventana (un diálogo, el IDE de Basic) no está confirmado.
- Solo Writer, Calc e Impress. Un documento de Draw, Math o Base no es un documento de una familia:
  no se enumera, y un script para cualquier destino se rechaza con `WrongDocumentType` mientras sea
  el actual.
- Describir un documento de Writer muy grande cuenta sus párrafos uno por uno a través de UNO; el
  tiempo solo lo limita el tiempo de espera del Hub.
- La compuerta de scripts del Hub es una heurística contra accidentes, no un entorno aislado. No ve
  las funciones de guardar, exportar y cerrar de LibreOffice, una ruta pasada como parámetro de función
  ni nada que haga un programa al hablar directamente con el puerto UNO.
- El puente atiende una conexión a la vez y un script a la vez para los tres destinos juntos.
- No se admite la variante de la conexión UNO por tubería con nombre.
