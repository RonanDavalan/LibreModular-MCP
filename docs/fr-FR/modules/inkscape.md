# Module Inkscape

**Statut : essayé dans Inkscape 1.4 (paquet Debian 13), piloté par le Hub via MCP sur stdio.**
Avec `/usr/bin/python3` (3.13.5) et `/usr/share/inkscape/extensions` sur `PYTHONPATH`, l'exemple ci-dessous a tourné et
rendu `{"layers": ["Layer 1", "Notes"]}` en 141 ms ; `get_state` a ensuite listé les deux calques et une capture PNG
(RGBA, mode 600) ; un script qui a levé une exception a rendu sa trace d'erreur avec la bonne ligne ; un second script a
retrouvé le document tel que le premier l'avait laissé. Un défaut est apparu lors de cet essai et il est corrigé : `import inkex`
importe numpy, et sur une machine à nombreux cœurs sa bibliothèque BLAS lançait un fil d'exécution par cœur, qui ne
finissait jamais de démarrer sous la limite de 2 Gio d'espace d'adressage (l'appel se terminait en `timeout` après 30 s).
Le module définit désormais `OPENBLAS_NUM_THREADS=1` et `OMP_NUM_THREADS=1`.

## Ce que fait le module

Le Hub joint Inkscape en mode `cli` : il n'y a ni pont ni processus de longue durée. Chaque `execute_script` lance un
processus Python qui exécute le script puis s'achève ; chaque `get_state` lance un processus Python qui lit le document de
travail, puis exécute `inkscape` une fois pour exporter une capture.

Le document de travail est `current.svg`, dans le répertoire de travail privé que le Hub réserve à Inkscape,
`<runtime>/libremodular-mcp/work/inkscape/`. Le premier script le crée, privé (mode 600), sous la forme d'un SVG vide avec
un calque nommé `Layer 1`. Chaque script part de ce fichier tel que le script précédent l'a laissé : rien d'autre ne
persiste d'un script à l'autre. Si `current.svg` est un lien symbolique, appartient à un autre utilisateur ou est accessible au groupe ou aux autres, le Hub refuse de l'utiliser et répond `unavailable`.

## Configuration

```toml
[apps.inkscape]
enabled = true
mode = "cli"
```

Il n'y a pas de port. Les programmes que le module lance sont déterminés au démarrage du Hub, et ni un script ni la
configuration ne peuvent les changer :

| Programme | Chemin |
|---|---|
| Interpréteur Python | `/usr/bin/python3` |
| Extensions d'Inkscape, placées sur `PYTHONPATH` pour que `import inkex` fonctionne | `/usr/share/inkscape/extensions` |
| Inkscape, pour sa version et pour les captures | `/usr/bin/inkscape` |

## Ce que voit un script

Le script est écrit dans un fichier privé du répertoire de travail, jamais passé sur la ligne de commande, et supprimé
quand le processus s'est achevé. La commande est une liste d'arguments, sans shell : `python3 -c <fixed bootstrap> <script file> <limits>`.
Le programme d'amorçage applique les limites ci-dessous, puis compile le script sous le nom `<libremodular>` : les numéros
de ligne d'une trace d'erreur sont donc ceux du script.

Le processus démarre dans le répertoire de travail, dans son propre groupe de processus, avec exactement cet environnement et
rien d'autre venant du Hub (en particulier jamais le jeton) :

| Variable | Valeur |
|---|---|
| `PATH`, `HOME`, `LANG`, `LC_ALL` | Les valeurs du Hub, quand le Hub les a |
| `PYTHONPATH` | `/usr/share/inkscape/extensions` |
| `LMM_DOCUMENT` | Le chemin de `current.svg` |
| `LMM_WORK_DIR` | Le répertoire de travail |
| `OPENBLAS_NUM_THREADS`, `OMP_NUM_THREADS` | `1` : numpy, que `inkex` importe, lancerait sinon un fil d'exécution par cœur |

Les fichiers que le script crée suivent le masque de création du Hub, `077` : ils sont privés. Un script qui remplace
`current.svg` par un nouveau fichier laisse donc un fichier privé que le script suivant peut utiliser ; celui qui écrit un
fichier lisible par d'autres oblige le Hub à le refuser.

## Résultat et erreurs

- Pour rendre une valeur, un script affiche `LMM_RESULT: ` suivi d'un JSON, en **dernière ligne** de sa sortie standard.
  Le Hub retire cette ligne de `stdout` et rend la valeur comme `result`. Sans cette ligne, ou quand son JSON est invalide
  (la ligne reste alors dans `stdout`), `result` vaut `null`. Si la sortie dépasse `max_output_bytes`, la fin de la sortie
  est perdue, et la ligne de résultat avec elle ; la réponse porte `"truncated": true`.
- Quand le script lève une exception (`SystemExit` comprise), le programme d'amorçage écrit `LMM_ERROR: ` suivi de
  `{"type", "message", "traceback"}` en dernière ligne de l'erreur standard. Le Hub retire cette ligne de `stderr` et la
  rend comme `error`, avec la trace d'erreur de Python.
- Un processus qui s'achève avec un code non nul, ou par un signal, sans cette ligne (tué par une limite, par exemple)
  donne `error.type` égal à `ProcessError` et un message qui porte le code de sortie ou le signal ; il n'y a pas de trace
  d'erreur.

## Limites

| Limite | Valeur |
|---|---|
| Durée réelle | `default_timeout_s` du Hub (30 s par défaut). Une fois écoulée, tous les processus du groupe, enfants compris, sont tués par `SIGKILL`, et la réponse est `timeout`. |
| `RLIMIT_CPU` | Le même nombre de secondes |
| `RLIMIT_AS` (espace d'adressage) | 2 Gio : une allocation plus grande échoue avec `MemoryError` ou met fin au processus |
| `RLIMIT_NOFILE` | 256 fichiers ouverts |
| `RLIMIT_CORE` | 0 : aucun vidage mémoire |
| Sortie | `max_output_bytes` par flux ; le reste est lu puis abandonné, si bien que le processus ne reste jamais bloqué sur un tube plein |

Quand un processus s'achève, tout ce qui tourne encore dans son groupe est tué aussi, et le Hub répond une fois le groupe disparu (deux secondes au plus). Un processus qui quitte son groupe
volontairement (avec `setsid`, par exemple) n'est pas suivi. Un seul script tourne à la fois : une seconde requête
pendant ce temps reçoit `busy`.

## État

`get_state("inkscape")` lit `current.svg` dans un processus Python lancé avec le même environnement et les mêmes limites,
et rend, sous `state` :

| Clé | Contenu |
|---|---|
| `width`, `height` | Les attributs de l'élément racine, tels qu'ils sont écrits (par exemple `210mm`), ou `null` |
| `element_count` | Le nombre d'éléments, racine comprise |
| `layers` | Chaque élément `g` dont `inkscape:groupmode` vaut `layer`, à toute profondeur : `id`, `label` (`inkscape:label`) |

Tant que `current.svg` n'existe pas, `state` vaut `{}` et un avertissement dit que le document n'existe pas encore ;
consulter l'état ne le crée jamais. Un `current.svg` qui existe est lu, qu'un script ait tourné ou non depuis le démarrage du
Hub : un document laissé par une exécution antérieure du Hub est donc décrit. Un document qui déclare des entités est
refusé : `state` vaut `{}` et un avertissement le dit. Rien dans le document n'est résolu ni récupéré, et un `DOCTYPE` sans
déclaration d'entité est lu sans que sa DTD soit récupérée. Le Hub lui-même n'analyse aucun XML.

Les noms de calques sont du contenu du document : ils sont rendus tels quels et ne sont jamais interprétés, quoi qu'ils
disent. Un calque créé avec `inkex.Layer.new` n'a pas de `id` tant que le script n'en définit pas un, et il est listé avec
`id: null`.

## Capture

`inkscape --export-type=png --export-filename=<path chosen by the Hub> current.svg`, avec le même environnement, dans son
propre groupe de processus, et 30 secondes pour s'achever. Les limites de ressources des scripts ne s'appliquent pas à
Inkscape lui-même. Si Inkscape est absent ou échoue, `screenshot` vaut `null` et un avertissement en donne la raison. Le
Hub vérifie ensuite le fichier avant d'en rendre le chemin.

## Connexion et version

`get_state("hub")` montre Inkscape comme connecté quand `/usr/bin/python3` existe et que `inkscape --version` répond en
moins de 5 secondes ; la version est le mot qui suit `Inkscape` dans cette réponse (par exemple `1.2.2`). Le module ne
garde aucun processus : le libérer ne fait rien, et une fenêtre Inkscape que l'opérateur a ouverte n'est jamais touchée.

## Exemple

Un script qui ajoute un calque contenant un rectangle par `inkex`, enregistre le document et rend les noms des calques.
Exécuté tel quel sur un document neuf, il rend `{"layers": ["Layer 1", "Notes"]}`.

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

## `inkex` 1.4 : pièges qui ne lèvent aucune erreur

Observé dans Inkscape 1.4.

- `inkex.Circle.new(a, b)` et `inkex.Ellipse.new(a, b)` prennent deux arguments, et le second n'est pas le centre :
  `inkex.Circle.new(64, 64)` donne `cx="64.0" cy="0.0" r="64.0"`, un cercle collé au bord supérieur du document. Un
  troisième argument lève une `TypeError`. Créez l'élément, puis définissez `cx` et `cy` explicitement.
- `inkex.Path` n'a aucune méthode de segment (`M`, `L`, `C`, `Z`, `moveTo`, `lineTo`). Définissez plutôt l'attribut `d` d'un
  élément `path`, par exemple avec `lxml.etree.SubElement` et `.set("d", ...)`.
- Le document est conservé d'un script à l'autre : ce qu'un script a ajouté est encore là pour le suivant. Un script qui
  reconstruit tout le dessin commence par retirer les enfants de la racine (`for child in list(svg): svg.remove(child)`).

## Limites de la protection

- Le garde-fou de script du Hub ne voit pas les moyens propres à `inkex` d'écrire ou d'exécuter quoi que ce soit. Le
  garde-fou est un filet de sécurité contre les accidents, pas un bac à sable.
- Un script s'exécute avec les droits de l'utilisateur.
