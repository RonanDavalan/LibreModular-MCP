"""Hub-side security primitives: token, private files, paths, script guard, bounds.

Why this module exists
----------------------
A script runs with the user's rights inside an application, and everything the
Hub returns leaves the machine. Keeping every check in one module means a
reviewer can read the whole security surface at once, and no other module can
quietly grow a weaker copy of a check.
"""

from __future__ import annotations

import ast
import errno
import os
import posixpath
import resource
import secrets
import stat
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from pathlib import Path

from libremodular_mcp.config import Limits

TRUNCATION_MARKER = "[truncated]"
TOKEN_HEX_LENGTH = 64
_TOKEN_READ_CAP = 4096
_HEX_DIGITS = frozenset("0123456789abcdefABCDEF")
_FORBIDDEN_CONTROL_FLAGS = os.O_TRUNC


class SecurityError(Exception):
    """A security condition failed or could not be verified."""


# --------------------------------------------------------------------------
# Process hardening
# --------------------------------------------------------------------------


def harden_process() -> None:
    """Disable core dumps and restrict the file creation mask.

    Must run before the token is read so that a crash cannot write it to disk.
    """
    try:
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    except (ValueError, OSError) as exc:
        raise SecurityError("cannot disable core dumps") from exc
    os.umask(0o077)


# --------------------------------------------------------------------------
# Files and directories
# --------------------------------------------------------------------------


def _current_uid(expected_uid: int | None) -> int:
    return os.geteuid() if expected_uid is None else expected_uid


def open_regular_nofollow(path: str | os.PathLike[str], flags: int = os.O_RDONLY,
                          mode: int = 0o600) -> int:
    """Open without following a final symbolic link; refuse anything but a regular file."""
    flags |= os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK
    try:
        fd = os.open(path, flags, mode)
    except OSError as exc:
        if exc.errno == errno.ELOOP:
            raise SecurityError(f"refusing to follow a symbolic link: {path}") from exc
        raise
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise SecurityError(f"not a regular file: {path}")
    except BaseException:
        os.close(fd)
        raise
    return fd


def open_control_file(path: str | os.PathLike[str], flags: int, mode: int = 0o600, *,
                      expected_uid: int | None = None) -> int:
    """Open a token, journal or capture file after checking owner and permissions.

    The file must belong to the expected user and carry no group or other bit.
    """
    if flags & _FORBIDDEN_CONTROL_FLAGS:
        raise ValueError("control files are never truncated on open")
    fd = open_regular_nofollow(path, flags, mode)
    try:
        info = os.fstat(fd)
        if info.st_uid != _current_uid(expected_uid):
            raise SecurityError(f"{path} is not owned by the current user")
        if info.st_mode & 0o077:
            raise SecurityError(
                f"{path} has mode {stat.S_IMODE(info.st_mode):04o}; expected 0600 (chmod 600)"
            )
    except BaseException:
        os.close(fd)
        raise
    return fd


def write_all(fd: int, data: bytes) -> None:
    view = memoryview(data)
    while view:
        view = view[os.write(fd, view):]


def ensure_private_dir(path: str | os.PathLike[str], *, expected_uid: int | None = None) -> Path:
    """Create the directory with mode 700, or verify an existing one is private."""
    target = Path(path)
    try:
        info = os.lstat(target)
    except FileNotFoundError:
        target.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.mkdir(target, 0o700)
        except FileExistsError:
            pass
        info = os.lstat(target)
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise SecurityError(f"not a plain directory: {target}")
    if info.st_uid != _current_uid(expected_uid):
        raise SecurityError(f"{target} is not owned by the current user")
    if info.st_mode & 0o077:
        raise SecurityError(
            f"{target} has mode {stat.S_IMODE(info.st_mode):04o}; expected 0700 (chmod 700)"
        )
    return target


# --------------------------------------------------------------------------
# Token
# --------------------------------------------------------------------------


def load_token(path: str | os.PathLike[str], *, expected_uid: int | None = None) -> str:
    """Read and validate the bridge token (64 hexadecimal characters)."""
    harden_process()
    try:
        fd = open_control_file(path, os.O_RDONLY, expected_uid=expected_uid)
    except OSError as exc:
        raise SecurityError(f"cannot open the token file {path}: {exc.strerror}") from exc
    try:
        data = b""
        while len(data) <= _TOKEN_READ_CAP:
            chunk = os.read(fd, _TOKEN_READ_CAP)
            if not chunk:
                break
            data += chunk
    finally:
        os.close(fd)
    if len(data) > _TOKEN_READ_CAP:
        raise SecurityError("the token file is too large")
    text = data.decode("ascii", errors="replace").rstrip("\n")
    if len(text) != TOKEN_HEX_LENGTH or not set(text) <= _HEX_DIGITS:
        raise SecurityError(
            f"the token file must hold {TOKEN_HEX_LENGTH} hexadecimal characters; "
            "run 'libremodular-mcp init --rotate'"
        )
    return text


def create_token(path: str | os.PathLike[str], rotate: bool = False) -> None:
    """Create the token file (exclusive, mode 600) in a private directory.

    Refuses to replace an existing token unless ``rotate`` is true.
    """
    harden_process()
    target = Path(path)
    ensure_private_dir(target.parent)
    data = secrets.token_hex(32).encode("ascii")
    try:
        if not rotate:
            try:
                fd = open_control_file(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL)
            except FileExistsError as exc:
                raise SecurityError(
                    f"{target} already exists; use rotate to replace the token"
                ) from exc
            try:
                write_all(fd, data)
                os.fsync(fd)
            finally:
                os.close(fd)
            return
        staging = target.with_name(f".{target.name}.{secrets.token_hex(8)}.new")
        fd = open_control_file(staging, os.O_WRONLY | os.O_CREAT | os.O_EXCL)
        try:
            try:
                write_all(fd, data)
                os.fsync(fd)
            finally:
                os.close(fd)
            os.replace(staging, target)
        except BaseException:
            try:
                os.unlink(staging)
            except OSError:
                pass
            raise
    except OSError as exc:
        raise SecurityError(f"cannot write the token file {target}: {exc.strerror}") from exc


# --------------------------------------------------------------------------
# Paths, targets, sizes
# --------------------------------------------------------------------------


def resolve_under(root: str | os.PathLike[str], candidate: str | os.PathLike[str]) -> Path:
    """Resolve symbolic links and require the result to stay inside ``root``."""
    try:
        root_real = os.path.realpath(root)
        text = os.fspath(candidate)
        if not os.path.isabs(text):
            text = os.path.join(root_real, text)
        real = os.path.realpath(text)
        inside = os.path.commonpath([root_real, real]) == root_real
    except ValueError as exc:
        raise SecurityError("invalid path") from exc
    if not inside:
        raise SecurityError(f"path escapes {root_real}: {candidate}")
    return Path(real)


def check_target(name: object, active_names: Iterable[str]) -> str:
    """Accept only an exact, case-sensitive match with an active application name."""
    active = tuple(active_names)
    if isinstance(name, str) and name in active:
        return name
    listing = ", ".join(sorted(active)) if active else "none"
    raise SecurityError(f"unknown or inactive target_app; active applications: {listing}")


def check_script_size(script: str, limits: Limits) -> int:
    """Return the script size in bytes, or refuse when it exceeds the limit."""
    size = len(script.encode("utf-8", errors="surrogatepass"))
    if size > limits.max_script_bytes:
        raise SecurityError(
            f"script is {size} bytes; the limit is {limits.max_script_bytes} bytes"
        )
    return size


def bounded(text: str, limit: int) -> tuple[str, bool]:
    """Cut ``text`` to at most ``limit`` UTF-8 bytes, ending with the truncation marker."""
    raw = text.encode("utf-8", errors="replace")
    if len(raw) <= limit:
        return text, False
    room = limit - len(TRUNCATION_MARKER.encode("ascii"))
    if room < 0:
        raise ValueError("limit is smaller than the truncation marker")
    head = raw[:room].decode("utf-8", errors="ignore")
    return head + TRUNCATION_MARKER, True


# --------------------------------------------------------------------------
# Script guard
# --------------------------------------------------------------------------

CATEGORY_LABELS = {
    "G-1": "file deletion",
    "G-2": "process launch",
    "G-3": "network access",
    "G-4": "write outside the work directory",
    "G-5": "analysis bypass",
}

_G1_EXACT = frozenset({
    "os.remove", "os.unlink", "os.rmdir", "os.removedirs", "shutil.rmtree",
    *(f"pathlib.{cls}.{meth}" for cls in
      ("Path", "PosixPath", "WindowsPath", "PurePath", "PurePosixPath", "PureWindowsPath")
      for meth in ("unlink", "rmdir")),
})
_G2_EXACT = frozenset({"os.system", "os.popen", "os.posix_spawn", "os.posix_spawnp"})
_G2_OS_PREFIXES = ("exec", "spawn")
_G2_MODULES = ("subprocess", "pty")
_G3_MODULES = ("socket", "urllib", "http.client", "ftplib", "smtplib", "requests")
_G5_EXACT = frozenset({"eval", "exec", "compile", "__import__", "__builtins__"})
_G5_MODULES = ("importlib", "ctypes")

_BARE_BUILTINS = _G5_EXACT | {"open", "getattr", "str"}
_PATH_CLASSES = frozenset(
    f"pathlib.{cls}" for cls in
    ("Path", "PosixPath", "WindowsPath", "PurePath", "PurePosixPath", "PureWindowsPath")
)
_PATH_CLASS_FACTORIES = frozenset(f"{c}.{m}" for c in _PATH_CLASSES for m in ("home", "cwd"))
_PATH_RETURNING_METHODS = frozenset({
    "resolve", "absolute", "expanduser", "joinpath", "with_name", "with_suffix", "with_stem",
})
_PASSTHROUGH_CALLS = frozenset({"str", "os.fspath", "os.path.expanduser", "os.path.normpath"})
_WRITE_FLAG_NAMES = frozenset({"O_WRONLY", "O_RDWR", "O_CREAT", "O_APPEND", "O_TRUNC"})
_WRITE_FLAG_MASK = (os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_APPEND | os.O_TRUNC)
_MAX_DEPTH = 12
_MAX_REFUSAL_LINES = 10


@dataclass(frozen=True)
class Finding:
    category: str
    construct: str
    line: int


@dataclass
class GuardReport:
    findings: list[Finding] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    @property
    def refused(self) -> bool:
        return bool(self.findings)

    @property
    def categories(self) -> list[str]:
        return sorted({f.category for f in self.findings})


def _in_modules(name: str, modules: Iterable[str]) -> bool:
    return any(name == m or name.startswith(m + ".") for m in modules)


def _classify(name: str) -> str | None:
    """Return the guard category of a fully qualified name, if it has one."""
    if name in _G1_EXACT:
        return "G-1"
    if name in _G2_EXACT or _in_modules(name, _G2_MODULES):
        return "G-2"
    if name.startswith("os.") and "." not in name[3:] and name[3:].startswith(_G2_OS_PREFIXES):
        return "G-2"
    if _in_modules(name, _G3_MODULES):
        return "G-3"
    if name in _G5_EXACT or _in_modules(name, _G5_MODULES):
        return "G-5"
    return None


def _normalize(name: str) -> str:
    for prefix in ("builtins.", "__builtins__."):
        if name.startswith(prefix):
            return name[len(prefix):]
    return name


def _is_pure_chain(node: ast.expr) -> bool:
    while isinstance(node, ast.Attribute):
        node = node.value
    return isinstance(node, ast.Name)


class _Analyzer:
    def __init__(self, tree: ast.AST, work_dir: str, roots: Sequence[str]) -> None:
        self.work_dir = work_dir
        self.roots = [work_dir, *roots]
        self.findings: dict[tuple[str, str, int], Finding] = {}
        self.warnings: list[str] = []
        self.imports: dict[str, list[str]] = {}
        self.star_modules: list[str] = []
        self.assigned: dict[str, list[ast.expr]] = {}
        self.bindings: dict[str, int] = {}
        self._collect(tree)

    # -- symbol collection ------------------------------------------------

    def _bind(self, name: str) -> None:
        self.bindings[name] = self.bindings.get(name, 0) + 1

    def _collect(self, tree: ast.AST) -> None:
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for alias in node.names:
                    if alias.asname:
                        self.imports.setdefault(alias.asname, []).append(alias.name)
                        self._bind(alias.asname)
                    else:
                        top = alias.name.split(".")[0]
                        self.imports.setdefault(top, []).append(top)
                        self._bind(top)
            elif isinstance(node, ast.ImportFrom):
                for alias in node.names:
                    if alias.name == "*":
                        if node.module and not node.level:
                            self.star_modules.append(node.module)
                        continue
                    bound = alias.asname or alias.name
                    if node.module and not node.level:
                        self.imports.setdefault(bound, []).append(f"{node.module}.{alias.name}")
                    self._bind(bound)
            elif isinstance(node, ast.Assign):
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        self.assigned.setdefault(target.id, []).append(node.value)
            elif isinstance(node, ast.AnnAssign):
                if isinstance(node.target, ast.Name) and node.value is not None:
                    self.assigned.setdefault(node.target.id, []).append(node.value)
            elif isinstance(node, ast.Name) and isinstance(node.ctx, (ast.Store, ast.Del)):
                self._bind(node.id)
            elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                self._bind(node.name)
            elif isinstance(node, ast.arg):
                self._bind(node.arg)
            elif isinstance(node, ast.ExceptHandler) and node.name:
                self._bind(node.name)
            elif isinstance(node, (ast.MatchAs, ast.MatchStar)) and node.name:
                self._bind(node.name)
            elif isinstance(node, ast.MatchMapping) and node.rest:
                self._bind(node.rest)

    # -- reporting --------------------------------------------------------

    def _add(self, category: str, construct: str, line: int) -> None:
        self.findings.setdefault((category, construct, line), Finding(category, construct, line))

    def _warn(self, line: int, message: str) -> None:
        text = f"line {line}: {message}"
        if text not in self.warnings:
            self.warnings.append(text)

    # -- name resolution --------------------------------------------------

    def qualify(self, expr: ast.expr, depth: int = 0,
                seen: frozenset[str] = frozenset()) -> list[str]:
        """Candidate fully qualified names of an expression, aliases resolved."""
        if depth > _MAX_DEPTH:
            return []
        if isinstance(expr, ast.Name):
            name = expr.id
            if name in seen:
                return []
            found = list(self.imports.get(name, []))
            for value in self.assigned.get(name, []):
                found.extend(self.qualify(value, depth + 1, seen | {name}))
            if not found:
                if name in self.bindings:
                    return []
                found = [f"{m}.{name}" for m in self.star_modules] + [name]
            return [_normalize(q) for q in found]
        if isinstance(expr, ast.Attribute):
            return [_normalize(f"{q}.{expr.attr}") for q in self.qualify(expr.value, depth + 1, seen)]
        if (isinstance(expr, ast.Call) and isinstance(expr.func, ast.Name)
                and expr.func.id == "getattr" and "getattr" not in self.bindings
                and len(expr.args) >= 2):
            attr = expr.args[1]
            bases = self.qualify(expr.args[0], depth + 1, seen)
            if isinstance(attr, ast.Constant) and isinstance(attr.value, str):
                return [_normalize(f"{q}.{attr.value}") for q in bases]
            if any(_classify(q + ".x") or _classify(q) for q in bases):
                self._warn(expr.lineno, "dynamic attribute access on a guarded module")
        return []

    def _single_value(self, name: str) -> ast.expr | None:
        values = self.assigned.get(name, [])
        if len(values) == 1 and self.bindings.get(name, 0) == 1:
            return values[0]
        return None

    def literal_path(self, expr: ast.expr, depth: int = 0) -> str | None:
        """The path an expression always evaluates to, when it can be known statically."""
        if depth > _MAX_DEPTH:
            return None
        sub = depth + 1
        if isinstance(expr, ast.Constant):
            return expr.value if isinstance(expr.value, str) else None
        if isinstance(expr, ast.Name):
            value = self._single_value(expr.id)
            if value is not None:
                return self.literal_path(value, sub)
            if expr.id == "LMM_WORK_DIR" and expr.id not in self.bindings:
                return self.work_dir
            return None
        if isinstance(expr, ast.JoinedStr):
            parts: list[str] = []
            for item in expr.values:
                if isinstance(item, ast.Constant):
                    parts.append(str(item.value))
                elif (isinstance(item, ast.FormattedValue) and item.conversion == -1
                      and item.format_spec is None):
                    inner = self.literal_path(item.value, sub)
                    if inner is None:
                        return None
                    parts.append(inner)
                else:
                    return None
            return "".join(parts)
        if isinstance(expr, ast.BinOp) and isinstance(expr.op, (ast.Add, ast.Div)):
            left, right = self.literal_path(expr.left, sub), self.literal_path(expr.right, sub)
            if left is None or right is None:
                return None
            return left + right if isinstance(expr.op, ast.Add) else posixpath.join(left, right)
        if isinstance(expr, ast.Attribute) and expr.attr == "parent":
            inner = self.literal_path(expr.value, sub)
            return None if inner is None else posixpath.dirname(inner)
        if isinstance(expr, ast.Call):
            return self._literal_call(expr, sub)
        return None

    def _literal_call(self, call: ast.Call, depth: int) -> str | None:
        if call.keywords:
            return None
        names = self.qualify(call.func)
        if isinstance(call.func, ast.Attribute) and call.func.attr in (
                "expanduser", "resolve", "absolute") and not call.args:
            inner = self.literal_path(call.func.value, depth)
            if inner is not None:
                return os.path.expanduser(inner) if call.func.attr == "expanduser" else inner
        if isinstance(call.func, ast.Attribute) and call.func.attr == "joinpath":
            base = self.literal_path(call.func.value, depth)
            args = [self.literal_path(a, depth) for a in call.args]
            if base is not None and all(a is not None for a in args):
                return posixpath.join(base, *args)  # type: ignore[arg-type]
            return None
        if len(names) != 1:
            return None
        name = names[0]
        args = [self.literal_path(a, depth) for a in call.args]
        if any(a is None for a in args):
            return None
        strs: list[str] = args  # type: ignore[assignment]
        if name in _PATH_CLASSES:
            return posixpath.join(*strs) if strs else "."
        if name in _PATH_CLASS_FACTORIES and name.endswith(".home"):
            return os.path.expanduser("~")
        if name in ("os.path.join", "posixpath.join") and strs:
            return posixpath.join(*strs)
        if name == "os.path.dirname" and len(strs) == 1:
            return posixpath.dirname(strs[0])
        if name in _PASSTHROUGH_CALLS and len(strs) == 1:
            value = strs[0]
            if name == "os.path.expanduser":
                return os.path.expanduser(value)
            if name == "os.path.normpath":
                return posixpath.normpath(value)
            return value
        return None

    def is_path_expr(self, expr: ast.expr, depth: int = 0) -> bool:
        """Whether the expression is, as far as can be told, a pathlib path object."""
        if depth > _MAX_DEPTH:
            return False
        sub = depth + 1
        if isinstance(expr, ast.Call):
            if any(q in _PATH_CLASSES or q in _PATH_CLASS_FACTORIES
                   for q in self.qualify(expr.func)):
                return True
            func = expr.func
            return (isinstance(func, ast.Attribute) and func.attr in _PATH_RETURNING_METHODS
                    and self.is_path_expr(func.value, sub))
        if isinstance(expr, ast.BinOp) and isinstance(expr.op, ast.Div):
            return self.is_path_expr(expr.left, sub) or self.is_path_expr(expr.right, sub)
        if isinstance(expr, ast.Attribute) and expr.attr == "parent":
            return self.is_path_expr(expr.value, sub)
        if isinstance(expr, ast.Name):
            return any(self.is_path_expr(v, sub) for v in self.assigned.get(expr.id, []))
        return False

    # -- write checks -----------------------------------------------------

    def _check_write_path(self, expr: ast.expr, line: int, construct: str) -> None:
        literal = self.literal_path(expr)
        if literal is None:
            self._warn(line, f"path not statically verifiable ({construct})")
            return
        literal = os.path.expanduser(literal)
        if not os.path.isabs(literal):
            self._warn(line, f"relative path not statically verifiable ({construct}); "
                             "build paths from LMM_WORK_DIR")
            return
        for root in self.roots:
            try:
                resolve_under(root, literal)
                return
            except SecurityError:
                continue
        self._add("G-4", f"{construct} to {literal}", line)

    @staticmethod
    def _arg(call: ast.Call, index: int, keyword: str) -> ast.expr | None:
        if len(call.args) > index and not any(
                isinstance(a, ast.Starred) for a in call.args[: index + 1]):
            return call.args[index]
        for kw in call.keywords:
            if kw.arg == keyword:
                return kw.value
        return None

    @staticmethod
    def _mode_state(mode: ast.expr | None) -> str:
        if mode is None:
            return "read"
        if isinstance(mode, ast.Constant) and isinstance(mode.value, str):
            return "write" if set(mode.value) & set("wax+") else "read"
        return "unknown"

    def _check_open(self, call: ast.Call, path: ast.expr | None, mode: ast.expr | None,
                    construct: str) -> None:
        if path is None:
            return
        state = self._mode_state(mode)
        if state == "read":
            return
        if state == "unknown":
            self._warn(call.lineno, f"open mode not statically verifiable ({construct})")
            return
        self._check_write_path(path, call.lineno, construct)

    def _check_os_open(self, call: ast.Call) -> None:
        path, flags = self._arg(call, 0, "path"), self._arg(call, 1, "flags")
        if path is None or flags is None:
            return
        writes = False
        for node in ast.walk(flags):
            if isinstance(node, ast.Attribute) and node.attr in _WRITE_FLAG_NAMES:
                writes = True
            elif isinstance(node, ast.Constant) and isinstance(node.value, int):
                writes = writes or bool(node.value & _WRITE_FLAG_MASK)
        if writes:
            self._check_write_path(path, call.lineno, "os.open")

    def _check_call(self, call: ast.Call) -> None:
        names = set(self.qualify(call.func))
        for name in names:
            if name in ("open", "io.open", "codecs.open"):
                self._check_open(call, self._arg(call, 0, "file" if name != "codecs.open"
                                                 else "filename"),
                                 self._arg(call, 1, "mode"), "open()")
            elif name == "os.open":
                self._check_os_open(call)
            elif name == "shutil.move":
                for index, key in ((0, "src"), (1, "dst")):
                    arg = self._arg(call, index, key)
                    if arg is not None:
                        self._check_write_path(arg, call.lineno, "shutil.move")
            elif name.startswith("shutil.copy") and name != "shutil.copyfileobj":
                arg = self._arg(call, 1, "dst")
                if arg is not None:
                    self._check_write_path(arg, call.lineno, name)
            elif name in {f"{c}.{m}" for c in _PATH_CLASSES
                          for m in ("write_text", "write_bytes")}:
                target = self._arg(call, 0, "self")
                if target is not None:
                    self._check_write_path(target, call.lineno, name.rsplit(".", 1)[1])
            elif name in {f"{c}.open" for c in _PATH_CLASSES}:
                target = self._arg(call, 0, "self")
                if target is not None:
                    self._check_open(call, target, self._arg(call, 1, "mode"), "Path.open()")
        func = call.func
        if isinstance(func, ast.Attribute) and self.is_path_expr(func.value):
            if func.attr in ("write_text", "write_bytes"):
                self._check_write_path(func.value, call.lineno, func.attr)
            elif func.attr == "open":
                self._check_open(call, func.value, self._arg(call, 0, "mode"), "Path.open()")

    # -- visitors ---------------------------------------------------------

    def _check_module(self, module: str, construct: str, line: int) -> None:
        category = _classify(module)
        if category:
            self._add(category, construct, line)

    def _on_import(self, node: ast.Import) -> None:
        for alias in node.names:
            self._check_module(alias.name, f"import {alias.name}", node.lineno)

    def _on_import_from(self, node: ast.ImportFrom) -> None:
        if node.level or not node.module:
            return
        self._check_module(node.module, f"from {node.module} import ...", node.lineno)
        for alias in node.names:
            if alias.name != "*":
                self._check_module(f"{node.module}.{alias.name}",
                                   _normalize(f"{node.module}.{alias.name}"), node.lineno)

    def _check_expr(self, node: ast.expr) -> None:
        hit = False
        for qualified in self.qualify(node):
            category = _classify(qualified)
            if category:
                self._add(category, qualified, node.lineno)
                hit = True
        if isinstance(node, ast.Attribute):
            if node.attr == "unlink" and self.is_path_expr(node.value):
                self._add("G-1", "Path.unlink", node.lineno)
            elif node.attr == "rmdir" and not hit:
                self._add("G-1", ".rmdir", node.lineno)

    def _on_name(self, node: ast.Name) -> None:
        if not isinstance(node.ctx, ast.Load):
            return
        for qualified in self.qualify(node):
            if (qualified == node.id and node.id not in self.imports
                    and qualified not in _BARE_BUILTINS):
                continue
            category = _classify(qualified)
            if category:
                self._add(category, qualified, node.lineno)

    def _on_attribute(self, node: ast.Attribute) -> None:
        if isinstance(node.ctx, ast.Load):
            self._check_expr(node)

    def _on_call(self, node: ast.Call) -> None:
        self._check_expr(node)
        self._check_call(node)

    def run(self, tree: ast.AST) -> None:
        """Visit every node iteratively, so deeply nested scripts cannot exhaust the stack.

        The inner nodes of a plain dotted chain are skipped: the outermost one is
        classified once, with its fully qualified name.
        """
        nodes = list(ast.walk(tree))
        inner: set[int] = set()
        for node in nodes:
            if isinstance(node, ast.Attribute) and _is_pure_chain(node):
                part = node.value
                while True:
                    inner.add(id(part))
                    if not isinstance(part, ast.Attribute):
                        break
                    part = part.value
        handlers = {
            ast.Import: self._on_import,
            ast.ImportFrom: self._on_import_from,
            ast.Name: self._on_name,
            ast.Attribute: self._on_attribute,
            ast.Call: self._on_call,
        }
        for node in nodes:
            handler = handlers.get(type(node))
            if handler is not None and id(node) not in inner:
                handler(node)


def guard_script(script: str, work_dir: str | os.PathLike[str],
                 extra_write_roots: Iterable[str | os.PathLike[str]] = ()) -> GuardReport:
    """Statically analyse a Python script; nothing is executed."""
    report = GuardReport()
    try:
        tree = ast.parse(script, filename="<libremodular>")
    except (SyntaxError, ValueError):
        report.warnings.append("syntax error not analysed")
        return report
    except RecursionError:
        report.findings.append(Finding("G-5", "nesting too deep to analyse", 0))
        return report
    try:
        analyzer = _Analyzer(tree, os.fspath(work_dir), [os.fspath(r) for r in extra_write_roots])
        analyzer.run(tree)
    except RecursionError:
        report.findings.append(Finding("G-5", "nesting too deep to analyse", 0))
        return report
    report.findings = sorted(analyzer.findings.values(),
                             key=lambda f: (f.line, f.category, f.construct))
    report.warnings = analyzer.warnings
    return report


def format_refusal(report: GuardReport) -> str:
    """Message for a refused script: category, construct, line, and how to allow it."""
    lines = ["Script refused by the safety guard before reaching the application:"]
    for finding in report.findings[:_MAX_REFUSAL_LINES]:
        label = CATEGORY_LABELS.get(finding.category, "")
        lines.append(f"  {finding.category} ({label}) at line {finding.line}: {finding.construct}")
    hidden = len(report.findings) - _MAX_REFUSAL_LINES
    if hidden > 0:
        lines.append(f"  ... and {hidden} more")
    lines.append(
        "The guard is a safety net, not a sandbox. Write files under LMM_WORK_DIR or a directory "
        "listed in extra_write_roots. To allow these constructs the operator sets "
        "allow_dangerous_scripts = true in the [security] section of config.toml; "
        "the audit log then records guard: bypassed."
    )
    return "\n".join(lines)
