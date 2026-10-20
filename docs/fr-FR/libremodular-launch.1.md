---
title: LIBREMODULAR-LAUNCH
section: 1
header: LibreModular-MCP
---

# NOM

libremodular-launch - démarre LibreOffice et son pont LibreModular-MCP

# SYNOPSIS

**libremodular-launch libreoffice --bridge-file** *chemin*

**libremodular-launch libreoffice --stop**

**libremodular-launch** {**-h**|**--help**}

# DESCRIPTION

**libremodular-launch libreoffice** démarre LibreOffice à l'écoute d'UNO sur 127.0.0.1, puis le pont qui le relie au Hub, et rend la main. Les deux processus continuent de tourner après le retour de la
commande. La liste d'arguments de chacun est fixée par le programme ; le jeton parvient au pont sous la forme du chemin de son fichier, jamais comme une valeur.

Les ports viennent des cibles **libreoffice-writer**, **libreoffice-calc** et **libreoffice-impress** activées dans la configuration (**uno_port** et **bridge_port**). Un seul LibreOffice et un seul pont les
servent toutes ; les cibles activées doivent donc nommer les mêmes ports.

La commande refuse de démarrer quand un LibreOffice du même utilisateur tourne déjà : elle ne se rattache jamais à une instance qui peut contenir un travail non enregistré, et ne ferme jamais une instance
qu'elle n'a pas démarrée. Elle refuse aussi de démarrer un second pont tant que le pont d'un lancement précédent tourne encore.

Les autres applications n'ont pas de lanceur : démarrez leur pont à la main, comme leur guide le dit. **libremodular-launch gimp**, **krita**, **blender** et **inkscape** répondent par ce rappel et sortent
avec le code 2.

# OPTIONS

**--bridge-file** *chemin*
:   Le chemin absolu de *uno_bridge.py*. Le fichier doit être un fichier ordinaire, non un lien, appartenir à l'utilisateur courant ou à root, et n'être inscriptible ni par le groupe ni par les autres. Avec
    le paquet Debian, c'est */usr/share/libremodular-mcp/bridges/uno_bridge.py* ; dans un clone des sources, c'est *bridges/generated/uno_bridge.py*, dont le mode 664 doit d'abord être changé, par exemple
    avec **chmod 600**.

**--stop**
:   Met fin au pont démarré par un lancement précédent, après avoir vérifié que le processus enregistré est bien ce pont. LibreOffice reste ouvert : le fermer vous revient.

**-h**, **--help**
:   Affiche l'usage, puis quitte.

# FICHIERS

*$XDG_RUNTIME_DIR/libremodular-mcp/launch/libreoffice.pids*
:   Les processus du dernier lancement, mode 600.

*$XDG_CONFIG_HOME/libremodular-mcp/config.toml*, *$XDG_CONFIG_HOME/libremodular-mcp/token*
:   Créés par **libremodular-mcp init**(1).

# CODES DE SORTIE

**0**
:   Terminé.

**1**
:   Un échec : une configuration illisible, aucune **XDG_RUNTIME_DIR**, un programme qui n'a pas démarré.

**2**
:   Un mauvais usage, ou un refus que vous pouvez corriger : aucune cible LibreOffice activée, des ports qui diffèrent ou sont égaux, un fichier de pont auquel on ne peut pas se fier, un LibreOffice ou un
    pont déjà en cours d'exécution.

# EXEMPLES

Démarrer LibreOffice et son pont depuis le paquet Debian, puis arrêter le pont :

```sh
libremodular-launch libreoffice --bridge-file /usr/share/libremodular-mcp/bridges/uno_bridge.py
libremodular-launch libreoffice --stop
```

# VOIR AUSSI

**libremodular-mcp**(1)
