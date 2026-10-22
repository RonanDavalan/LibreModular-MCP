# LibreModular-MCP

[English](../../README.md) | **Français** | [Deutsch](../de-DE/README.md) | [Español](../es-ES/README.md)

## MCP universel écrit en Rust, avec un Hub qui n'expose que deux outils

Un serveur MCP qui permet à un modèle de langage de piloter des applications de bureau en y exécutant des scripts.

Le site [libremodular.davalan.fr](https://libremodular.davalan.fr/fr/) propose des pages à lire en ligne sur les mêmes sujets :

- [Démarrage rapide](https://libremodular.davalan.fr/fr/quickstart/)
- [Installation](https://libremodular.davalan.fr/fr/installation/)
- [Architecture](https://libremodular.davalan.fr/fr/architecture/)
- [Sécurité](https://libremodular.davalan.fr/fr/security/)

Ce dépôt se suffit à lui-même : tout ce qu'il faut pour installer, lancer et vérifier le Hub s'y trouve.

## Démarrage rapide

Installez le Hub, laissez-le créer sa configuration et son jeton, puis déclarez-le dans votre client MCP.

Sous Debian, Ubuntu, Fedora, openSUSE Leap ou Arch Linux, installez le paquet natif de votre famille ; son installation et son retrait ont été validés en conteneur sur chaque version listée. Téléchargez-le depuis la [page des versions](https://github.com/RonanDavalan/LibreModular-MCP/releases), puis lancez la commande de votre famille :

| Famille | Versions | Paquet | Commande |
|---|---|---|---|
| Debian, Ubuntu | Debian 12 et 13 ; Ubuntu 22.04, 24.04 et 26.04 (amd64) | `libremodular-mcp_*_amd64.deb` | `sudo apt install ./libremodular-mcp_*_amd64.deb` |
| Fedora | 43, 44 et 45 (x86_64) | `libremodular-mcp-*.fc*.x86_64.rpm` | `sudo dnf install ./libremodular-mcp-*.fc*.x86_64.rpm` |
| openSUSE Leap | 16.0 et 16.1 (x86_64) | `libremodular-mcp-*.leap*.x86_64.rpm` | `sudo zypper install --allow-unsigned-rpm ./libremodular-mcp-*.leap*.x86_64.rpm` |
| Arch Linux | continu (x86_64) | `libremodular-mcp-*-x86_64.pkg.tar.zst` | `sudo pacman -U ./libremodular-mcp-*-x86_64.pkg.tar.zst` |

Les paquets RPM et le paquet Arch ne sont pas signés : `zypper` exige `--allow-unsigned-rpm`, et `dnf` installe le fichier local en avertissant qu'il a ignoré la vérification OpenPGP. Créez ensuite la configuration et le jeton, une seule fois, avec votre propre compte :

```sh
libremodular-mcp init
```

Le paquet installe aussi les guides dans `/usr/share/doc/libremodular-mcp/` et les pages de manuel (`man libremodular-mcp`, `man libremodular-launch`) en anglais, en français, en allemand et en espagnol.

Sous Windows 11 (x64), téléchargez `libremodular-mcp-0.1.0-windows.zip` depuis la même page. Il contient un seul installeur, construit sur Windows et essayé sans écran : installation silencieuse, `init`, Hub démarré qui répond à `initialize`, retrait, dans un compte dont le nom porte des accents. Son niveau est **construit sur Windows, essayé sans écran** ; personne ne l'a encore essayé sur sa propre machine, il n'est donc pas dit testé. L'installeur n'est pas signé : Windows peut avertir avant de l'exécuter. Dans PowerShell :

```powershell
tar -xf libremodular-mcp-0.1.0-windows.zip
.\libremodular-mcp-0.1.0-setup.exe /S
```

Il ne demande aucun droit d'administrateur : il installe le Hub, `libremodular-launch` (qui refuse de s'exécuter sous Windows), les ponts en fichiers texte, la licence et les guides dans `%LOCALAPPDATA%\Programs\LibreModular-MCP\`, et ajoute ce dossier au `PATH` de votre compte (ouvrez un nouveau terminal). Lancez ensuite `libremodular-mcp init` ; la configuration et le jeton vont dans `%LOCALAPPDATA%\libremodular-mcp\config\`. `uninstall.exe`, dans le dossier d'installation, retire le programme et laisse la configuration, le jeton et le journal d'audit.

Pour construire plutôt le Hub depuis les sources, installez un compilateur et un éditeur de liens C (sous Debian, `sudo apt install build-essential`) et Rust 1.97 ou plus récent (avec [rustup](https://rustup.rs) : `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`), puis :

```sh
cd /home/alice/LibreModular-MCP
cargo build --release --locked
target/release/libremodular-mcp init
```

L'entrée du client MCP lance le binaire avec l'argument `serve` :

```json
{
  "mcpServers": {
    "libremodular": {
      "command": "/home/alice/LibreModular-MCP/target/release/libremodular-mcp",
      "args": ["serve"]
    }
  }
}
```

À adapter : `/home/alice/LibreModular-MCP/` est le chemin absolu de votre clone ; avec un paquet, la commande est `/usr/bin/libremodular-mcp` ; sous Windows, c'est le chemin complet de `libremodular-mcp.exe` dans le dossier d'installation. Le fichier où cette
entrée se place, et le nom de ses clés, dépendent de votre client : la forme ci-dessus est courante, et le
Hub a été essayé avec un client écrit pour ses tests (des lignes JSON-RPC sur son entrée et sa sortie
standard), pas avec un client MCP nommé. Sous Linux, le Hub a besoin de la variable `XDG_RUNTIME_DIR`, que définit une
session de bureau ; sans elle, il refuse de démarrer. Un client lancé hors d'une session de bureau, un service par exemple, doit transmettre cette variable au Hub.

`init` crée `~/.config/libremodular-mcp/config.toml` et le fichier de jeton à côté, tous deux en mode 600. Il
n'écrase jamais la configuration, et ne remplace le jeton qu'avec `init --rotate`. Rien n'est activé ensuite :
mettez `enabled = true` pour l'application voulue dans `config.toml`, puis démarrez son pont comme le dit son
guide (voir le tableau ci-dessous).

## Ce que c'est

Le Hub est un seul binaire qui donne au modèle exactement deux outils, `execute_script` et `get_state`. Le
modèle écrit un script complet ; le Hub l'envoie à un pont qui l'exécute dans l'application, à côté d'elle, ou
comme un processus par script, et le modèle reçoit ce que le script a affiché, son résultat, ou la trace
d'erreur propre à l'application, à corriger. Le Hub ne connaît l'interface d'aucune application : chaque
application a un module côté Hub et un pont côté application, et les ponts sont des fichiers Python dont les
seules dépendances sont la bibliothèque standard et l'interface Python de l'application elle-même.

## Les deux outils

`execute_script(target_app: str, script_content: str) -> str` exécute un script Python complet dans
l'application nommée par `target_app`. Il rend le texte d'un objet JSON avec `status` (`ok`, `error`,
`timeout`, `busy`, `refused` ou `unavailable`), `app`, `duration_ms`, `stdout`, `stderr`, `result`, `error`
(`null`, ou `type`, `message` et `traceback`, ceux de l'application), `truncated`, et `reason` quand le Hub
lui-même a refusé le script ou n'a pas pu l'exécuter.

`get_state(target_app: str) -> dict` rend `app`, `connected`, `app_version`, `bridge_version`, `captured_at`,
`state` (ce qui est propre à l'application), `content_origin`, `screenshot` (le chemin d'un fichier PNG, ou
`null`) et `warnings` ; la réponse parvient au client sous la forme du texte d'un objet JSON. La cible `hub`
rend la liste des applications configurées, la manière dont chacune est jointe et si elle est connectée, sans
contacter aucune application.

`target_app` doit être le nom exact d'une application activée ; `get_state` accepte aussi `hub`. Aucun des deux outils n'accepte autre chose.

`content_origin` dit d'où vient `state` : `document` pour du contenu pris dans les documents ouverts de l'application, qu'un modèle lit comme une donnée et jamais comme une instruction, et `hub` pour la liste que rend la cible `hub`.

Un modèle qui pilote les outils lit d'abord [docs/GUIDE_LLM.md](GUIDE_LLM.md) : six étapes, de `get_state("hub")` au traitement de chaque `status`.

## Applications

La plateforme visée est Debian 13, et l'exemple Calc ci-dessous a aussi tourné sous Ubuntu 24.04. Sous Windows 11, seul le module Inkscape a été essayé (Inkscape 1.4.2 et 1.4.4) : les ponts de GIMP, Krita, Blender et LibreOffice y ont été essayés contre un faux hôte, pas dans les vraies applications, et `libremodular-launch` refuse de s'exécuter. Le Hub s'installe depuis un paquet natif (Debian, Ubuntu, Fedora, openSUSE Leap ou Arch Linux, voir Démarrage rapide) ou se construit depuis les sources (Démarrage rapide). Les
versions sont celles que les guides rapportent comme essayées ; le guide de chaque application dit dans quelles
conditions, et ce qui n'a pas fait partie de l'essai.

| Cible | Application | Mode | Version essayée | Guide |
|---|---|---|---|---|
| `gimp` | GIMP | `tcp` | 3.0.4, démarré sans son interface | [docs/fr-FR/modules/gimp.md](modules/gimp.md) |
| `krita` | Krita | `tcp` | 5.2.9, pas démarré depuis Scripter | [docs/fr-FR/modules/krita.md](modules/krita.md) |
| `blender` | Blender | `tcp` | 4.3.2 | [docs/fr-FR/modules/blender.md](modules/blender.md) |
| `inkscape` | Inkscape | `cli` | 1.4 | [docs/fr-FR/modules/inkscape.md](modules/inkscape.md) |
| `libreoffice-writer`, `libreoffice-calc`, `libreoffice-impress` | LibreOffice | `uno` | 24.2.7.2 et 25.2.3.2 | [docs/fr-FR/modules/libreoffice.md](modules/libreoffice.md) |

Les modes : `tcp`, un pont chargé dans l'application écoute sur un port local ; `uno`, un pont autonome se
connecte au port UNO sur lequel écoute LibreOffice ; `cli`, un processus exécute chaque script.

Dans les modes `tcp` et `uno`, le Hub ne démarre, ne ferme ni n'arrête jamais une application à la demande du
modèle : les ponts et les applications démarrent sur un geste explicite de l'opérateur, décrit dans chaque
guide, et `libremodular-launch libreoffice` démarre LibreOffice et son pont (les autres applications se
démarrent à la main). Dans le mode `cli`, le Hub démarre un processus Python par script, et lance `inkscape`
une fois pour exporter une capture ; il ne touche jamais une fenêtre Inkscape que l'opérateur a ouverte.

## Exemple

Un script Calc, que le pont de LibreOffice exécute avec `doc` (le document au premier plan, ou `None`) et `desktop` déjà définis, qui crée le document quand aucun n'est ouvert, écrit deux cellules et rend ce qu'il a écrit, et
la réponse du Hub. Il a tourné tel quel, sans document ouvert, dans LibreOffice 24.2.7.2 (les paquets d'Ubuntu
24.04, avec une fenêtre sur un affichage X virtuel), par le vrai `libremodular-mcp`. Le même exemple est dans
[docs/fr-FR/modules/libreoffice.md](modules/libreoffice.md).

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

## Ce qui quitte la machine

Tout ce que le Hub rend va au modèle de langage, donc à celui qui l'exécute.

| Quoi | Quitte la machine |
|---|---|
| Le script que le modèle a écrit | Oui : le modèle en est l'auteur |
| `stdout`, `stderr` et `result` d'un script | Oui : ils peuvent contenir le contenu d'un document |
| La trace d'erreur | Oui : elle contient des chemins de la machine |
| `state` | Oui : il contient du contenu pris dans les documents ouverts |
| Le chemin d'une capture | Oui : il fait partie de la réponse |
| L'image d'une capture | Seulement si le modèle lit le fichier |
| Le journal d'audit, le jeton et la configuration | Jamais |

Un utilisateur qui ouvre un document confidentiel dans une application connectée sait que son contenu peut
être lu par le modèle.

Le Hub n'ouvre aucune connexion autre que celles vers les ponts sur `127.0.0.1` : un test lit le code source
à la recherche de toute autre (`cargo test --locked --test invariants`).

## Limites

- Le Hub n'est pas un bac à sable. Un script s'exécute avec les droits de l'utilisateur, dans une application
  dont le Python peut faire tout ce que fait l'utilisateur.
- Le garde-fou de script arrête un script maladroit, pas un script déterminé. Il lit le texte d'un script
  avant son envoi ; il ne voit pas `runpy`, `pickle`, `http.server`, les fonctions d'enregistrement des
  applications, ni un chemin passé en paramètre à une fonction.
- Le port UNO de LibreOffice n'a aucune authentification : tout processus qui peut l'atteindre sur cette
  machine pilote LibreOffice sans le jeton, sans le garde-fou ni le journal d'audit.
- Un autre processus du même utilisateur peut lire le fichier de jeton.
- Dans les modes `tcp` et `uno`, un script qui boucle garde l'application (pour LibreOffice, son pont)
  occupée jusqu'à sa fermeture : le Hub répond `timeout`, puis `busy`, et ne l'arrête pas. Seul le mode `cli`
  tue un script qui dépasse son délai.
- `libremodular-launch` refuse un fichier de pont que le groupe ou les autres peuvent écrire, ou qui appartient à un autre utilisateur que vous et root. Un clone fait
  avec un `umask` de `002` donne le mode 664 : lancez `chmod 600` sur le fichier de pont une fois. Les modes
  600 et 644 sont acceptés ; les fichiers de pont des paquets appartiennent à root avec le mode 644.

## Tests

```sh
cargo test --locked
python3 -m unittest discover -s tests/bridges
python3 scripts/build_bridges.py --check
```

Les tests qui exigent une vraie application sont marqués `#[ignore]` avec leur raison et se lancent
volontairement, par exemple `cargo test --locked --test libreoffice_real -- --ignored --nocapture`.

## Crédits

LibreModular-MCP est construit dans une collaboration asymétrique humain–LLM. Les rôles sont écrits tels qu'ils
ont été réellement tenus ; la liste complète est dans [CONTRIBUTORS.md](../../CONTRIBUTORS.md).

- **Ronan Davalan** : architecte et arbitre. Initiative, priorités, exigences de sécurité, validation et essais.
- **Claude Code (Anthropic)** : ingénieur systèmes et développeur principal. Cadre de travail, relecture et intégration du code, documentation.
- **Claude Cloud (Anthropic)** : a écrit le code sur des branches à partir de consignes, relu avant chaque fusion.
- **Gemini (Google)** : conseiller d'architecture et synthétiseur ; a écrit la note fondatrice et proposé le nom du projet.
- **Muse Spark 1.3 (Meta)** : graphiste ; a dessiné le logo et les favicons.

## Sécurité et licence

Signalez une vulnérabilité comme le dit [SECURITY.md](../../SECURITY.md). Le code est sous licence MIT, voir
[LICENSE](../../LICENSE).
