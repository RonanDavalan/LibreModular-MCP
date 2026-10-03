"""Append-only JSON Lines audit journal.

Why this module exists
----------------------
After the fact, the operator must be able to tell what the model asked the Hub
to run, without the journal itself becoming a second copy of the documents, the
scripts or the token. When the journal cannot be written, running a script
without a trace is worse than refusing, so failure is reported to the caller.
"""

from __future__ import annotations

import hashlib
import os
from collections.abc import Callable, Mapping
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from libremodular_mcp.config import Limits
from libremodular_mcp.output import finalize, to_text
from libremodular_mcp.security import (
    SecurityError,
    write_all,
    ensure_private_dir,
    open_control_file,
)

ENTRY_KEYS = (
    "ts",
    "tool",
    "target_app",
    "status",
    "duration_ms",
    "script_bytes",
    "script_sha256",
    "guard",
    "output_bytes",
)
DEFAULT_MAX_BYTES = 10 * 1024 * 1024
DEFAULT_GENERATIONS = 5


class AuditUnavailable(Exception):
    """The journal cannot be written; the caller must refuse to execute."""


def script_fingerprint(script: str) -> tuple[int, str]:
    """Size in bytes and SHA-256 of a script, for the journal."""
    raw = script.encode("utf-8", errors="surrogatepass")
    return len(raw), hashlib.sha256(raw).hexdigest()


class AuditLog:
    def __init__(
        self,
        path: str | os.PathLike[str],
        *,
        token: str,
        limits: Limits,
        max_bytes: int = DEFAULT_MAX_BYTES,
        generations: int = DEFAULT_GENERATIONS,
        audit_script_text: bool = False,
        clock: Callable[[], datetime] | None = None,
    ) -> None:
        self._path = Path(path)
        self._token = token
        self._limits = limits
        self._max_bytes = max_bytes
        self._generations = generations
        self._script_text = audit_script_text
        self._clock = clock or (lambda: datetime.now(UTC))

    def record(self, entry: Mapping[str, Any], *, script_text: str | None = None) -> None:
        """Append one line. ``script_text`` is written only when enabled by configuration."""
        unknown = set(entry) - set(ENTRY_KEYS)
        if unknown:
            raise ValueError(f"unknown audit keys: {sorted(unknown)}")
        line = {key: entry.get(key) for key in ENTRY_KEYS}
        if line["ts"] is None:
            line["ts"] = self._clock().isoformat(timespec="milliseconds")
        if self._script_text and script_text is not None:
            line["script_text"] = script_text
        data = (to_text(finalize(line, token=self._token, limits=self._limits)) + "\n").encode()
        try:
            ensure_private_dir(self._path.parent)
            fd = open_control_file(self._path, os.O_WRONLY | os.O_APPEND | os.O_CREAT)
            try:
                write_all(fd, data)
                size = os.fstat(fd).st_size
            finally:
                os.close(fd)
            if size > self._max_bytes:
                self._rotate()
        except (OSError, SecurityError) as exc:
            raise AuditUnavailable(f"audit journal unavailable: {exc}") from exc

    def _rotate(self) -> None:
        oldest = self._path.with_name(f"{self._path.name}.{self._generations}")
        if oldest.exists():
            os.unlink(oldest)
        for index in range(self._generations - 1, 0, -1):
            source = self._path.with_name(f"{self._path.name}.{index}")
            if source.exists():
                os.replace(source, self._path.with_name(f"{self._path.name}.{index + 1}"))
        os.replace(self._path, self._path.with_name(f"{self._path.name}.1"))
