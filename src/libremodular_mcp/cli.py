"""Command line entry points: ``libremodular-mcp`` and ``libremodular-launch``.

Why this module exists
----------------------
The operator needs one explicit, safe way to create the configuration and the
bridge token with the right permissions, and a launcher whose argument list is
fixed by code and never built from text the model wrote.
"""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Sequence
from typing import TextIO

from libremodular_mcp import config
from libremodular_mcp.config import ConfigError, Paths
from libremodular_mcp.security import (
    SecurityError,
    create_token,
    ensure_private_dir,
    harden_process,
    load_token,
    open_control_file,
)


def example_config_text() -> str:
    """Commented example configuration; every application starts disabled."""
    limits = config.Limits()
    lines = [
        "# LibreModular-MCP configuration. Absent keys keep the strict defaults.",
        "",
        "[hub]",
        f"# max_script_bytes = {limits.max_script_bytes}",
        f"# max_output_bytes = {limits.max_output_bytes}",
        f"# max_line_bytes = {limits.max_line_bytes}",
        f"# default_timeout_s = {limits.default_timeout_s}",
        f"# max_timeout_s = {limits.max_timeout_s}",
        "",
        "[security]",
        "# Lifts the script guard. The audit journal then records: guard: bypassed.",
        "# allow_dangerous_scripts = false",
        "# Write the script text into the audit journal.",
        "# audit_script_text = false",
        "# Absolute directories scripts may write to, besides their own work directory.",
        "# extra_write_roots = []",
        "",
    ]
    for name, (mode, port, uno_port, bridge_port) in config.APP_DEFAULTS.items():
        lines += [f"[apps.{name}]", "enabled = false", f'mode = "{mode}"']
        if port is not None:
            lines.append(f"port = {port}")
        if uno_port is not None:
            lines.append(f"uno_port = {uno_port}")
        if bridge_port is not None:
            lines.append(f"bridge_port = {bridge_port}")
        lines.append("")
    return "\n".join(lines)


def run_init(paths: Paths, *, rotate: bool, out: TextIO) -> int:
    """Create directories, the example configuration and the token. Never prints the token."""
    harden_process()
    try:
        config_dir = ensure_private_dir(paths.config_dir)
        state_dir = ensure_private_dir(paths.state_dir)
        out.write(f"directory ready: {config_dir}\n")
        out.write(f"directory ready: {state_dir}\n")

        try:
            fd = open_control_file(paths.config_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL)
        except FileExistsError:
            out.write(f"configuration exists, unchanged: {paths.config_file}\n")
        else:
            try:
                os.write(fd, example_config_text().encode("utf-8"))
            finally:
                os.close(fd)
            out.write(f"configuration created: {paths.config_file}\n")

        if paths.token_file.exists() or paths.token_file.is_symlink():
            if rotate:
                create_token(paths.token_file, rotate=True)
                out.write(f"token rotated: {paths.token_file}\n")
                out.write("restart every bridge: they still hold the previous token\n")
            else:
                load_token(paths.token_file)
                out.write(f"token exists, unchanged: {paths.token_file}\n")
        else:
            create_token(paths.token_file)
            out.write(f"token created: {paths.token_file}\n")
    except (SecurityError, ConfigError, OSError) as exc:
        sys.stderr.write(f"error: {exc}\n")
        return 1
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="libremodular-mcp",
        description="LibreModular-MCP: let a language model drive desktop applications.",
    )
    sub = parser.add_subparsers(dest="command")
    init = sub.add_parser("init", help="create the configuration directory, example file and token")
    init.add_argument("--rotate", action="store_true", help="replace the existing token")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    if args.command == "init":
        try:
            paths = config.resolve_paths()
        except ConfigError as exc:
            sys.stderr.write(f"error: {exc}\n")
            return 1
        return run_init(paths, rotate=args.rotate, out=sys.stdout)
    parser.print_help()
    return 0


def launch_main(argv: Sequence[str] | None = None) -> int:
    sys.stderr.write("libremodular-launch: no application is supported yet\n")
    return 2
