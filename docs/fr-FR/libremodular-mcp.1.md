---
title: LIBREMODULAR-MCP
section: 1
header: LibreModular-MCP
---

# NOM

libremodular-mcp - serveur MCP qui permet à un modèle de langage de piloter des applications de bureau en y exécutant des scripts

# SYNOPSIS

**libremodular-mcp** [**serve**]

**libremodular-mcp init** [**--rotate**]

**libremodular-mcp** {**-h**|**--help**|**-V**|**--version**}

# DESCRIPTION

**libremodular-mcp** est le Hub : un seul binaire qui parle le Model Context Protocol sur son entrée et sa sortie standard, et donne au modèle deux outils, **execute_script** et **get_state**. Le
Hub envoie un script au pont de l'application nommée par *target_app*, et renvoie ce que le script a affiché, son résultat, ou la trace d'erreur de l'application elle-même. Un pont est un fichier Python
qui s'exécute dans l'application ou à côté d'elle ; le Hub n'en démarre aucun, sauf le pont de LibreOffice que **libremodular-launch**(1) démarre à la demande.

Un client MCP démarre le Hub avec l'argument **serve**. Aucune application n'est activée tant que vous n'avez pas mis **enabled = true** pour elle dans le fichier de configuration.

# COMMANDES

**serve**
:   Sert MCP sur l'entrée et la sortie standard. C'est la valeur par défaut quand aucune commande n'est donnée. Le Hub refuse de démarrer quand le jeton n'existe pas, quand ses permissions sont trop larges,
    ou quand **XDG_RUNTIME_DIR** n'est pas définie.

**init**
:   Crée le répertoire de configuration, une configuration d'exemple où chaque application est désactivée, et le jeton que les ponts vérifient. À lancer une seule fois. Elle n'écrase jamais la
    configuration et conserve un jeton existant.

# OPTIONS

**--rotate**
:   Avec **init** : remplace le jeton existant. Chaque pont en cours d'exécution détient encore l'ancien jeton et doit être redémarré.

**-h**, **--help**
:   Affiche l'aide, puis quitte.

**-V**, **--version**
:   Affiche *libremodular-mcp* suivi de la version, puis quitte.

# FICHIERS

*$XDG_CONFIG_HOME/libremodular-mcp/config.toml*
:   La configuration, mode 600. L'emplacement par défaut est *~/.config/libremodular-mcp/config.toml*.

*$XDG_CONFIG_HOME/libremodular-mcp/token*
:   Le jeton des ponts, mode 600. Il n'est jamais affiché.

*$XDG_STATE_HOME/libremodular-mcp/audit.jsonl*
:   Le journal d'audit, une ligne par appel d'outil, mode 600. L'emplacement par défaut est *~/.local/state/libremodular-mcp/audit.jsonl*.

*$XDG_RUNTIME_DIR/libremodular-mcp/*
:   Fichiers volatils : le répertoire de travail de chaque script et les captures, mode 700.

*/usr/share/libremodular-mcp/bridges/*
:   Les ponts installés par le paquet Debian : *gimp_bridge.py*, *krita_bridge.py*, *blender_bridge.py* et *uno_bridge.py*, qui appartiennent à root et sont lisibles par tous. Le guide de chaque application,
    dans */usr/share/doc/libremodular-mcp/docs/modules/*, dit comment démarrer son pont.

*/usr/share/doc/libremodular-mcp/*
:   Le README et les guides, dont *docs/GUIDE_LLM.md*, qu'un modèle lit avant d'utiliser les outils.

# ENVIRONNEMENT

**XDG_RUNTIME_DIR**
:   Exigée par **serve**. Une session de bureau la définit.

**XDG_CONFIG_HOME**, **XDG_STATE_HOME**
:   Répertoires de base de la configuration et du journal. Quand elles ne sont pas définies, **HOME** donne *~/.config* et *~/.local/state*.

# CODES DE SORTIE

**0**
:   Succès.

**1**
:   Un échec : aucun répertoire de base utilisable, un jeton absent ou non sûr, un fichier impossible à écrire.

**2**
:   Une ligne de commande incorrecte.

# EXEMPLES

Créer la configuration et le jeton :

```sh
libremodular-mcp init
```

Déclarer le Hub dans un client MCP : la commande est */usr/bin/libremodular-mcp* avec l'argument **serve**.

# DISPONIBILITÉ

Le paquet Debian prend en charge Debian 12 et 13 et Ubuntu 22.04, 24.04 et 26.04, en amd64 ; son installation et son retrait ont été validés en conteneur. Installez-le avec :

```sh
sudo apt install ./libremodular-mcp_*_amd64.deb
```

# VOIR AUSSI

**libremodular-launch**(1)
