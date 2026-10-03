# Journal

Append-only product log.

## 2026-10-03 — Clean start in Rust

The hub is rewritten in Rust so that it ships as a single binary with no interpreter to install.
The first Python implementation of the security foundation was removed from the working tree.
It remains readable in the history as a behaviour reference: commit `7fa7dd5`
(`git show 7fa7dd5:src/libremodular_mcp/security.py`, and `git show 7fa7dd5:tests/test_guard.py`).
The bridges that run inside the applications stay in Python.
