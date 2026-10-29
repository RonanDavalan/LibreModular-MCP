# Module Blender

**Statut : essayé dans Blender 4.3.2 (paquet Debian 13), des deux façons de le démarrer.** Le pont a été écrit d'après
l'API Python publique de Blender (`bpy`). Il a tourné dans Blender 4.3.2 démarré sans son interface
(`blender --background --python <boot file>`, le fichier de démarrage exécutant le pont puis lui confiant le fil
principal, voir « Démarrage du pont ») et avec son interface sur un affichage X virtuel
(`xvfb-run blender --python <bridge file>`), le Hub le pilotant par MCP sur stdio. Dans les deux cas,
`get_state("hub")` a listé `blender` comme connecté en version `4.3.2` ; `get_state("blender")` a rendu la scène par
défaut (`Cube`, `Light`, `Camera`, images d'animation 1 à 250, moteur `BLENDER_EEVEE_NEXT`, `Cube` sélectionné) ;
l'exemple ci-dessous a rendu `{"object": "Generated triangle", "objects": 4}`, après quoi la liste des objets comptait
quatre entrées ; un script qui a levé une exception a rendu sa trace d'erreur avec la bonne ligne ; un script qui a
construit un cube et l'a rendu en 128 par 128 avec `bpy.ops.render.render` a rendu `ok` et écrit le PNG (0,7 s sans
interface, 27 s avec l'interface sur un affichage virtuel, où EEVEE calcule le rendu par logiciel). Un appel qui ne fait
que lire ou modifier la scène a pris 1 ms sans interface et environ 70 ms avec elle (le minuteur regarde toutes les
50 ms). N'ont pas fait partie de ces essais : un pont démarré depuis la console de l'espace de travail *Scripting*
d'une interface ouverte par l'opérateur, et une session sans affichage avec l'interface.

## Ce que fait le module

Le Hub joint Blender en mode `tcp`. Un pont tourne dans le Python de Blender, n'écoute que sur `127.0.0.1` et exécute
les scripts que le Hub envoie :

- `execute_script` exécute dans Blender un script Python complet et rend ce qu'il a affiché, la valeur de sa variable
  `result` et, s'il a échoué, la trace d'erreur de Python.
- `get_state` rend la scène active, ses objets, les objets sélectionnés, les images de début et de fin de l'animation et
  le moteur de rendu. Il ne rend jamais de capture (voir plus bas).

Le pont est le fichier généré `bridges/generated/blender_bridge.py` : le cœur commun (`bridges/_core.py`) suivi de
l'adaptateur Blender (`bridges/adapters/blender.py`), assemblés par `python3 scripts/build_bridges.py`. Il n'emploie que
la bibliothèque standard de Python et se charge avec Python 3.9 ou plus récent. Il ne détient aucun jeton : il lit le
fichier de jeton au démarrage.

Le paquet installe le même fichier sous `/usr/share/libremodular-mcp/bridges/blender_bridge.py`. Les commandes ci-dessous nomment le fichier du pont d'un clone, puis celui du paquet.

## Démarrage du pont

Le pont ne démarre jamais seul : l'opérateur le démarre à la main, à chaque fois.

1. Créez le jeton une fois, s'il n'existe pas encore :

   ```sh
   libremodular-mcp init
   ```

2. Activez Blender dans la configuration du Hub, `~/.config/libremodular-mcp/config.toml` :

   ```toml
   [apps.blender]
   enabled = true
   mode = "tcp"
   port = 9303
   ```

3. Démarrez Blender avec le pont. Le pont lit deux variables d'environnement :

   | Variable | Valeur par défaut |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9303` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token`, ou `~/.config/libremodular-mcp/token` quand `XDG_CONFIG_HOME` n'est pas définie ou n'est pas un chemin absolu |

   Avec les valeurs par défaut de `init` et de la configuration ci-dessus, il n'y a rien à définir. Faites exécuter le
   fichier par Blender au démarrage, avec son interface :

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9303 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   blender --python /home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py
   ```

   Depuis le paquet installé :

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9303 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   blender --python /usr/share/libremodular-mcp/bridges/blender_bridge.py
   ```

   ou, dans un Blender déjà ouvert, exécutez-le depuis la console Python de l'espace de travail *Scripting* :

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py", encoding="utf-8").read())
   ```

   Depuis le paquet installé :

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/blender_bridge.py", encoding="utf-8").read())
   ```

   Les variables peuvent aussi être définies dans cette console, avant la ligne ci-dessus, par `os.environ` : le pont les
   lit quand son fichier s'exécute.

4. Le pont écrit une ligne sur le flux d'erreur de Blender (le terminal depuis lequel Blender a été lancé) et, quand la
   console redirige ce flux, dans la console :

   ```text
   LMM bridge listening on 127.0.0.1:9303
   ```

   Le pont refuse de démarrer si le fichier de jeton est un lien symbolique, appartient à un autre utilisateur, est lisible
   ou modifiable par le groupe ou par les autres, n'est pas un fichier ordinaire, ou ne contient pas un jeton créé par
   `libremodular-mcp init`. Pour l'arrêter, exécutez `stop()` dans la console où le fichier a été exécuté, ou quittez
   Blender.

Démarré sans interface (`blender --background --python ...`), Blender quitte dès que le fichier a fini de s'exécuter, ce
qui met fin au pont. Pour le garder en vie, exécutez le fichier du pont depuis un fichier de démarrage qui donne ensuite le
fil principal au pont en appelant `serve_main_thread()` :

```python
import runpy

runpy.run_path("/home/alice/LibreModular-MCP/bridges/generated/blender_bridge.py")[
    "serve_main_thread"
]()
```

Depuis le paquet installé :

```python
import runpy

runpy.run_path("/usr/share/libremodular-mcp/bridges/blender_bridge.py")[
    "serve_main_thread"
]()
```

`serve_main_thread()` rend la main quand le pont s'arrête (`stop()`), au bout de `seconds` si vous passez
`serve_main_thread(seconds=600)`, ou sur Ctrl+C ; Blender quitte alors. Dans ce mode aucun minuteur ne tourne : c'est cet
appel qui exécute les scripts, dans le fil principal, comme le fait la ligne de commande de Blender elle-même. Un fichier
de démarrage qui attend au lieu de l'appeler garde le pont en vie mais exécute les scripts dans le fil réseau du pont, où
`bpy.ops.render.render` peut se bloquer (voir « Rendu »).

Pour vérifier la connexion du côté du modèle, appelez `get_state` avec la cible `hub` : Blender y figure avec
`connected: true` et la version que Blender a indiquée.

## Ce que voit un script

Chaque script s'exécute dans un espace de noms neuf qui contient :

| Nom | Ce que c'est |
|---|---|
| `bpy` | Le module Python de Blender |
| `LMM_WORK_DIR` | Le répertoire privé que le Hub a préparé pour les fichiers de cette application |

- Le script est compilé sous le nom `<libremodular>` : les numéros de ligne d'une trace d'erreur sont ceux du script.
- Tout ce que le script affiche revient dans `stdout` et `stderr`. Pour renvoyer une valeur, affectez à `result` une
  valeur que JSON sait représenter ; toute autre valeur donne `null`, avec un avertissement ajouté à `stderr`.
- Les fichiers créés pendant l'exécution du script sont privés (masque de création `077`).
- `bpy` doit être employé depuis le fil principal de Blender. Quand le fichier du pont s'exécute dans le fil principal de
  Blender avec l'interface, l'adaptateur y enregistre un minuteur persistant (`bpy.app.timers.register`), qui cherche
  toutes les 50 millisecondes les scripts en file d'attente et les exécute ; le fil de connexion attend le résultat. En
  mode arrière-plan, les scripts s'exécutent dans le fil principal tant qu'il est dans `serve_main_thread()` (voir
  « Démarrage du pont »). Dans les autres cas (le paquet `bpy` autonome, un pont démarré depuis un autre fil, ou un fichier
  de démarrage qui n'appelle jamais `serve_main_thread()`), rien n'exécuterait jamais un script en file d'attente : les
  scripts sont donc appelés directement dans le fil du pont. Les deux chemins ont tourné dans Blender 4.3.2 (voir le
  statut ci-dessus).
- Un script qui ne rend jamais la main bloque le fil principal de Blender ; le Hub répond `timeout`, puis `busy` jusqu'à
  la fin du script, et n'envoie aucun second script entre-temps.
- Les opérateurs (`bpy.ops`) dépendent du contexte où ils s'exécutent ; depuis un minuteur, il n'y a pas de zone active.
  `bpy.ops.mesh.primitive_cube_add` et `bpy.ops.render.render` ont tourné en mode arrière-plan (voir « Rendu ») ;
  `bpy.ops.render.render` a aussi tourné avec l'interface sur un affichage virtuel. Tout autre opérateur est non essayé : l'API de données (`bpy.data`) ne dépend pas du contexte.

## Version

`app_version` est le premier mot de `bpy.app.version_string`, par exemple `4.3.2` ; une version à support long nommée
`4.2.3 LTS` se lit `4.2.3`. Sans `bpy`, elle vaut `unknown`.

## État

`get_state("blender")` rend, sous `state` :

| Clé | Contenu |
|---|---|
| `scene` | Le nom de la scène active (`bpy.context.scene`), ou `null` s'il n'y en a pas |
| `objects` | Chaque objet de la scène : `name`, `type` (par exemple `MESH`, `CAMERA`, `LIGHT`) |
| `selected_objects` | Les noms des objets sélectionnés ; un objet hors du calque de vue actif compte comme non sélectionné |
| `frame_range` | Les images d'animation `start` et `end` de la scène, ou `null` sans scène |
| `render_engine` | Le moteur de rendu de la scène, par exemple `CYCLES`, ou `null` sans scène |

Les noms de scènes et d'objets sont du contenu du document : ils sont rendus tels quels et ne sont jamais interprétés,
quoi qu'ils disent.

Quand `bpy` ne peut pas être importé (le fichier a été exécuté hors de Blender), `state` vaut `{}`, `app_version` vaut
`unknown` et `warnings` le dit.

## Pas de capture

`screenshot` vaut toujours `null` pour Blender. Produire une image reviendrait à lancer un rendu implicitement, ce
qu'une demande de lecture de l'état ne doit jamais faire, et il n'existe pas de copie sûre de la vue 3D sans l'interface.
Le rendu depuis un script est décrit plus bas.

## Rendu

`bpy.ops.render.render(write_still=True)` fonctionne par le pont quand le fil principal exécute le script : avec
l'interface (le minuteur), et dans `blender -b` quand le fichier de démarrage appelle `serve_main_thread()` (voir
« Démarrage du pont »). Dans Blender 4.3.2, un rendu de 128 par 128 d'un cube dans le moteur par défaut
(`BLENDER_EEVEE_NEXT`) a rendu `ok` en 0,6 à 0,9 s chaque fois qu'il a été essayé ainsi (11 exécutions, avec et sans
variable d'affichage, et sous charge du processeur), et `get_state("blender")` répondait ensuite. Un fichier de démarrage
qui attend au lieu de l'appeler laisse le script dans le fil réseau du pont. Le même rendu y a répondu en 0,6 à 0,9 s dans
22 exécutions sur 23, et une fois ne s'est jamais terminé : le Hub a répondu `timeout` au bout de 30 secondes, puis
`busy`. Blender ne prend pas en charge `bpy.ops` hors de son fil principal ; ce fil peut donc se bloquer à n'importe quel
rendu, et exécuter le script dans le fil principal supprime la cause. Définissez `filepath` sous `LMM_WORK_DIR`.

Dans un profil neuf, le premier rendu est lent. Cinq profils neufs ont donné un premier rendu de 128 par 128 en 30,0 à
30,7 s, et les rendus suivants en 0,12 à 0,13 s. Le délai par défaut du Hub est de 30 s : quatre des cinq premiers rendus
ont rendu `ok`, un a rendu `timeout`, et les appels suivants ont reçu `busy` jusqu'à sa fin. Augmentez `default_timeout_s`
dans la section `[hub]` de `config.toml`, ou faites un rendu à la main avant qu'un modèle ne le fasse.

Pour un rendu piloté par `bpy` et jugé au pixel, dans le moteur Workbench (mesuré dans Blender 4.3.2, chaque point étant un
rendu qui est sorti faux sans erreur) :

- `scene.display.shading.color_type = "OBJECT"` lit `object.color`, blanc par défaut : tout est rendu en blanc. Employez
  `"MATERIAL"`.
- `material.diffuse_color` est linéaire. Écrire la valeur sRGB de `#c4553a` rend `#E39C83`. Convertissez chaque canal de
  sRGB en linéaire avant d'affecter la valeur.
- Le fond d'un rendu Workbench reste le gris du thème (`srgba(64,64,64,1)`) même quand une couleur de monde est affectée ;
  `background_type = "WORLD"` ne change rien, et `"CUSTOM"` échoue en mode arrière-plan. Rendez sur un film transparent
  et composez le fond ensuite (`magick render.png -background "#f4efe8" -alpha remove -alpha off`).
- Un biseau posé après `obj.scale` est écrasé par l'échelle. Appliquez d'abord l'échelle au maillage
  (`bpy.ops.object.transform_apply(scale=True)`), puis ajoutez le biseau.
- Un PNG transparent paraît vide dans une visionneuse qui compose sur du blanc : jugez le rendu sur l'image composée, et
  lisez les vraies couleurs avec un histogramme (`magick render.png +dither -colors 6 -format %c histogram:info:`).

Dans le moteur par défaut (`BLENDER_EEVEE_NEXT`, mesuré dans Blender 4.3.2), la transformation de vue d'une scène neuve
est `AgX`, qui modifie les couleurs en route vers le PNG : une couleur de monde écrite `#1c385c` a été rendue `#103759`,
et la réponse du script était `ok`. Avec `scene.view_settings.view_transform = "Standard"`, le même fond a été rendu
`#1C385C`. Une surface éclairée ne se lit jamais comme la couleur de son matériau, quelle que soit la transformation.

## Exemple

Un script qui ajoute un triangle à la scène active avec l'API de données. Exécuté tel quel dans la scène par défaut, il
rend `{"object": "Generated triangle", "objects": 4}`.

```python
mesh = bpy.data.meshes.new("Generated mesh")
mesh.from_pydata([(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)], [], [(0, 1, 2)])
mesh.update()
item = bpy.data.objects.new("Generated triangle", mesh)
bpy.context.scene.collection.objects.link(item)
result = {"object": item.name, "objects": len(bpy.context.scene.objects)}
```

## Limites

- Le garde-fou de script du Hub ne voit pas les fonctions d'enregistrement et d'export propres à Blender : un script peut
  écraser un fichier par `bpy.ops.wm.save_as_mainfile`. Le garde-fou est un filet de sécurité contre les accidents, pas un
  bac à sable.
- Un script s'exécute avec les droits de l'utilisateur, dans le Python de Blender.
- Le pont ne sert qu'une connexion à la fois.
- Un script ne remplace pas le fichier ouvert. Pendant un script, le pont refuse `bpy.ops.wm.open_mainfile`,
  `wm.revert_mainfile`, `wm.read_homefile` et `wm.read_factory_settings` : l'appel lève `RuntimeError`
  (`bpy.ops.wm.open_mainfile cannot run from a bridge script: it ends Blender. Start Blender on the file instead
  (blender -b <file>.blend --python <boot file>).`), le Hub répond `error` et Blender reste debout. Sans ce refus, ces
  opérateurs arrêtaient Blender 4.3.2 (le Hub répondait `unavailable`). Pour travailler sur un autre fichier, lancez
  Blender dessus (`blender -b scene.blend --python <boot file>`).
- Cycles ne peut pas débruiter avec la compilation Debian 4.3.2 : avec `scene.cycles.use_denoising = True`,
  `bpy.ops.render.render` a levé `Failed to denoise, build has no OpenImageDenoise support`. Laissez `False` et
  augmentez `scene.cycles.samples` (320 échantillons en 1 360 par 766 pixels ont pris de 5 à 23 s sur le processeur, selon la mesure).
- Un rendu Workbench dessine chaque objet en volume plein, quoi que dise `object.display_type` : `WIRE` n'a rien
  changé. Un modificateur `WIREFRAME` avec `use_replace = True` dessine les arêtes.
