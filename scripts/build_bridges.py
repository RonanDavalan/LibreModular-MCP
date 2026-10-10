"""Assemble chaque pont à partir du cœur commun et d'un adaptateur.

Pourquoi ce module existe
Une application charge un seul fichier, alors que le code du protocole et de la sécurité ne doit
exister qu'une fois. Ce script joint bridges/_core.py et bridges/adapters/<name>.py en
bridges/generated/<name>_bridge.py, pour que les copies ne puissent jamais diverger.

Usage : python3 scripts/build_bridges.py [--check]
"""

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

HEADER = (
    "# Généré par scripts/build_bridges.py à partir de bridges/_core.py et bridges/adapters/{name}.py.\n"
    "# Ne pas modifier : changer ces fichiers puis lancer python3 scripts/build_bridges.py.\n"
    "# Ne contient aucun jeton : le pont le lit dans LIBREMODULAR_TOKEN_PATH ou au chemin par défaut.\n"
    "\n"
)

FOOTER = (
    "\n"
    "# --- point d'entrée, posé par scripts/build_bridges.py -----------------------\n"
    "\n"
    "launch(ADAPTER(), DEFAULT_PORT)\n"
)

SEPARATOR = "\n# --- adaptateur : bridges/adapters/{name}.py -----------------------------------\n\n"


def render(name, core_text, adapter_text):
    """Le texte du pont généré de l'adaptateur `name`."""
    return (
        HEADER.format(name=name)
        + core_text.rstrip("\n")
        + "\n"
        + SEPARATOR.format(name=name)
        + adapter_text.rstrip("\n")
        + "\n"
        + FOOTER
    )


def adapters(root):
    """Les noms des adaptateurs, dans un ordre stable."""
    folder = root / "bridges" / "adapters"
    return sorted(path.stem for path in folder.glob("*.py") if not path.stem.startswith("_"))


def build(root, check_only=False):
    """Écrit chaque pont généré dont le texte a changé ; rend les chemins changés.
    Avec `check_only`, rien n'est écrit."""
    core_text = (root / "bridges" / "_core.py").read_text(encoding="utf-8")
    generated = root / "bridges" / "generated"
    changed = []
    for name in adapters(root):
        adapter_text = (root / "bridges" / "adapters" / (name + ".py")).read_text(
            encoding="utf-8"
        )
        text = render(name, core_text, adapter_text)
        target = generated / (name + "_bridge.py")
        current = target.read_text(encoding="utf-8") if target.exists() else None
        if current == text:
            continue
        changed.append(target)
        if not check_only:
            generated.mkdir(exist_ok=True)
            with open(target, "w", encoding="utf-8", newline="\n") as handle:
                handle.write(text)
    return changed


def main(arguments):
    check_only = arguments == ["--check"]
    if arguments and not check_only:
        sys.stderr.write("usage: python3 scripts/build_bridges.py [--check]\n")
        return 2
    changed = build(ROOT, check_only)
    for path in changed:
        verb = "out of date" if check_only else "written"
        sys.stderr.write("%s: %s\n" % (verb, path.relative_to(ROOT)))
    return 1 if check_only and changed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
