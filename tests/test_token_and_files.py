"""Token, private file and path confinement tests.

Why this module exists
----------------------
A weak permission check would silently expose the bridge token.
"""

from __future__ import annotations

import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest
from conftest import FILE_TOKEN, assert_check_fails, fresh

from libremodular_mcp import security
from libremodular_mcp.security import SecurityError


def _write(path: Path, mode: int, text: str = FILE_TOKEN) -> None:
    path.write_text(text)
    os.chmod(path, mode)


def check_refuses_loose_modes(load, tmp_path: Path) -> None:
    tmp_path = fresh(tmp_path)
    for mode in (0o640, 0o644, 0o660, 0o604, 0o606):
        path = tmp_path / f"token-{mode:o}"
        _write(path, mode)
        try:
            load(path)
        except SecurityError:
            continue
        raise AssertionError(f"mode {mode:o} was accepted")


def check_accepts_private_token(load, tmp_path: Path) -> None:
    tmp_path = fresh(tmp_path)
    path = tmp_path / "token-ok"
    _write(path, 0o600)
    assert load(path) == FILE_TOKEN


def check_refuses_symlink(load, tmp_path: Path) -> None:
    tmp_path = fresh(tmp_path)
    real = tmp_path / "real-token"
    _write(real, 0o600)
    link = tmp_path / "link-token"
    link.symlink_to(real)
    try:
        load(link)
    except SecurityError:
        return
    raise AssertionError("symbolic link was followed")


def check_refuses_foreign_owner(load, tmp_path: Path) -> None:
    tmp_path = fresh(tmp_path)
    path = tmp_path / "token-owner"
    _write(path, 0o600)
    try:
        load(path, expected_uid=os.geteuid() + 1)
    except SecurityError:
        return
    raise AssertionError("foreign owner was accepted")


def check_refuses_malformed(load, tmp_path: Path) -> None:
    tmp_path = fresh(tmp_path)
    for index, text in enumerate(("", "abc", "zz" * 32, FILE_TOKEN + "00")):
        path = tmp_path / f"token-bad-{index}"
        _write(path, 0o600, text)
        try:
            load(path)
        except SecurityError:
            continue
        raise AssertionError(f"malformed token {index} was accepted")


def lenient_load(path, *, expected_uid=None):
    return Path(path).read_text().strip()


@pytest.mark.parametrize(
    "check",
    [check_refuses_loose_modes, check_refuses_symlink, check_refuses_foreign_owner,
     check_refuses_malformed],
)
def test_load_token_refusals_and_their_counter_proof(check, tmp_path):
    check(security.load_token, tmp_path)
    assert_check_fails(check, lenient_load, tmp_path)


def test_load_token_accepts_a_private_token(tmp_path):
    check_accepts_private_token(security.load_token, tmp_path)


def test_load_token_error_never_contains_the_token(tmp_path):
    path = tmp_path / "token"
    _write(path, 0o644)
    with pytest.raises(SecurityError) as info:
        security.load_token(path)
    assert FILE_TOKEN not in str(info.value)


def test_load_token_disables_core_dumps_in_a_fresh_process(token_file):
    code = textwrap.dedent(
        f"""
        import resource
        from libremodular_mcp.security import load_token
        hard = resource.getrlimit(resource.RLIMIT_CORE)[1]
        resource.setrlimit(resource.RLIMIT_CORE, (hard, hard))
        before = resource.getrlimit(resource.RLIMIT_CORE)
        load_token({str(token_file)!r})
        print(before[0] == hard, resource.getrlimit(resource.RLIMIT_CORE))
        """
    )
    result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True,
                            env={**os.environ, "PYTHONPATH": "src"}, check=True)
    assert result.stdout.strip().endswith("(0, 0)")


# --- create_token --------------------------------------------------------


def check_create_does_not_overwrite(create, tmp_path: Path) -> None:
    tmp_path = fresh(tmp_path)
    path = tmp_path / "cfg" / "token"
    create(path, False)
    first = path.read_text()
    try:
        create(path, False)
    except SecurityError:
        pass
    assert path.read_text() == first, "existing token was overwritten"


def check_rotate_replaces_with_mode_600(create, tmp_path: Path) -> None:
    tmp_path = fresh(tmp_path)
    path = tmp_path / "cfg2" / "token"
    create(path, False)
    first = path.read_text()
    create(path, True)
    assert path.read_text() != first
    assert (path.stat().st_mode & 0o777) == 0o600
    assert (path.parent.stat().st_mode & 0o777) == 0o700
    assert len(path.read_text()) == 64


def naive_create(path, rotate):
    path = Path(path)
    path.parent.mkdir(mode=0o700, exist_ok=True)
    path.write_text(os.urandom(32).hex())
    os.chmod(path, 0o644)


def test_create_token_never_overwrites_without_rotate(tmp_path):
    check_create_does_not_overwrite(security.create_token, tmp_path)
    assert_check_fails(check_create_does_not_overwrite, naive_create, tmp_path)


def test_create_token_rotate_replaces_and_stays_private(tmp_path):
    check_rotate_replaces_with_mode_600(security.create_token, tmp_path)
    assert_check_fails(check_rotate_replaces_with_mode_600, naive_create, tmp_path)


def test_created_token_loads_back(tmp_path):
    path = tmp_path / "cfg" / "token"
    security.create_token(path)
    assert len(security.load_token(path)) == 64


def test_ensure_private_dir_refuses_wide_permissions(tmp_path):
    wide = tmp_path / "wide"
    wide.mkdir()
    os.chmod(wide, 0o755)
    with pytest.raises(SecurityError):
        security.ensure_private_dir(wide)
    created = security.ensure_private_dir(tmp_path / "new")
    assert (created.stat().st_mode & 0o777) == 0o700


def test_ensure_private_dir_refuses_a_symlink(tmp_path):
    target = tmp_path / "target"
    target.mkdir(mode=0o700)
    link = tmp_path / "link"
    link.symlink_to(target)
    with pytest.raises(SecurityError):
        security.ensure_private_dir(link)


def test_open_regular_nofollow_refuses_links_and_directories(tmp_path):
    real = tmp_path / "f"
    real.write_text("x")
    link = tmp_path / "l"
    link.symlink_to(real)
    with pytest.raises(SecurityError):
        security.open_regular_nofollow(link)
    with pytest.raises(SecurityError):
        security.open_regular_nofollow(tmp_path)


# --- resolve_under -------------------------------------------------------


def check_resolve_confines(resolve, tmp_path: Path) -> None:
    tmp_path = fresh(tmp_path)
    root = tmp_path / "root"
    root.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (root / "escape").symlink_to(outside)
    (root / "ok.txt").write_text("x")
    assert Path(resolve(root, "ok.txt")) == (root / "ok.txt").resolve()
    for bad in ("../outside/x", "escape/x", str(outside / "x"), "sub/../../outside"):
        try:
            resolve(root, bad)
        except SecurityError:
            continue
        raise AssertionError(f"{bad} escaped the root")


def naive_resolve(root, candidate):
    return Path(root) / candidate


def test_resolve_under_confines_paths_and_has_a_counter_proof(tmp_path):
    check_resolve_confines(security.resolve_under, tmp_path)
    assert_check_fails(check_resolve_confines, naive_resolve, tmp_path)


# --- targets, sizes, bounds ----------------------------------------------


def test_check_target_is_exact(limits):
    active = ["gimp", "libreoffice-calc"]
    assert security.check_target("gimp", active) == "gimp"
    for bad in ("GIMP", " gimp", "gimp ", "gimp\n", "libreoffice", "", None, "hub"):
        with pytest.raises(SecurityError) as info:
            security.check_target(bad, active)
        assert "gimp" in str(info.value) and "libreoffice-calc" in str(info.value)


def test_check_script_size_uses_bytes(limits):
    small = limits.__class__(max_script_bytes=1024)
    assert security.check_script_size("a" * 1024, small) == 1024
    with pytest.raises(SecurityError):
        security.check_script_size("é" * 600, small)


def test_bounded_marks_truncation_and_respects_the_limit():
    text, cut = security.bounded("é" * 100, 50)
    assert cut and text.endswith(security.TRUNCATION_MARKER)
    assert len(text.encode()) <= 50
    assert security.bounded("short", 50) == ("short", False)
