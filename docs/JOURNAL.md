# Journal

Append-only product log.

## 2026-10-03 — Skeleton, configuration and security foundation

Done:

- Package skeleton: `pyproject.toml` (`fastmcp==4.0.10`, Python >= 3.12), `uv.lock`,
  console scripts `libremodular-mcp` and `libremodular-launch`.
- `config.py`: strict, immutable configuration; absent file means strict defaults and nothing
  enabled; unknown keys, wrong types and out-of-range values are errors.
- `security.py`: process hardening, token loading/creation, private directories, path confinement,
  exact target matching, size and output bounds, AST-based script guard (categories G-1 to G-5).
- `output.py`: single exit point that removes the token, bounds strings and keeps JSON-safe types.
- `audit.py`: append-only JSON Lines journal, mode 600, rotation at 10 MiB over five generations,
  fails closed.
- `libremodular-mcp init`: creates directories (700), example configuration (600) and token (600).

Proof: `uv run pytest -q` (87 passed, 1 skipped: read-only directory test, skipped when run as
root), `uv run ruff check .`, `python3 -m compileall src`.

Remaining: MCP server and the two tools, bridge protocol client, registry, bridges and
application modules, public documentation.
