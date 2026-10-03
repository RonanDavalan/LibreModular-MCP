"""Mechanical invariants of the code base, replayed on every run.

Why this module exists
----------------------
Security properties that only hold by convention rot quietly. Each test here
reads the code (or runs it) and fails when a rule is broken, and each ships a
counter-proof showing the rule would be noticed.
"""

from __future__ import annotations

import ast
import os
import stat
from datetime import UTC, datetime
from pathlib import Path

from conftest import FAKE_TOKEN, assert_check_fails

from libremodular_mcp import audit, cli, config
from libremodular_mcp.config import Limits

PACKAGE = Path(__file__).resolve().parent.parent / "src" / "libremodular_mcp"


def package_sources() -> dict[str, str]:
    return {p.name: p.read_text() for p in sorted(PACKAGE.glob("*.py"))}


def _call_name(node: ast.Call) -> str:
    func = node.func
    parts: list[str] = []
    while isinstance(func, ast.Attribute):
        parts.append(func.attr)
        func = func.value
    if isinstance(func, ast.Name):
        parts.append(func.id)
    return ".".join(reversed(parts))


def _function(tree: ast.AST, name: str) -> ast.FunctionDef:
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and node.name == name:
            return node
    raise AssertionError(f"function {name} not found")


# --- process hardening precedes any token read ---


def check_hardening_precedes_token_access(source: str, function: str, access: set[str]) -> None:
    body = _function(ast.parse(source), function)
    calls = sorted(
        ((n.lineno, n.col_offset, _call_name(n).split(".")[-1])
         for n in ast.walk(body) if isinstance(n, ast.Call)))
    names = [c[2] for c in calls]
    reads = [i for i, n in enumerate(names) if n in access]
    assert reads, f"{function} never accesses the token"
    hardens = [i for i, n in enumerate(names) if n == "harden_process"]
    assert hardens and hardens[0] < reads[0], f"{function} reads the token before hardening"


DEFECTIVE_LOAD = '''
def load_token(path):
    fd = open_control_file(path, 0)
    harden_process()
    return fd
'''


def test_hardening_precedes_token_access_in_load_token_create_token_and_init():
    sources = package_sources()
    check_hardening_precedes_token_access(
        sources["security.py"], "load_token", {"open_control_file"})
    check_hardening_precedes_token_access(
        sources["security.py"], "create_token", {"open_control_file"})
    check_hardening_precedes_token_access(
        sources["cli.py"], "run_init", {"load_token", "create_token"})


def test_counter_proof_detects_a_late_hardening_call():
    def check(source):
        check_hardening_precedes_token_access(source, "load_token", {"open_control_file"})

    assert_check_fails(check, DEFECTIVE_LOAD)


# --- security logic lives only in security.py ---


def stray_security_logic(source: str) -> list[str]:
    tree = ast.parse(source)
    problems: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Call):
            last = _call_name(node).split(".")[-1]
            if last in {"compare_digest", "realpath", "commonpath", "resolve", "guard_script"}:
                problems.append(f"{last} at line {node.lineno}")
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            names = [a.name for a in node.names] + [getattr(node, "module", None) or ""]
            if "ast" in names:
                problems.append(f"import ast at line {node.lineno}")
        elif isinstance(node, ast.FunctionDef):
            if node.name in {"guard", "resolve_under", "check_token", "compare_token"}:
                problems.append(f"def {node.name} at line {node.lineno}")
        elif isinstance(node, ast.Compare):
            operands = [node.left, *node.comparators]
            ids = [getattr(o, "id", getattr(o, "attr", "")).lower() for o in operands]
            if any("token" in i for i in ids) and any(
                    isinstance(op, (ast.Eq, ast.NotEq)) for op in node.ops):
                problems.append(f"token comparison at line {node.lineno}")
    return problems


def test_no_security_logic_outside_security_py():
    for name, source in package_sources().items():
        if name == "security.py":
            continue
        assert not stray_security_logic(source), (name, stray_security_logic(source))


def test_counter_proof_detects_stray_logic():
    def check(source):
        assert not stray_security_logic(source)

    for defective in (
        "import hmac\nhmac.compare_digest(a, b)\n",
        "import os\nos.path.realpath(p)\n",
        "import ast\n",
        "def f(token, other):\n    return token == other\n",
        "def resolve_under(root, c):\n    return c\n",
    ):
        assert_check_fails(check, defective)


# --- control files are opened only through the permission check ---


def raw_file_writes(source: str) -> list[str]:
    problems = []
    for node in ast.walk(ast.parse(source)):
        if isinstance(node, ast.Call):
            name = _call_name(node)
            if name in {"open", "os.open", "os.fdopen"} or name.split(".")[-1] in {
                    "write_text", "write_bytes", "touch"}:
                problems.append(f"{name} at line {node.lineno}")
    return problems


def test_control_files_are_only_written_through_security_py():
    for name, source in package_sources().items():
        if name == "security.py":
            continue
        assert not raw_file_writes(source), (name, raw_file_writes(source))


def test_counter_proof_detects_a_raw_write():
    def check(source):
        assert not raw_file_writes(source)

    assert_check_fails(check, "open('audit.jsonl', 'a').write('x')\n")
    assert_check_fails(check, "from pathlib import Path\nPath('t').write_text('x')\n")


def test_security_py_opens_files_only_with_nofollow():
    tree = ast.parse(package_sources()["security.py"])
    opens = [n for n in ast.walk(tree) if isinstance(n, ast.Call) and _call_name(n) == "os.open"]
    assert len(opens) == 1
    assert "O_NOFOLLOW" in ast.dump(_function(tree, "open_regular_nofollow"))


# --- every file the Hub creates is private ---


def modes_under(root: Path) -> list[tuple[str, int]]:
    found = []
    for path in [root, *root.rglob("*")]:
        found.append((str(path), stat.S_IMODE(os.lstat(path).st_mode)))
    return found


def test_created_files_and_directories_have_no_group_or_other_bit(tmp_path):
    previous = os.umask(0o000)
    try:
        root = tmp_path / "tree"
        paths = config.resolve_paths({
            "HOME": str(tmp_path),
            "LIBREMODULAR_CONFIG_DIR": str(root / "c"),
            "LIBREMODULAR_STATE_DIR": str(root / "s"),
        })
        os.umask(0o000)
        cli.run_init(paths, rotate=False, out=open(os.devnull, "w"))  # noqa: SIM115
        os.umask(0o000)
        log = audit.AuditLog(paths.audit_file, token=FAKE_TOKEN, limits=Limits(),
                             max_bytes=200, clock=lambda: datetime(2000, 1, 1, tzinfo=UTC))
        for _ in range(8):
            log.record({"tool": "get_state", "target_app": "gimp", "status": "ok"})
        os.umask(0o000)
        cli.run_init(paths, rotate=True, out=open(os.devnull, "w"))  # noqa: SIM115
    finally:
        os.umask(previous)
    leaked = [(p, m) for p, m in modes_under(root) if m & 0o077 and not p == str(root)]
    assert not leaked, leaked
    assert any(p.endswith("audit.jsonl.1") for p, _ in modes_under(root))
