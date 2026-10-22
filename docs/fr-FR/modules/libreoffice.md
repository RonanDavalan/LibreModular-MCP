# Module LibreOffice

**Statut : essayé dans LibreOffice 24.2.7.2 et 25.2.3.2.** L'adaptateur et le lanceur ont été écrits d'après l'API UNO
publique et d'après des mesures faites sur LibreOffice 25.2.3.2 ; ils ont ensuite tourné contre LibreOffice 24.2.7.2 (les
paquets `libreoffice-core`, `-writer`, `-calc`, `-impress` et `python3-uno` d'Ubuntu 24.04, avec le `/usr/bin/python3`
3.11.15 du système) et contre LibreOffice 25.2.3.2 (les paquets de Debian 13, avec le `/usr/bin/python3` 3.13.5 du
système), de deux façons :

- sans interface, par `cargo test --test libreoffice_real -- --ignored --nocapture`, qui démarre son propre LibreOffice sur
  un profil et un port jetables, démarre le pont généré avec l'environnement que le lanceur lui donne, et pilote les trois
  cibles par le Hub (réussi sur les deux versions) ;
- avec une fenêtre sur un affichage X virtuel, démarré par le vrai `libremodular-launch`, piloté par le vrai
  `libremodular-mcp` par MCP sur stdio : `get_state("hub")` a listé les trois cibles comme connectées en mode `uno`,
  avec la version de LibreOffice (`24.2.7.2`, puis `25.2.3.2`) ; après l'ouverture d'un document Calc dans la fenêtre, un
  script a écrit deux cellules, `get_state` a décrit la feuille et rendu une capture PNG, un script pour Writer a été
  refusé avec `WrongDocumentType`, et `--stop` a mis fin au pont en laissant LibreOffice tourner.

Ce que fait un script quand aucun document n'est ouvert (il s'exécute, avec `doc` valant `None`, et peut créer le premier
document) a été ajouté après ces deux essais. Cela n'a tourné que contre LibreOffice 24.2.7.2, sans interface
(`cargo test --test libreoffice_real`) et avec une fenêtre sur un affichage X virtuel (l'exemple en fin de page). Un pont
qui démarre avant que LibreOffice écoute, et la version qu'il indique alors, n'ont tourné que contre 24.2.7.2 sans
interface : voir *Limites*.

N'ont pas fait partie de ces essais : une vraie session de bureau, un document de plusieurs feuilles ou pages (ce que
montre sa capture), des documents d'autres sortes ouverts à côté des trois familles, et un LibreOffice qui met plus de
temps à démarrer que le pont n'attend (voir la version sous *Limites*). Les tests unitaires de l'adaptateur emploient un
faux `uno` et se déroulent de la même façon, que LibreOffice soit installé ou non.

## Ce que fait le module

Le Hub joint LibreOffice en mode `uno`, par **trois cibles qui partagent un pont et une connexion** :

| Cible | Famille | Documents qu'elle sert |
|---|---|---|
| `libreoffice-writer` | `writer` | Documents texte |
| `libreoffice-calc` | `calc` | Classeurs |
| `libreoffice-impress` | `impress` | Présentations |

Le pont est un processus Python autonome, non un greffon : il se connecte, à chaque requête, au port UNO sur lequel
LibreOffice écoute, et y exécute le script. Le Hub ne parle jamais UNO : il parle au pont sur `bridge_port` comme à
n'importe quel autre pont, et nomme la famille de la cible dans chaque requête. Un script en cours sur une cible fait
répondre `busy` à une requête sur une autre, puisque le pont exécute un script à la fois.

- `execute_script` exécute un script Python complet sur le **document courant** de la famille de la cible, ou avec `doc`
  valant `None` quand LibreOffice n'a pas de document courant, et rend ce qu'il a affiché, la valeur de sa variable
  `result` et, s'il a échoué, la trace d'erreur.
- `get_state` liste les documents ouverts, décrit le document actif et rend le chemin d'une capture PNG de ce dernier.

Le pont est le fichier généré `bridges/generated/uno_bridge.py` : le cœur commun (`bridges/_core.py`) suivi de
l'adaptateur (`bridges/adapters/uno.py`), assemblés par `python3 scripts/build_bridges.py`. Il n'emploie que la
bibliothèque standard de Python, hormis `uno`, qu'il importe quand une requête en a besoin ; il se charge avec Python 3.9
ou plus récent, avec ou sans `uno`. Il ne détient aucun jeton.

Le paquet installe le même fichier sous `/usr/share/libremodular-mcp/bridges/uno_bridge.py`. Les commandes ci-dessous nomment le fichier du pont d'un clone, puis celui du paquet.

## La limite à connaître d'abord

**Le port UNO de LibreOffice n'a aucune authentification.** Quiconque peut s'y connecter peut piloter LibreOffice : ouvrir,
modifier, enregistrer et fermer des documents, et atteindre tout ce que le Python de LibreOffice peut atteindre, avec les
droits de l'utilisateur. Il ne demande aucun jeton, ne passe par aucun garde-fou de script et ne laisse rien dans le
journal d'audit. Seul le pont est protégé par le jeton, et seulement du côté du Hub.

- LibreOffice reçoit l'ordre d'écouter sur `127.0.0.1` seulement : rien hors de la machine ne l'atteint. Tout autre
  processus capable d'ouvrir une connexion vers ce port sur cette machine, lui, l'atteint.
- Que LibreOffice filtre les connexions par utilisateur : non confirmé ; les essais ci-dessus se connectaient avec le
  même utilisateur. Supposez qu'il ne filtre pas.
- Le port existe tant que LibreOffice a été démarré avec `--accept`. Fermez cette instance quand vous n'en avez pas besoin ;
  le lanceur ne le fait jamais à votre place.
- Le garde-fou de script du Hub ne regarde que les scripts envoyés par `execute_script`. Un programme qui parle directement
  au port UNO ne passe pas sous ses yeux.

## Démarrage

Le Hub ne démarre jamais LibreOffice ni le pont, et jamais sur la parole du modèle. Les deux démarrent sur un geste explicite
de l'opérateur.

Activez les cibles dans `~/.config/libremodular-mcp/config.toml`. Les trois cibles sont un seul LibreOffice et un seul
pont : les cibles activées doivent nommer les mêmes ports, sans quoi le lanceur refuse. Les valeurs ci-dessous sont les
valeurs par défaut.

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

`uno_port` est le port sur lequel LibreOffice écoute et que seul le lanceur emploie ; `bridge_port` est le port du pont,
celui auquel le Hub se connecte.

### Avec le lanceur

Créez le jeton une fois, s'il n'existe pas encore (`libremodular-mcp init`), puis :

```sh
libremodular-launch libreoffice --bridge-file /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
```

Depuis le paquet installé :

```sh
libremodular-launch libreoffice --bridge-file /usr/share/libremodular-mcp/bridges/uno_bridge.py
```

Il démarre LibreOffice avec sa fenêtre, puis le pont, et rend la main. Sortie d'une exécution :

```text
LibreOffice started (process 32212), listening for UNO on 127.0.0.1:2002
bridge started (process 32213), listening on 127.0.0.1:9304
a script runs on the current document of its target's family, or with doc set to None when none is open
stop the bridge with 'libremodular-launch libreoffice --stop'; LibreOffice is yours to close
```

Ce que le lanceur fait, et ne fait pas :

- LibreOffice est démarré comme `/usr/bin/soffice --accept=socket,host=127.0.0.1,port=2002;urp;
  --norestore --nologo --nodefault`, à partir d'une liste d'arguments fixe, sans shell et sans `--headless`. Il reçoit
  l'environnement du lanceur, hormis les variables dont le nom commence par `LIBREMODULAR_`.
- Le pont est démarré comme `/usr/bin/python3 <the bridge file>`, avec exactement ces variables : `PATH`, `HOME`, `LANG` et
  `LC_ALL` telles que le lanceur les a, `LD_LIBRARY_PATH` (`/usr/lib/libreoffice/program`) et `PYTHONPATH`
  (`/usr/lib/python3/dist-packages`) pour que le Python du système trouve `uno`, `LIBREMODULAR_UNO_PORT`,
  `LIBREMODULAR_BRIDGE_PORT` et `LIBREMODULAR_TOKEN_PATH`. Le jeton lui-même n'est dans aucun argument ni aucune variable :
  le pont lit le fichier.
- Les deux processus sont détachés, dans leurs propres groupes, leur entrée et leur sortie envoyées nulle part. Ils continuent
  quand le lanceur rend la main.
- Le fichier du pont doit être un chemin absolu vers un fichier ordinaire, non un lien, vous appartenir ou appartenir à root, et ne
  pas être modifiable par le groupe ni par les autres. Un clone récent fait avec un `umask` de `002` (la valeur par défaut de
  nombreux comptes Debian) donne le mode `664`, que le lanceur refuse : exécutez une fois
  `chmod 600 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py`. Le fichier installé par le paquet
  appartient à root avec le mode `644` : rien à changer. Le répertoire qui contient le fichier n'est
  pas vérifié.
- **Il refuse de démarrer si un LibreOffice à vous tourne déjà** (un processus `soffice.bin` de votre utilisateur, lu dans
  `/proc`). Il ne se rattache jamais à une instance qui peut contenir du travail non enregistré, et ne ferme jamais une
  instance qu'il n'a pas démarrée. Fermez LibreOffice et relancez-le, ou démarrez LibreOffice vous-même comme indiqué
  ci-dessous.
- Il refuse de démarrer un second pont tant que le pont d'un lancement précédent tourne.
- Les processus qu'il a démarrés sont consignés dans `$XDG_RUNTIME_DIR/libremodular-mcp/launch/libreoffice.pids`
  (répertoire `700`, fichier `600`) :

  ```text
  soffice 32212
  bridge 32213 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
  ```

  Le premier nombre est le processus que le lanceur a démarré ; ce n'est pas le processus `soffice.bin` que le système
  liste (numéro 32240 dans cette exécution).

`libremodular-launch libreoffice --stop` met fin au pont, **seulement après avoir vérifié que le processus consigné est bien
le pont** (l'un de ses arguments est exactement le fichier du pont) : un numéro qu'un autre programme a repris depuis
ne reçoit jamais de signal. Il ne ferme pas LibreOffice. Fermer une instance qui peut contenir du travail est votre geste, et
le message le dit.

Codes de sortie : `0` fait ; `2` un mauvais usage, ou un refus que vous pouvez corriger (aucune cible activée, ports
différents ou égaux, fichier de pont auquel on ne peut pas se fier, LibreOffice ou pont déjà en cours) ; `1` un échec
(configuration illisible, pas de `XDG_RUNTIME_DIR`, programme qui n'a pas démarré). Les autres applications se démarrent à
la main et répondent `start the bridge by hand, see docs/modules/<name>.md` avec le code `2`.

Si le pont ne peut pas être démarré après LibreOffice, le lanceur le dit, ne consigne que LibreOffice et le laisse tourner.

### À la main

Démarrez LibreOffice avec l'option `--accept`. L'hôte est le nombre `127.0.0.1`, jamais un nom, qui pourrait se résoudre en
une adresse sur laquelle LibreOffice n'écoute pas :

```sh
soffice --accept="socket,host=127.0.0.1,port=2002;urp;" --norestore --nologo --nodefault
```

Puis, une fois LibreOffice en marche, démarrez le pont avec le Python du système, qui a `uno` :

```sh
LD_LIBRARY_PATH=/usr/lib/libreoffice/program \
PYTHONPATH=/usr/lib/python3/dist-packages \
LIBREMODULAR_UNO_PORT=2002 \
LIBREMODULAR_BRIDGE_PORT=9304 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
/usr/bin/python3 /home/alice/LibreModular-MCP/bridges/generated/uno_bridge.py
```

Depuis le paquet installé :

```sh
LD_LIBRARY_PATH=/usr/lib/libreoffice/program \
PYTHONPATH=/usr/lib/python3/dist-packages \
LIBREMODULAR_UNO_PORT=2002 \
LIBREMODULAR_BRIDGE_PORT=9304 \
LIBREMODULAR_TOKEN_PATH=/home/alice/.config/libremodular-mcp/token \
/usr/bin/python3 /usr/share/libremodular-mcp/bridges/uno_bridge.py
```

Le pont écrit une ligne sur son flux d'erreur, `LMM bridge listening on 127.0.0.1:9304`, et tourne jusqu'à l'arrêt du
processus. Il refuse de démarrer si le fichier de jeton est un lien, appartient à un autre utilisateur, est accessible au groupe ou aux autres, ou n'est pas un jeton créé par `libremodular-mcp init`.

### Le premier document

**Un script s'exécute même quand aucun document n'est ouvert.** `doc` vaut alors `None`, et `uno`, `ctx` et `desktop` sont
fournis comme d'habitude : le script peut donc créer lui-même le document :

```python
if doc is None:
    doc = desktop.loadComponentFromURL('private:factory/scalc', '_blank', 0, ())
```

Le document qu'il crée devient le document courant de LibreOffice : la requête suivante le trouve comme son `doc`. Les noms
de fabriques sont `private:factory/swriter`, `private:factory/scalc` et `private:factory/simpress`. Le script d'une cible
crée un document de n'importe quelle famille ; la requête suivante est refusée avec `WrongDocumentType` quand la famille
de la cible n'est pas celle du document courant (voir *Erreurs*).

Pour vérifier la connexion du côté du modèle, appelez `get_state` avec la cible `hub` : les trois cibles y figurent avec
`connected: true` et la version que LibreOffice a indiquée.

## Ce que voit un script

Un script s'exécute dans le pont, qui est un processus à part, dans un espace de noms neuf qui contient :

| Nom | Ce que c'est |
|---|---|
| `uno` | Le module `uno` |
| `ctx` | Le contexte de composants distant de LibreOffice |
| `desktop` | `com.sun.star.frame.Desktop`, l'application |
| `doc` | Le document courant, de la famille de la cible ; `None` quand LibreOffice n'a pas de document courant |
| `LMM_WORK_DIR` | Le répertoire privé que le Hub a préparé pour les fichiers de cette application |

- Le script est compilé sous le nom `<libremodular>` : les numéros de ligne d'une trace d'erreur sont ceux du script.
- Tout ce que le script affiche revient dans `stdout` et `stderr`. Pour renvoyer une valeur, affectez à `result` une
  valeur que JSON sait représenter ; toute autre valeur donne `null`, avec un avertissement ajouté à `stderr`. Les objets
  UNO ne sont pas du JSON : rendez des valeurs simples.
- La connexion à LibreOffice est faite à chaque requête, et retentée jusqu'à cinq secondes tant que LibreOffice n'a pas
  fini de démarrer.
- La famille est vérifiée avant que quoi que ce soit du script s'exécute, quand il y a un document courant. Le document
  n'est jamais converti : un document Calc n'est pas proposé à un script Writer.
- Une exception levée par UNO est rapportée avec son nom UNO complet, par exemple
  `com.sun.star.container.NoSuchElementException`.
- Tout ce que le script fait par UNO (enregistrer, fermer, exporter) est fait avec vos droits ; le garde-fou de script du Hub
  ne voit pas les fonctions d'enregistrement et d'export de l'application.

## État

`get_state("libreoffice-calc")`, et de même pour les deux autres cibles, rend, sous `state` :

| Clé | Contenu |
|---|---|
| `documents` | Chaque document ouvert des trois familles, dans l'ordre où LibreOffice les liste : `title` et `family` (`writer`, `calc` ou `impress`). Les dessins et les autres composants ne sont pas listés |
| `active` | `null`, ou la description du document actif, ci-dessous |

`active` vaut `null`, avec un avertissement, quand LibreOffice n'a pas de document courant ou que son document courant n'est
pas de la famille de la cible. Ce n'est pas une erreur. Le pont ne sait pas distinguer les deux cas, si bien que
l'avertissement nomme les deux.

| Famille | Contenu de `active` |
|---|---|
| `writer` | `paragraphs` : le nombre de paragraphes (les tableaux sont listés à côté et ne sont pas comptés) ; `pages` : le nombre de pages, ou `null` pour un document sans contrôleur |
| `calc` | `sheets` : une liste de `name`, `last_column` et `last_row`, les indices, comptés à partir de zéro, de la dernière cellule utilisée, tels qu'UNO les donne. Une feuille vide indique `0` et `0`, comme une feuille dont le seul contenu est `A1` |
| `impress` | `slides` : le nombre de diapositives |

Les titres et les noms de feuilles sont du contenu du document : ils sont rendus tels quels et ne sont jamais interprétés,
quoi qu'ils disent.

Quand `uno` ne peut pas être importé par l'interpréteur qui exécute le pont, `state` vaut `{}`, `app_version` vaut `unknown`
et `warnings` le dit. Il en va de même quand LibreOffice n'accepte pas la connexion, la raison figurant dans
l'avertissement.

## Capture

La capture est un export du document actif de la famille, fait par LibreOffice avec son filtre PNG (`writer_png_Export`,
`calc_png_Export` ou `impress_png_Export`) vers le chemin choisi par le Hub. Le document n'est ni enregistré, ni fermé, ni
modifié : après une capture, `hasLocation()` vaut toujours `False` et le document est toujours modifié. Rien dans le pont
n'appelle `store`, `storeSelf`, `storeAsURL`, `close`, `dispose` ni `terminate`, et les tests échouent si c'est le cas.

- Le fichier est écrit par le propre processus de LibreOffice, avec le masque de création de ce processus. Le Hub le
  vérifie ensuite, et le rend privé (mode `600`) dans son propre répertoire privé.
- Sans document actif de la famille, il n'y a pas de capture (`screenshot: null`).
- Ce que montre le PNG d'un document de plusieurs feuilles ou pages : non confirmé.

## Erreurs

| `error.type` | Quand |
|---|---|
| `LibreOfficeUnavailable` | `uno` ne peut pas être importé par l'interpréteur du pont, ou LibreOffice n'accepte aucune connexion dans le délai d'attente. Le message dit lequel |
| `WrongDocumentType` | Le document courant n'est pas de la famille de la cible. Rien du script ne s'est exécuté |
| `UnknownFamily` | La requête n'a nommé aucune famille que le pont sert. Le Hub en nomme toujours une : cela signifie seulement que le Hub et le fichier du pont ne correspondent pas |

Les deux premières sont levées par le pont lui-même : leur trace d'erreur montre donc les cadres du pont et le chemin du fichier du
pont, non des lignes du script. Le message est la partie à lire.

## Exemple

Un script Calc qui crée le document quand aucun n'est ouvert, écrit deux cellules et rend ce qu'il a écrit. Il a tourné
tel quel, sans document ouvert, dans LibreOffice 24.2.7.2 (les paquets d'Ubuntu 24.04, avec une fenêtre sur un affichage X
virtuel), par le vrai `libremodular-mcp` ; la réponse ci-dessous est celle que le Hub a rendue dans cet essai, avec les champs
dans l'ordre où le Hub les écrit. Rejoué avec le document désormais ouvert, il rend les mêmes valeurs.

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

Le `get_state` qui a suivi a rendu, sous `state` :

```json
{
  "active": {"sheets": [{"last_column": 1, "last_row": 1, "name": "Sheet1"}]},
  "documents": [{"family": "calc", "title": "Untitled 1"}]
}
```

## Limites

- **La version est relue tant qu'elle vaut `unknown`.** Le pont la lit à son démarrage et fait une seule tentative, sans
  attendre. Si LibreOffice n'écoute pas encore, il indique `unknown`, et la redemande à chaque nouvelle connexion du Hub et à
  chaque `get_state`, jusqu'à l'avoir ; l'avertissement que l'API ne peut pas être importée l'accompagne. Le Hub garde la
  version reçue dans le `hello` de sa connexion courante : `get_state` et `get_state("hub")` montrent donc `unknown` jusqu'à
  ce que le Hub se reconnecte, même si le pont connaît alors la version. LibreOffice a répondu sur UNO 1,3 seconde après son
  démarrage dans une exécution de test ; à son tout premier démarrage dans un autre environnement (caches froids), il n'avait
  pas répondu au bout de 8 secondes et l'avait fait au bout de 14. Essayé contre LibreOffice 24.2.7.2 : un pont démarré avant
  LibreOffice, puis LibreOffice démarré, puis l'état redemandé et le Hub reconnecté
  (`cargo test --test libreoffice_real -- --ignored`).
- Seul le document **courant** est joignable. Avec plusieurs documents d'une famille ouverts, un script reçoit celui que
  LibreOffice indique comme courant ; les autres sont listés dans `documents` et ne sont pas proposés. Quel document est
  courant quand le focus est sur une autre fenêtre (une boîte de dialogue, l'EDI Basic) : non confirmé.
- Writer, Calc et Impress seulement. Un document Draw, Math ou Base n'est pas un document d'une famille : il n'est pas listé,
  et un script pour n'importe quelle cible est refusé avec `WrongDocumentType` tant qu'il est le document courant.
- Décrire un très gros document Writer compte ses paragraphes un par un par UNO ; la durée n'est bornée que par le délai
  d'expiration du Hub.
- Le garde-fou de script du Hub est une heuristique contre les accidents, pas un bac à sable. Il ne voit pas les fonctions
  d'enregistrement, d'export et de fermeture de LibreOffice, un chemin passé en paramètre d'une fonction, ni rien de ce
  qu'un programme fait en parlant lui-même au port UNO.
- Le pont sert une connexion à la fois, et un script à la fois pour les trois cibles ensemble.
- La variante de la connexion UNO par tube nommé n'est pas prise en charge.
