# Module Krita

**Statut : essayé dans Krita 5.2.9 (paquet Debian 13) sur un affichage X virtuel, pas encore depuis Scripter.** Le pont
a été écrit d'après l'API Python publique de Krita 5 (le module `krita` et sa classe `Krita`). Il a tourné dans
Krita 5.2.9 lancé sous `xvfb-run`, avec un profil jetable dont une extension de démarrage d'une ligne exécutait le fichier
généré dans le fil d'exécution de l'interface (comme le fait Scripter), le Hub le pilotant par MCP sur stdio.
`get_state("hub")` a listé `krita` comme connecté, en version `5.2.9` ; sans document, `get_state("krita")` a rendu
`document: null` ; un script a créé un document de 300 par 200 et l'a ouvert dans la fenêtre ; l'état a alors décrit ce
document (`RGBA`, `U8`, profil `sRGB-elle-V2-srgbtrc.icc`), son calque `Background` et une capture PNG de 300 par
200 pixels (RGBA, mode 600) ; l'exemple ci-dessous a ajouté un calque, et l'état suivant les a listés tous les deux ; un
script qui a levé une exception a rendu sa trace d'erreur avec la bonne ligne. Un appel a pris entre 1 et 7 ms une fois
le pont démarré. N'a pas fait partie de cet essai : le démarrage du pont depuis Scripter lui-même (le chemin de
menu, les variables d'environnement qui lui parviennent, sa durée de vie quand Scripter se ferme).

## Ce que fait le module

Le Hub joint Krita en mode `tcp`. Un pont tourne dans le Python de Krita, n'écoute que sur `127.0.0.1` et exécute les
scripts que le Hub envoie :

- `execute_script` exécute dans Krita un script Python complet et rend ce qu'il a affiché, la valeur de sa variable
  `result` et, s'il a échoué, la trace d'erreur de Python.
- `get_state` rend le document actif, ses calques, le calque actif et le chemin d'une capture PNG du document.

Le pont est le fichier généré `bridges/generated/krita_bridge.py` : le cœur commun (`bridges/_core.py`) suivi de
l'adaptateur Krita (`bridges/adapters/krita.py`), assemblés par `python3 scripts/build_bridges.py`. Il n'emploie que la
bibliothèque standard de Python et se charge avec Python 3.9 ou plus récent. Il ne détient aucun jeton : il lit le
fichier de jeton au démarrage.

Le paquet installe le même fichier sous `/usr/share/libremodular-mcp/bridges/krita_bridge.py`. Les commandes ci-dessous nomment le fichier du pont d'un clone, puis celui du paquet.

## Démarrage du pont

Le pont ne démarre jamais seul. Rien n'est installé dans le répertoire des greffons Python de Krita, qui peut contenir
les extensions de l'utilisateur : ni le code de ce projet ni cette procédure n'y écrivent. L'opérateur exécute à la main
le fichier généré, à chaque fois, depuis le Scripter de Krita.

1. Créez le jeton une fois, s'il n'existe pas encore :

   ```sh
   libremodular-mcp init
   ```

2. Activez Krita dans la configuration du Hub, `~/.config/libremodular-mcp/config.toml` :

   ```toml
   [apps.krita]
   enabled = true
   mode = "tcp"
   port = 9302
   ```

3. Démarrez Krita. Le pont lit deux variables d'environnement :

   | Variable | Valeur par défaut |
   |---|---|
   | `LIBREMODULAR_BRIDGE_PORT` | `9302` |
   | `LIBREMODULAR_TOKEN_PATH` | `$XDG_CONFIG_HOME/libremodular-mcp/token`, ou `~/.config/libremodular-mcp/token` quand `XDG_CONFIG_HOME` n'est pas définie ou n'est pas un chemin absolu |

   Avec les valeurs par défaut de `init` et de la configuration ci-dessus, il n'y a rien à définir. Sinon, définissez-les
   avant de démarrer Krita, par exemple depuis un terminal :

   ```sh
   LIBREMODULAR_BRIDGE_PORT=9302 \
   LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
   krita
   ```

   Le pont les lit quand son fichier s'exécute : vous pouvez donc aussi les définir dans Scripter, avant l'étape suivante,
   par `os.environ`.

4. Ouvrez Scripter (dans Krita 5, sous *Tools → Scripts → Scripter* ; le chemin de menu exact : non confirmé) et
   exécutez :

   ```python
   exec(open("/home/alice/LibreModular-MCP/bridges/generated/krita_bridge.py", encoding="utf-8").read())
   ```

   Depuis le paquet installé :

   ```python
   exec(open("/usr/share/libremodular-mcp/bridges/krita_bridge.py", encoding="utf-8").read())
   ```

   Le pont démarre dans un fil d'exécution d'arrière-plan et Krita reste utilisable. Il écrit une ligne sur le flux
   d'erreur du processus et, quand Scripter redirige ce flux, dans la sortie de Scripter :

   ```text
   LMM bridge listening on 127.0.0.1:9302
   ```

   Le pont refuse de démarrer si le fichier de jeton est un lien symbolique, appartient à un autre utilisateur, est lisible
   ou modifiable par le groupe ou par les autres, n'est pas un fichier ordinaire, ou ne contient pas un jeton créé par
   `libremodular-mcp init`. Il tourne jusqu'à la fermeture de Krita ; pour l'arrêter plus tôt, exécutez `stop()` dans
   Scripter.

Pour vérifier la connexion du côté du modèle, appelez `get_state` avec la cible `hub` : Krita y figure avec
`connected: true` et la version que Krita a indiquée.

## Ce que voit un script

Chaque script s'exécute dans un espace de noms neuf qui contient :

| Nom | Ce que c'est |
|---|---|
| `Krita` | La classe de l'API Python de Krita ; `Krita.instance()` est l'application |
| `LMM_WORK_DIR` | Le répertoire privé que le Hub a préparé pour les fichiers de cette application |

- Le script est compilé sous le nom `<libremodular>` : les numéros de ligne d'une trace d'erreur sont ceux du script.
- Tout ce que le script affiche revient dans `stdout` et `stderr`. Pour renvoyer une valeur, affectez à `result` une
  valeur que JSON sait représenter ; toute autre valeur donne `null`, avec un avertissement ajouté à `stderr`.
- Les fichiers créés pendant l'exécution du script sont privés (masque de création `077`).
- L'API de Krita doit être appelée depuis le fil d'exécution de son interface. Quand le pont démarre dans Krita (l'instance
  `Krita` existe et une application Qt tourne), l'adaptateur crée un petit objet Qt qui vit dans ce fil ; chaque script lui
  est confié par un signal en file d'attente, et le fil de connexion attend le résultat. Un `QTimer` lancé depuis le
  fil de connexion n'est pas employé : ce fil n'a pas de boucle d'événements Qt, le minuteur ne se déclencherait jamais.
  Hors de Krita (pas de module `krita`, ou PyQt installé sans application Qt en cours), les scripts sont appelés
  directement. Le chemin par signal en file d'attente a tourné dans Krita 5.2.9 (voir le statut ci-dessus).
- Un script qui ne rend jamais la main bloque l'interface de Krita ; le Hub répond `timeout`, puis `busy` jusqu'à la fin du
  script, et n'envoie aucun second script entre-temps.

## État

`get_state("krita")` rend, sous `state` :

| Clé | Contenu |
|---|---|
| `document` | `null` sans document actif ; sinon les clés ci-dessous |
| `document.name` | Le nom du document |
| `document.width`, `document.height` | Sa taille en pixels |
| `document.color_model`, `document.color_depth`, `document.color_profile` | Tels que Krita les nomme, par exemple `RGBA`, `U8`, `sRGB-elle-V2-srgbtrc.icc` |
| `document.layers` | Les calques de premier niveau : `id` (l'identifiant unique de Krita), `name`, `visible`, `opacity` (de 0 à 255, comme Krita l'indique), `type` (par exemple `paintlayer`, `grouplayer`, `vectorlayer`) |
| `document.active_layer` | Le `id` du calque actif, ou `null` |

Un groupe de calques est listé comme un seul calque ; ses enfants ne le sont pas.

Les noms de documents et de calques sont du contenu du document : ils sont rendus tels quels et ne sont jamais
interprétés, quoi qu'ils disent.

Quand l'API de Krita est inutilisable (le fichier a été exécuté hors de Krita), `state` vaut `{}`, `app_version` vaut
`unknown` et `warnings` le dit.

## Capture

La capture est la projection du document actif : l'image fusionnée que Krita affiche, lue avec `Document.projection`
après `Document.waitForDone` et enregistrée en PNG au chemin choisi par le Hub. La projection est une copie ; le document
lui-même n'est ni enregistré, ni exporté, ni modifié. Sans document actif, il n'y a pas de capture (`screenshot: null`).
Le Hub vérifie ensuite le fichier avant d'en rendre le chemin.

## Exemple

Un script qui ajoute un calque de peinture vide au document actif. Exécuté tel quel sur un nouveau document qui n'avait
que son calque `Background`, il rend `{"layers": ["Background", "Notes"]}`.

```python
document = Krita.instance().activeDocument()
layer = document.createNode("Notes", "paintlayer")
document.rootNode().addChildNode(layer, None)
document.refreshProjection()
result = {"layers": [node.name() for node in document.topLevelNodes()]}
```

## Limites

- Le garde-fou de script du Hub ne voit pas les fonctions d'enregistrement et d'export propres à Krita : un script peut
  écraser un fichier par `Document.saveAs` ou `Document.exportImage`. Le garde-fou est un filet de sécurité contre les
  accidents, pas un bac à sable.
- Un script s'exécute avec les droits de l'utilisateur, dans le Python de Krita.
- Le pont ne sert qu'une connexion à la fois.
