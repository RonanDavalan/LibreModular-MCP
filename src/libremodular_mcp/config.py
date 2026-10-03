"""Configuration loading and path resolution for the Hub.

Why this module exists
----------------------
Every limit and switch that changes how strict the Hub is has to come from one
place, validated once, so that a typo in a file can never silently weaken a
protection. Nothing else in the package reads configuration or the environment.
"""

from __future__ import annotations

import os
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

PACKAGE_DIR_NAME = "libremodular-mcp"

MODES = ("tcp", "uno", "cli")
PORT_MIN = 1024
PORT_MAX = 65535
MIN_BYTE_LIMIT = 1024

# name -> (mode, port, uno_port, bridge_port)
APP_DEFAULTS: dict[str, tuple[str, int | None, int | None, int | None]] = {
    "gimp": ("tcp", 9301, None, None),
    "krita": ("tcp", 9302, None, None),
    "blender": ("tcp", 9303, None, None),
    "inkscape": ("cli", None, None, None),
    "libreoffice-writer": ("uno", None, 2002, 9304),
    "libreoffice-calc": ("uno", None, 2002, 9304),
    "libreoffice-impress": ("uno", None, 2002, 9304),
}

_HUB_KEYS = (
    "max_script_bytes",
    "max_output_bytes",
    "max_line_bytes",
    "default_timeout_s",
    "max_timeout_s",
)
_SECURITY_KEYS = ("allow_dangerous_scripts", "audit_script_text", "extra_write_roots")
_APP_KEYS = ("enabled", "mode", "port", "uno_port", "bridge_port")


class ConfigError(Exception):
    """The configuration, or the environment it depends on, is invalid."""


@dataclass(frozen=True)
class Limits:
    max_script_bytes: int = 262144
    max_output_bytes: int = 1048576
    max_line_bytes: int = 8388608
    default_timeout_s: int = 30
    max_timeout_s: int = 300


@dataclass(frozen=True)
class SecuritySettings:
    allow_dangerous_scripts: bool = False
    audit_script_text: bool = False
    extra_write_roots: tuple[str, ...] = ()


@dataclass(frozen=True)
class AppSettings:
    name: str
    enabled: bool
    mode: str
    port: int | None
    uno_port: int | None
    bridge_port: int | None


@dataclass(frozen=True)
class Config:
    limits: Limits
    security: SecuritySettings
    apps: tuple[AppSettings, ...]

    def app(self, name: str) -> AppSettings | None:
        for app in self.apps:
            if app.name == name:
                return app
        return None

    def active_names(self) -> tuple[str, ...]:
        return tuple(app.name for app in self.apps if app.enabled)


@dataclass(frozen=True)
class Paths:
    config_dir: Path
    state_dir: Path
    runtime_dir: Path | None

    @property
    def config_file(self) -> Path:
        return self.config_dir / "config.toml"

    @property
    def token_file(self) -> Path:
        return self.config_dir / "token"

    @property
    def audit_file(self) -> Path:
        return self.state_dir / "audit.jsonl"

    def require_runtime_dir(self) -> Path:
        """Return the volatile directory, or refuse when the system gave none."""
        if self.runtime_dir is None:
            raise ConfigError(
                "XDG_RUNTIME_DIR is not set; refusing to write volatile files elsewhere"
            )
        return self.runtime_dir


def default_config() -> Config:
    apps = tuple(
        AppSettings(name, False, mode, port, uno_port, bridge_port)
        for name, (mode, port, uno_port, bridge_port) in APP_DEFAULTS.items()
    )
    return Config(Limits(), SecuritySettings(), apps)


def _absolute_dir(value: str, origin: str) -> Path:
    path = Path(value)
    if not path.is_absolute():
        raise ConfigError(f"{origin} must be an absolute path")
    return path


def resolve_paths(environ: Mapping[str, str] | None = None) -> Paths:
    """Resolve the three base directories from the environment (XDG rules)."""
    env = os.environ if environ is None else environ

    def pick(override: str, xdg: str, fallback: Path | None) -> Path | None:
        if env.get(override):
            return _absolute_dir(env[override], override)
        value = env.get(xdg)
        if value and Path(value).is_absolute():
            return Path(value)
        return fallback

    try:
        home = Path(env["HOME"]) if env.get("HOME") else Path.home()
    except RuntimeError as exc:
        raise ConfigError("cannot determine the home directory") from exc
    if not home.is_absolute():
        raise ConfigError("the home directory must be an absolute path")

    config_base = pick("LIBREMODULAR_CONFIG_DIR", "XDG_CONFIG_HOME", home / ".config")
    state_base = pick("LIBREMODULAR_STATE_DIR", "XDG_STATE_HOME", home / ".local" / "state")
    runtime_base = pick("LIBREMODULAR_RUNTIME_DIR", "XDG_RUNTIME_DIR", None)
    assert config_base is not None and state_base is not None
    return Paths(
        config_dir=config_base / PACKAGE_DIR_NAME,
        state_dir=state_base / PACKAGE_DIR_NAME,
        runtime_dir=None if runtime_base is None else runtime_base / PACKAGE_DIR_NAME,
    )


def _int(value: Any, where: str, minimum: int, maximum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ConfigError(f"{where} must be an integer")
    if value < minimum or (maximum is not None and value > maximum):
        bound = f"at least {minimum}" if maximum is None else f"between {minimum} and {maximum}"
        raise ConfigError(f"{where} must be {bound}")
    return value


def _bool(value: Any, where: str) -> bool:
    if not isinstance(value, bool):
        raise ConfigError(f"{where} must be a boolean")
    return value


def _table(value: Any, where: str, allowed: tuple[str, ...]) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ConfigError(f"{where} must be a table")
    for key in value:
        if key not in allowed:
            raise ConfigError(f"unknown key {key!r} in {where} (allowed: {', '.join(allowed)})")
    return value


def _parse_limits(raw: Any) -> Limits:
    table = _table(raw, "[hub]", _HUB_KEYS)
    base = Limits()
    values: dict[str, int] = {}
    for key in _HUB_KEYS:
        minimum = MIN_BYTE_LIMIT if key.endswith("_bytes") else 1
        values[key] = _int(table.get(key, getattr(base, key)), f"hub.{key}", minimum)
    if values["max_timeout_s"] < values["default_timeout_s"]:
        raise ConfigError("hub.max_timeout_s must not be smaller than hub.default_timeout_s")
    return Limits(**values)


def _parse_security(raw: Any) -> SecuritySettings:
    table = _table(raw, "[security]", _SECURITY_KEYS)
    roots = table.get("extra_write_roots", [])
    if not isinstance(roots, list) or not all(isinstance(r, str) and r for r in roots):
        raise ConfigError("security.extra_write_roots must be a list of non-empty strings")
    for root in roots:
        _absolute_dir(root, "security.extra_write_roots entries")
    return SecuritySettings(
        allow_dangerous_scripts=_bool(
            table.get("allow_dangerous_scripts", False), "security.allow_dangerous_scripts"
        ),
        audit_script_text=_bool(table.get("audit_script_text", False), "security.audit_script_text"),
        extra_write_roots=tuple(roots),
    )


def _parse_apps(raw: Any) -> tuple[AppSettings, ...]:
    if not isinstance(raw, dict):
        raise ConfigError("[apps] must be a table")
    for name in raw:
        if name not in APP_DEFAULTS:
            raise ConfigError(f"unknown application {name!r} (known: {', '.join(APP_DEFAULTS)})")
    apps = []
    for name, (d_mode, d_port, d_uno, d_bridge) in APP_DEFAULTS.items():
        table = _table(raw.get(name, {}), f"[apps.{name}]", _APP_KEYS)
        where = f"apps.{name}"
        mode = table.get("mode", d_mode)
        if mode not in MODES:
            raise ConfigError(f"{where}.mode must be one of: {', '.join(MODES)}")

        def port(key: str, default: int | None) -> int | None:
            if key not in table:
                return default
            return _int(table[key], f"{where}.{key}", PORT_MIN, PORT_MAX)

        apps.append(
            AppSettings(
                name=name,
                enabled=_bool(table.get("enabled", False), f"{where}.enabled"),
                mode=mode,
                port=port("port", d_port),
                uno_port=port("uno_port", d_uno),
                bridge_port=port("bridge_port", d_bridge),
            )
        )
    return tuple(apps)


def parse_config(data: Mapping[str, Any]) -> Config:
    """Validate parsed TOML data; every violation is a ConfigError."""
    for section in data:
        if section not in ("hub", "security", "apps"):
            raise ConfigError(f"unknown section [{section}]")
    return Config(
        limits=_parse_limits(data.get("hub", {})),
        security=_parse_security(data.get("security", {})),
        apps=_parse_apps(data.get("apps", {})),
    )


def load_config(path: Path) -> Config:
    """Load the configuration file. A missing file yields the strict defaults."""
    try:
        raw = Path(path).read_bytes()
    except FileNotFoundError:
        return default_config()
    except OSError as exc:
        raise ConfigError(f"cannot read {path}: {exc.strerror}") from exc
    try:
        data = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as exc:
        raise ConfigError(f"{path} is not valid TOML: {exc}") from exc
    return parse_config(data)
