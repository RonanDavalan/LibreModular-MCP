"""Single exit point for everything the Hub returns, writes or logs.

Why this module exists
----------------------
Protecting one output path at a time leaves the others open. Every payload goes
through ``finalize``, which protects the class of data: the bridge token never
leaves, sizes are bounded, and only JSON-safe values come out.
"""

from __future__ import annotations

import json
import math
import re
from collections.abc import Mapping
from typing import Any

from libremodular_mcp.config import Limits
from libremodular_mcp.security import SecurityError, bounded

REDACTION = "[redacted]"
_MAX_DEPTH = 64
_TOO_DEEP = "[nested too deep]"


class _Scrubber:
    def __init__(self, token: str, limit: int) -> None:
        self._pattern = re.compile(re.escape(token), re.IGNORECASE)
        self._limit = limit
        self.truncated = False

    def text(self, value: str) -> str:
        # The token is removed before truncation: cutting first could leave a
        # prefix of the token at the end of the kept text.
        value = value.encode("utf-8", errors="replace").decode("utf-8")
        value = self._pattern.sub(REDACTION, value)
        value, cut = bounded(value, self._limit)
        self.truncated = self.truncated or cut
        return self._pattern.sub(REDACTION, value)

    def walk(self, value: Any, depth: int = 0) -> Any:
        if depth > _MAX_DEPTH:
            return _TOO_DEEP
        if isinstance(value, str):
            return self.text(value)
        if value is None or isinstance(value, (bool, int)):
            return value
        if isinstance(value, float):
            return value if math.isfinite(value) else self.text(str(value))
        if isinstance(value, Mapping):
            return {self.text(str(k)): self.walk(v, depth + 1) for k, v in value.items()}
        if isinstance(value, (list, tuple, set, frozenset)):
            return [self.walk(v, depth + 1) for v in value]
        if isinstance(value, (bytes, bytearray, memoryview)):
            raise TypeError("raw bytes are never returned by the Hub")
        return self.text(str(value))


def finalize(payload: Mapping[str, Any], *, token: str, limits: Limits) -> dict[str, Any]:
    """Return a JSON-safe copy of ``payload``: token removed, strings bounded.

    Each string is bounded to ``max_output_bytes``. When anything was cut, the
    ``truncated`` key is set to true.
    """
    if not isinstance(token, str) or not token:
        raise SecurityError("output cannot be finalized without the token")
    if not isinstance(payload, Mapping):
        raise TypeError("payload must be a mapping")
    scrubber = _Scrubber(token, limits.max_output_bytes)
    result: dict[str, Any] = scrubber.walk(payload)
    if scrubber.truncated:
        result["truncated"] = True
    return result


def to_text(payload: Mapping[str, Any]) -> str:
    """Serialise a finalized payload as compact JSON."""
    return json.dumps(payload, separators=(",", ":"), ensure_ascii=False, allow_nan=False)
