"""Audit journal tests.

Why this module exists
----------------------
The journal must be private, bounded and free of script text and token.
"""

from __future__ import annotations

import json
import os
import stat
from datetime import UTC, datetime

import pytest
from conftest import FAKE_TOKEN, assert_check_fails

from libremodular_mcp.audit import AuditLog, AuditUnavailable, script_fingerprint
from libremodular_mcp.config import Limits

SCRIPT = "print('secret-looking script body')"


def entry(**extra):
    size, digest = script_fingerprint(SCRIPT)
    base = {"tool": "execute_script", "target_app": "gimp", "status": "ok", "duration_ms": 5,
            "script_bytes": size, "script_sha256": digest, "guard": [], "output_bytes": 10}
    return {**base, **extra}


def make(path, **kwargs):
    kwargs.setdefault("clock", lambda: datetime(2000, 1, 1, tzinfo=UTC))
    return AuditLog(path, token=FAKE_TOKEN, limits=Limits(), **kwargs)


def lines(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def test_one_json_line_per_record_with_mode_600_in_a_private_directory(tmp_path):
    path = tmp_path / "state" / "audit.jsonl"
    log = make(path)
    log.record(entry())
    log.record(entry(status="error"))
    assert [row["status"] for row in lines(path)] == ["ok", "error"]
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert stat.S_IMODE(path.parent.stat().st_mode) == 0o700
    assert lines(path)[0]["ts"].startswith("2000-01-01")


def check_script_text_policy(make_log, tmp_path, expect_text: bool) -> None:
    path = tmp_path / ("on" if expect_text else "off") / "audit.jsonl"
    make_log(path).record(entry(), script_text=SCRIPT)
    assert (SCRIPT in path.read_text()) is expect_text


def test_script_text_is_absent_by_default_and_present_when_enabled(tmp_path):
    check_script_text_policy(make, tmp_path, False)
    check_script_text_policy(lambda p: make(p, audit_script_text=True), tmp_path, True)

    def counter_proof(impl):
        check_script_text_policy(impl, tmp_path / "cp", False)

    (tmp_path / "cp").mkdir()
    assert_check_fails(counter_proof, lambda p: make(p, audit_script_text=True))


def test_the_token_is_removed_from_entries(tmp_path):
    path = tmp_path / "audit.jsonl"
    make(path).record(entry(target_app=f"x{FAKE_TOKEN}"))
    assert FAKE_TOKEN not in path.read_text()


def test_unknown_keys_are_rejected_so_output_cannot_leak_into_the_journal(tmp_path):
    with pytest.raises(ValueError):
        make(tmp_path / "audit.jsonl").record(entry(stdout="data"))


def test_rotation_keeps_five_generations(tmp_path):
    path = tmp_path / "audit.jsonl"
    log = make(path, max_bytes=300, generations=5)
    for _ in range(60):
        log.record(entry())
    names = {p.name for p in tmp_path.iterdir()}
    assert {f"audit.jsonl.{i}" for i in range(1, 6)} <= names
    assert names <= {"audit.jsonl"} | {f"audit.jsonl.{i}" for i in range(1, 6)}
    for p in tmp_path.iterdir():
        assert stat.S_IMODE(p.stat().st_mode) == 0o600


def test_unavailable_when_the_directory_cannot_be_used(tmp_path):
    blocker = tmp_path / "blocker"
    blocker.write_text("a file, not a directory")
    with pytest.raises(AuditUnavailable):
        make(blocker / "audit.jsonl").record(entry())


@pytest.mark.skipif(os.geteuid() == 0, reason="root ignores directory permission bits")
def test_unavailable_when_the_directory_is_read_only(tmp_path):
    directory = tmp_path / "ro"
    directory.mkdir(mode=0o700)
    os.chmod(directory, 0o500)
    with pytest.raises(AuditUnavailable):
        make(directory / "audit.jsonl").record(entry())


def test_unavailable_when_the_journal_is_too_open_or_a_symlink(tmp_path):
    directory = tmp_path / "state"
    directory.mkdir(mode=0o700)
    path = directory / "audit.jsonl"
    path.write_text("")
    os.chmod(path, 0o644)
    with pytest.raises(AuditUnavailable):
        make(path).record(entry())
    path.unlink()
    target = tmp_path / "elsewhere"
    target.write_text("")
    os.chmod(target, 0o600)
    path.symlink_to(target)
    with pytest.raises(AuditUnavailable):
        make(path).record(entry())
    assert target.read_text() == ""


def test_unavailable_when_the_directory_is_too_open(tmp_path):
    directory = tmp_path / "open"
    directory.mkdir()
    os.chmod(directory, 0o755)
    with pytest.raises(AuditUnavailable):
        make(directory / "audit.jsonl").record(entry())


def test_script_fingerprint_is_size_and_sha256():
    size, digest = script_fingerprint("abc")
    assert size == 3 and digest.startswith("ba7816bf")
