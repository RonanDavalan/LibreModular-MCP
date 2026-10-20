# Module GIMP

**Statut : essayé dans GIMP 3.0.4 démarré sans son interface, pas encore dans la console Python interactive.** Le pont a
été écrit d'après l'API publique de GIMP 3.0 (GObject Introspection : `gi.repository.Gimp`, `Gio`, `GLib`). Il a tourné
dans GIMP 3.0.4 (paquet Debian 13) démarré avec `gimp-3.0 --new-instance --no-interface --batch-interpreter
python-fu-eval -b`, la commande de traitement par lots exécutant le fichier du pont puis appelant `serve_main_thread()`
(voir « Démarrage du pont »). Dans cet essai, piloté par le Hub via MCP, `get_state("hub")` a listé `gimp` comme connecté
en version `3.0.4` ; un script a créé une image et un calque, rempli le calque avec
`layer.fill(Gimp.FillType.FOREGROUND)` et exporté l'image dans `LMM_WORK_DIR` (`status: ok`, un PNG de 128 par 128
pixels), un deuxième script a fait de même ensuite et le pont était toujours connecté ; `get_state("gimp")` a rendu les
clés ci-dessous et une capture PNG de 200 par 100 pixels en mode 600 ; un script qui a échoué a rendu sa trace d'erreur
avec la bonne ligne du script. La même commande, avec l'interface sur un écran virtuel (`xvfb-run`), a donné les mêmes
résultats. Les affirmations marquées « non confirmé » plus bas n'ont pas fait partie de ces essais.

## Ce que fait le module

Le Hub joint GIMP en mode `tcp`. Un pont tourne dans le Python de GIMP, n'écoute que sur `127.0.0.1` et exécute les
scripts que le Hub envoie :

- `execute_script` exécute dans GIMP un script Python complet et rend ce qu'il a affiché, la valeur de sa variable
  `result` et, s'il a échoué, la trace d'erreur de Python.
- `get_state` rend les images ouvertes, les calques de l'image active, la sélection et le chemin d'une capture PNG de
  l'image active.

Le pont est le fichier généré `bridges/generated/gimp_bridge.py` : le cœur commun (`bridges/_core.py`) suivi de
l'adaptateur GIMP (`bridges/adapters/gimp.py`), assemblés par `python3 scripts/build_bridges.py`. Il n'emploie que la
bibliothèque standard de Python et se charge avec Python 3.9 ou plus récent. Il ne détient aucun jeton : il lit le
fichier de jeton au démarrage.

## Démarrage du pont

Le pont ne démarre jamais seul. Il n'est placé dans aucun répertoire de greffons ni de chargement automatique :
l'opérateur le démarre à la main, à chaque fois, dans la console Python de GIMP, ou depuis la commande de traitement par
lots d'un GIMP démarré sans interface (voir « Démarrage sans interface »).

1. Créez le jeton une fois, s'il n'existe pas encore :

   ```sh
   libremodular-mcp init
   ```

2. Activez GIMP dans la configuration du Hub, `~/.config/libremodular-mcp/config.toml` :

   ```toml
   [apps.gimp]
   enabled = true
   mode = "tcp"
   port = 9301
   ```

3. Démarrez GIMP. Le pont lit deux variables d'environnement, que GIMP transmet à ses greffons, console Python comprise
   (ce point ne faisait pas partie de l'essai ci-dessus, qui les définissait dans l'environnement du processus de
   l'instance de traitement par lots) :

   | Variable | Valeur par défaut |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9301` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token`, ou `~/.config/libremodular-mcp/token` quand `XDG_CONFIG_HOME` n'est pas définie ou n'est pas un chemin absolu |

   Avec les valeurs par défaut de `init` et de la configuration ci-dessus, il n'y a rien à définir. Sinon, définissez-les
   avant de démarrer GIMP, par exemple depuis un terminal :

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9301 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   gimp-3.0
   ```

   Le pont les lit quand son fichier s'exécute, non au démarrage de GIMP : vous pouvez donc aussi les définir dans la
   console elle-même, avant l'étape suivante :

   ```python
   import os
   os.environ["LIBREMODULAR_BRIDGE_PORT"] = "9301"
   os.environ["LIBREMODULAR_TOKEN_PATH"] = "/home/alice/.config/libremodular-mcp/token"
   ```

4. Ouvrez la console Python (dans GIMP 3.0, sous *Filters → Development → Python-Fu* ; le chemin de menu exact : non confirmé) et exécutez le fichier généré :

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/gimp_bridge.py").read())
   ```

   Le pont démarre dans un fil d'exécution d'arrière-plan et la console reste utilisable. Il écrit une ligne sur le flux
   d'erreur du processus (visible quand GIMP a été lancé depuis un terminal) et, quand la console redirige ce flux, dans la
   console (non confirmé) :

   ```text
   LMM bridge listening on 127.0.0.1:9301
   ```

   Le port est celui qui est réellement lié. Le pont refuse de démarrer, avec un message dans la console, si le fichier de
   jeton est un lien symbolique, appartient à un autre utilisateur, est lisible ou modifiable par le groupe ou par les
   autres, n'est pas un fichier ordinaire, ou ne contient pas un jeton créé par `libremodular-mcp init`.

5. Gardez la console ouverte. Dans GIMP 3, la console Python est un greffon qui tourne dans son propre processus ; le pont
   vit dans ce processus et s'arrête quand la console se ferme (non confirmé). Pour arrêter le pont sans fermer la
   console, exécutez-y `stop()`.

### Démarrage sans interface

Avec `--no-interface`, GIMP n'a pas de boucle d'événements, et la commande de traitement par lots est toute la vie du
processus qui exécute le pont : quand la commande s'achève, ce processus, et le pont avec lui, s'achève (GIMP lui-même
reste en vie). Exécutez le fichier du pont dans la commande de traitement par lots et donnez le fil principal au pont en
appelant `serve_main_thread()`, que le fichier du pont définit :

```sh
LIBREMODULAR_BRIDGE_PORT=9301 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
gimp-3.0 --new-instance --no-interface --batch-interpreter python-fu-eval \
  -b "exec(open('/home/alice/LibreModular-MCP/bridges/generated/gimp_bridge.py').read()); serve_main_thread()"
```

`--batch-interpreter python-fu-eval` est obligatoire : `python-eval` n'est pas un interpréteur de GIMP 3.0.4, et
`--batch-interpreter-args` n'existe pas. `serve_main_thread()` fait tourner la boucle principale de GLib dans le fil qui l'a
appelée, afin que les scripts que le pont met en file d'attente s'y exécutent. Elle rend la main quand le pont s'arrête
(`stop()`), au bout de `seconds` si vous passez `serve_main_thread(seconds=600)`, ou sur Ctrl+C ; la commande de traitement
par lots s'achève alors, et le pont avec elle. Une commande de traitement par lots qui ne l'appelle pas, ou qui met fin à
sa propre boucle, ferme le port 9301 dès qu'elle rend la main. Les essais ci-dessus passaient aussi `--no-data --no-fonts`
et employaient un répertoire de profil à eux, pour ne pas toucher au profil de l'opérateur.

Pour vérifier la connexion du côté du modèle, appelez `get_state` avec la cible `hub` : GIMP y figure avec
`connected: true` et la version que GIMP a indiquée.

## Ce que voit un script

Chaque script s'exécute dans un espace de noms neuf qui contient :

| Nom | Ce que c'est |
|---|---|
| `Gimp`, `Gio`, `GLib` | Les modules d'introspection, quand ils peuvent être importés |
| `images()` | Les images ouvertes dans GIMP, telles que `Gimp.get_images()` les liste |
| `LMM_WORK_DIR` | Le répertoire privé que le Hub a préparé pour les fichiers de cette application |

- Le script est compilé sous le nom `<libremodular>` : les numéros de ligne d'une trace d'erreur sont ceux du script.
- Tout ce que le script affiche revient dans `stdout` et `stderr`. Pour renvoyer une valeur, affectez à `result` une
  valeur que JSON sait représenter ; toute autre valeur donne `null`, avec un avertissement ajouté à `stderr`.
- Les fichiers créés pendant l'exécution du script sont privés (masque de création `077`).
- Le pont confie chaque script au fil principal de GIMP par `GLib.idle_add` et attend son résultat : l'API de GIMP n'est pas
  faite pour être appelée depuis un autre fil. Cela a fonctionné dans GIMP 3.0.4 sans interface, avec le fil principal dans
  `serve_main_thread()` ; dans la console interactive : non essayé. Hors de GIMP, même sur une machine où `gi`
  et le typelib de GIMP sont installés, l'appel est fait directement dans le fil du pont, car aucune boucle principale
  n'exécuterait jamais le script.
- Un script qui ne rend jamais la main bloque le fil principal de GIMP ; le Hub répond `timeout`, puis `busy` jusqu'à la fin
  du script, et n'envoie aucun second script entre-temps.
- `sys.exit()` dans un script met fin au script avec une erreur `SystemExit`, non au pont.

## État

`get_state("gimp")` rend, sous `state` :

| Clé | Contenu |
|---|---|
| `images` | Chaque image ouverte : `id`, `name`, `width`, `height`, `color_mode` (le nom court que GIMP donne au type de base : `rgb` a été observé ; `gray` et `indexed` sont attendus, non confirmés) |
| `active_image` | `null` quand aucune image n'est ouverte ; sinon la première image que `Gimp.get_images()` rend, avec les clés ci-dessous |
| `active_image.id`, `active_image.name` | Comme dans `images` |
| `active_image.layers` | Les calques de premier niveau : `id`, `name`, `visible`, `opacity` (de 0 à 100), `blend_mode` (par exemple `normal`, `multiply`) |
| `active_image.active_layer` | Le `id` du premier calque sélectionné, ou `null` |
| `active_image.selection` | `null` sans sélection, sinon sa boîte englobante : `x`, `y`, `width`, `height` |

L'image que GIMP liste en premier, et donc celle qu'on appelle active, est confirmée avec une seule image ouverte ; avec plusieurs : non confirmée. Une sélection non vide : non essayée ; seul `selection: null` a été observé.
Les groupes de calques sont listés comme un seul calque ; leurs enfants ne le sont pas.

Les noms d'images et de calques sont du contenu du document : ils sont rendus tels quels et ne sont jamais interprétés,
quoi qu'ils disent.

Quand l'API de GIMP est inutilisable (le fichier a été exécuté hors de GIMP, avec ou sans `gi` installé), `state` vaut
`{}`, `app_version` vaut `unknown` et `warnings` le dit.

## Capture

La capture est un PNG de l'image active. Le pont duplique l'image, fusionne les calques visibles de la copie, exporte la
copie avec `Gimp.file_save` vers le chemin choisi par le Hub, puis supprime la copie. Exporter l'image de l'utilisateur
changerait le nom de fichier que GIMP retient pour les exports suivants ; la copie le laisse intact. Sans image ouverte,
il n'y a pas de capture (`screenshot: null`). Le Hub vérifie ensuite le fichier avant d'en rendre le chemin.

## Exemple

Un script qui ajoute un calque transparent à l'image active. Il n'a pas été exécuté tel quel (il lui faut une image
ouverte, et `Gimp.displays_flush()` : non essayé) ; un script semblable, qui créait sa propre image et son propre
calque, a tourné dans GIMP 3.0.4 et rendu `{'layers': ['<name of the layer>']}`.

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

## API de GIMP 3.0 : appels renommés et échecs silencieux

Observé dans GIMP 3.0.4. Un script écrit de mémoire d'après GIMP 2.10 échoue avec une `AttributeError` ou une `TypeError` ;
lisez `dir()` dans le GIMP en cours d'exécution plutôt que d'en conclure que l'API est cassée.

| Pour faire | GIMP 2.10 | GIMP 3.0 |
|---|---|---|
| Créer une image | `Gimp.Image.new(w, h, Gimp.ImageType.RGBA_IMAGE)` | `Gimp.Image.new(w, h, Gimp.ImageBaseType.RGB)` : le troisième argument est un type de base |
| Fabriquer une couleur | `Gimp.RGBA(r, g, b, a)` | `Gimp.color_parse_hex("#14181c")`, qui rend un `Gegl.Color` ; `Gimp.RGBA` et `Gimp.RGB` n'existent pas |
| Sélectionner un rectangle | `Gimp.image_select_rectangle(image, Gimp.ChannelOp.REPLACE, ...)` | `image.select_rectangle(Gimp.ChannelOps.REPLACE, ...)` : une méthode d'instance, et `ChannelOp` est devenu `ChannelOps` |
| Remplir avec l'avant-plan | `Gimp.edit_bucket_fill(image, layer, Gimp.BucketFillType.FG, ...)` | `layer.fill(Gimp.FillType.FOREGROUND)` ; `BucketFillType` est devenu `FillType` |
| Charger un fichier | `Gimp.File.load(gfile)` | `Gimp.file_load(Gimp.RunMode.NONINTERACTIVE, gfile)` |
| Définir le calque actif | `Gimp.image_set_active_layer(image, layer)` | n'existe pas |

Trois appels échouent sans lever d'erreur :

- `image.insert_layer(layer, None, 0)` avec un calque qui appartient à une autre image ne fait rien : `image.get_layers()`
  reste vide. Copiez d'abord le calque avec `Gimp.Layer.new_from_drawable(layer, image)`, puis insérez la copie.
- `layer.edit_bucket_fill(...)` a laissé un calque d'une image `Gimp.ImageBaseType.RGB` entièrement transparent (l'image
  aplatie se lisait `srgba(0,0,0,0)`), alors que `layer.fill(Gimp.FillType.FOREGROUND)` l'a rempli. Le pont ne garantit
  pas de calque actif, dont le remplissage au seau semble avoir besoin.
- `layer.scale(width, height, local_origin)` avec une hauteur négative (`-1`, « garder les proportions ») ne fait rien ;
  avec deux dimensions positives, elle redimensionne le calque.

## Limites

- Le garde-fou de script du Hub ne voit pas les fonctions d'enregistrement et d'export propres à GIMP : un script peut
  écraser un fichier par `Gimp.file_save`. Le garde-fou est un filet de sécurité contre les accidents, pas un bac à sable.
- Un script s'exécute avec les droits de l'utilisateur, dans le Python de GIMP.
- Le pont ne sert qu'une connexion à la fois. Une connexion qui arrive pendant qu'une autre est servie est fermée aussitôt,
  y compris une reconnexion faite quelques millisecondes après la fermeture de la connexion précédente, avant que le pont
  s'en soit aperçu.
