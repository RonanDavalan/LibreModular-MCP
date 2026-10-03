"""Entry point for ``python -m libremodular_mcp``.

Why this module exists
----------------------
Lets the package run from a checkout without installing the console scripts.
"""

from __future__ import annotations

import sys

from libremodular_mcp.cli import main

if __name__ == "__main__":
    sys.exit(main())
