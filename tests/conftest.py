"""Shared fixtures: temporary directories and an obviously fake token.

Why this module exists
----------------------
Tests must never touch the real configuration or a real token, and every
security check needs a way to prove it can fail.
"""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

import pytest

from libremodular_mcp.config import Limits

FAKE_TOKEN = "test-token-0000"
FILE_TOKEN = "00" * 32
WORK_DIR = "/run/user/1000/libremodular-mcp/work/gimp"


@pytest.fixture
def limits() -> Limits:
    return Limits()


@pytest.fixture
def token_file(tmp_path: Path) -> Path:
    directory = tmp_path / "cfg"
    directory.mkdir(mode=0o700)
    path = directory / "token"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.write(fd, FILE_TOKEN.encode())
    os.close(fd)
    os.chmod(path, 0o600)
    return path


def assert_check_fails(check, defective, *args) -> None:
    """Counter-proof: a check that never fails on a defective implementation proves nothing."""
    try:
        check(defective, *args)
    except AssertionError:
        return
    raise AssertionError("the check did not detect the defective implementation")


def fresh(tmp_path: Path) -> Path:
    """A new empty directory, so one check can run against several implementations."""
    return Path(tempfile.mkdtemp(dir=tmp_path))
