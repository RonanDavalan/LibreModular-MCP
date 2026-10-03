"""Tests of the single output point.

Why this module exists
----------------------
Whatever the Hub returns must never carry the token and must stay bounded.
"""

from __future__ import annotations

import json

import pytest
from conftest import FAKE_TOKEN, assert_check_fails

from libremodular_mcp import output
from libremodular_mcp.config import Limits
from libremodular_mcp.security import SecurityError

SMALL = Limits(max_output_bytes=64)


def finalize(payload, limits=None):
    return output.finalize(payload, token=FAKE_TOKEN, limits=limits or Limits())


def _flat(value) -> str:
    return json.dumps(value)


def check_removes_token_everywhere(impl) -> None:
    payload = {
        "plain": f"before {FAKE_TOKEN} after",
        FAKE_TOKEN: "key",
        "nested": {"deep": [f"x{FAKE_TOKEN}", {"deeper": FAKE_TOKEN}]},
        "error": {"traceback": f'File "<libremodular>", line 1\n  token = "{FAKE_TOKEN}"'},
        "upper": FAKE_TOKEN.upper(),
    }
    assert FAKE_TOKEN not in _flat(impl(payload)).replace(FAKE_TOKEN.upper(), "")
    assert FAKE_TOKEN.upper() not in _flat(impl(payload))


def check_removes_token_cut_by_truncation(impl) -> None:
    text = "x" * 48 + FAKE_TOKEN + "y" * 100
    out = impl({"stdout": text})
    assert out["truncated"] is True
    assert FAKE_TOKEN[:5] not in out["stdout"]


def top_level_only(payload, limits=None):
    return {k: v.replace(FAKE_TOKEN, "") if isinstance(v, str) else v for k, v in payload.items()}


def cut_then_remove(payload, limits=None):
    out = {}
    for key, value in payload.items():
        cut = value[:53] + "[truncated]"
        out[key] = cut.replace(FAKE_TOKEN, "")
    out["truncated"] = True
    return out


def test_finalize_removes_the_token_from_values_keys_nested_data_and_traces():
    check_removes_token_everywhere(finalize)
    assert_check_fails(check_removes_token_everywhere, top_level_only)


def test_finalize_removes_a_token_that_truncation_would_have_cut():
    check_removes_token_cut_by_truncation(lambda p: finalize(p, SMALL))
    assert_check_fails(check_removes_token_cut_by_truncation, cut_then_remove)


def test_finalize_bounds_strings_and_marks_truncation():
    out = finalize({"stdout": "é" * 1000, "stderr": "ok"}, SMALL)
    assert out["truncated"] is True
    assert out["stdout"].endswith("[truncated]") and len(out["stdout"].encode()) <= 64
    assert out["stderr"] == "ok"
    assert "truncated" not in finalize({"stderr": "ok"}, SMALL)


def test_finalize_preserves_scalar_types():
    out = finalize({"ok": True, "n": 3, "f": 1.5, "none": None, "items": (1, "a")})
    assert out == {"ok": True, "n": 3, "f": 1.5, "none": None, "items": [1, "a"]}
    assert out["ok"] is True and type(out["n"]) is int


def test_finalize_always_returns_json_serialisable_data():
    out = finalize({"nan": float("nan"), "obj": object(), 5: "intkey", "s": {1, 2}})
    output.to_text(out)


def test_finalize_refuses_bytes_and_a_missing_token():
    with pytest.raises(TypeError):
        finalize({"image": b"\x89PNG"})
    for bad in (None, ""):
        with pytest.raises(SecurityError):
            output.finalize({"a": 1}, token=bad, limits=Limits())


def test_finalize_survives_pathological_depth():
    value: dict = {}
    cursor = value
    for _ in range(500):
        cursor["n"] = {}
        cursor = cursor["n"]
    output.to_text(finalize(value))


def test_to_text_is_compact_utf8_json():
    assert output.to_text({"a": "é", "b": [1, 2]}) == '{"a":"é","b":[1,2]}'
