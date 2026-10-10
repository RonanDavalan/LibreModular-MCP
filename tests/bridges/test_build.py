"""Tests de scripts/build_bridges.py et des fichiers qu'il génère.

Pourquoi ce module existe
Une application charge le fichier généré, jamais les sources : il doit être exactement le cœur suivi de
l'adaptateur, se charger sur le plus ancien Python pris en charge et ne contenir aucun secret.
Chaque contrôle est montré en échec sur un fichier qui le casse.
"""

import ast
import os
import re
import shutil
import subprocess
import sys
import unittest

from bridge_support import BUILD_SCRIPT, GENERATED, ROOT, PrivateDir, load_text

BUILD = load_text(BUILD_SCRIPT.read_text(encoding="utf-8"), "lmm_build_bridges", str(BUILD_SCRIPT))
HEX_SECRET = re.compile(r"[0-9a-fA-F]{64}")


def copy_tree(directory):
    """Une copie des parties du dépôt que le script de construction lit et écrit."""
    root = directory.path / "repository"
    shutil.copytree(ROOT / "bridges", root / "bridges", ignore=shutil.ignore_patterns("__pycache__"))
    shutil.copytree(ROOT / "scripts", root / "scripts", ignore=shutil.ignore_patterns("__pycache__"))
    return root


def snapshot(root):
    found = {}
    for folder, _, names in os.walk(root):
        for name in names:
            path = os.path.join(folder, name)
            with open(path, "rb") as handle:
                found[os.path.relpath(path, root)] = handle.read()
    return found


def generated_files(root):
    return sorted((root / "bridges" / "generated").glob("*_bridge.py"))


def check_up_to_date(root):
    stale = BUILD.build(root, check_only=True)
    return ["out of date: %s" % path.name for path in stale]


def check_regeneration_changes_nothing(root):
    before = snapshot(root)
    completed = subprocess.run(
        [sys.executable, str(root / "scripts" / "build_bridges.py")],
        capture_output=True,
        text=True,
        timeout=60,
        env={"PATH": os.environ.get("PATH", ""), "PYTHONDONTWRITEBYTECODE": "1"},
    )
    problems = []
    if completed.returncode != 0:
        problems.append("the build failed: %s" % completed.stderr[-400:])
    after = snapshot(root)
    changed = sorted(name for name in set(before) | set(after) if before.get(name) != after.get(name))
    if changed:
        problems.append("regeneration changed %s" % changed)
    return problems


def check_python39(text, name):
    try:
        ast.parse(text, filename=name, feature_version=(3, 9))
        compile(text, name, "exec", dont_inherit=True)
    except SyntaxError as error:
        return ["%s does not compile for Python 3.9: %s" % (name, error)]
    return []


def check_no_secret(text, name):
    problems = []
    if HEX_SECRET.search(text):
        problems.append("%s holds a 64-character hexadecimal value" % name)
    if "test-token" in text:
        problems.append("%s holds a test token" % name)
    return problems


class GeneratedFileTests(unittest.TestCase):
    def test_there_is_a_generated_bridge_for_gimp(self):
        names = [path.name for path in generated_files(ROOT)]
        self.assertIn("gimp_bridge.py", names)

    def test_the_generated_bridges_are_up_to_date(self):
        self.assertEqual(check_up_to_date(ROOT), [])

    def test_counter_proof_a_stale_generated_bridge_is_detected(self):
        directory = PrivateDir()
        try:
            root = copy_tree(directory)
            adapter = root / "bridges" / "adapters" / "gimp.py"
            adapter.write_text(adapter.read_text(encoding="utf-8") + "\n# edited\n", encoding="utf-8")
            self.assertNotEqual(check_up_to_date(root), [])
        finally:
            directory.cleanup()

    def test_regenerating_changes_no_byte(self):
        directory = PrivateDir()
        try:
            self.assertEqual(check_regeneration_changes_nothing(copy_tree(directory)), [])
        finally:
            directory.cleanup()

    def test_counter_proof_a_hand_edited_generated_bridge_is_rewritten(self):
        directory = PrivateDir()
        try:
            root = copy_tree(directory)
            target = root / "bridges" / "generated" / "gimp_bridge.py"
            target.write_text(target.read_text(encoding="utf-8").replace("9301", "9399"), encoding="utf-8")
            self.assertNotEqual(check_regeneration_changes_nothing(root), [])
        finally:
            directory.cleanup()

    def test_the_build_script_check_mode_agrees(self):
        completed = subprocess.run(
            [sys.executable, str(BUILD_SCRIPT), "--check"],
            capture_output=True,
            text=True,
            timeout=60,
            env={"PATH": os.environ.get("PATH", ""), "PYTHONDONTWRITEBYTECODE": "1"},
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_every_bridge_file_compiles_for_python_3_9(self):
        paths = generated_files(ROOT) + [ROOT / "bridges" / "_core.py", BUILD_SCRIPT]
        paths += sorted((ROOT / "bridges" / "adapters").glob("*.py"))
        for path in paths:
            self.assertEqual(check_python39(path.read_text(encoding="utf-8"), path.name), [])

    def test_counter_proof_syntax_newer_than_3_9_is_detected(self):
        newer = "match command:\n    case 'go':\n        pass\n"
        self.assertNotEqual(check_python39(newer, "newer.py"), [])

    def test_the_generated_bridges_say_what_generated_them_and_hold_no_token(self):
        for path in generated_files(ROOT):
            text = path.read_text(encoding="utf-8")
            self.assertTrue(
                text.startswith("# Généré par scripts/build_bridges.py à partir de bridges/_core.py"),
                path.name,
            )
            self.assertEqual(check_no_secret(text, path.name), [])

    def test_counter_proof_a_token_written_in_a_bridge_is_detected(self):
        text = (GENERATED / "gimp_bridge.py").read_text(encoding="utf-8")
        self.assertNotEqual(check_no_secret(text + "\nTOKEN = '%s'\n" % ("ab" * 32), "x"), [])
        self.assertNotEqual(check_no_secret(text + "\nTOKEN = 'test-token-0000'\n", "x"), [])


if __name__ == "__main__":
    unittest.main()
