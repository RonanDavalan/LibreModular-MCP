"""Configuration and path resolution tests.

Why this module exists
----------------------
A bad file must stop the Hub rather than weaken a protection.
"""

from __future__ import annotations

import pytest

from libremodular_mcp import config
from libremodular_mcp.config import ConfigError


def load(tmp_path, text: str | None):
    path = tmp_path / "config.toml"
    if text is not None:
        path.write_text(text)
    return config.load_config(path)


def test_absent_file_gives_strict_defaults_and_nothing_enabled(tmp_path):
    cfg = load(tmp_path, None)
    assert cfg.active_names() == ()
    assert cfg.limits == config.Limits()
    assert cfg.security.allow_dangerous_scripts is False
    assert cfg.security.audit_script_text is False
    assert cfg.app("gimp").port == 9301 and cfg.app("krita").port == 9302
    assert cfg.app("blender").port == 9303
    assert cfg.app("libreoffice-calc").uno_port == 2002
    assert cfg.app("libreoffice-calc").bridge_port == 9304
    assert cfg.app("inkscape").mode == "cli"


def test_values_are_read_and_the_object_is_immutable(tmp_path):
    cfg = load(tmp_path, '[hub]\nmax_timeout_s = 120\n[apps.gimp]\nenabled = true\nport = 9400\n')
    assert cfg.limits.max_timeout_s == 120 and cfg.active_names() == ("gimp",)
    assert cfg.app("gimp").port == 9400
    with pytest.raises(AttributeError):
        cfg.limits.max_script_bytes = 1  # type: ignore[misc]


@pytest.mark.parametrize("text", [
    "[hub]\nbogus = 1\n",
    "[bogus]\nx = 1\n",
    "[apps.nosuchapp]\nenabled = true\n",
    "[apps.gimp]\nhost = '0.0.0.0'\n",
    "[apps.gimp]\nenabled = 'yes'\n",
    "[apps.gimp]\nmode = 'ssh'\n",
    "[apps.gimp]\nport = 80\n",
    "[apps.gimp]\nport = 70000\n",
    "[apps.gimp]\nport = true\n",
    "[hub]\nmax_timeout_s = 5\ndefault_timeout_s = 30\n",
    "[hub]\nmax_script_bytes = 'big'\n",
    "[hub]\nmax_output_bytes = 10\n",
    "[security]\nallow_dangerous_scripts = 1\n",
    "[security]\nextra_write_roots = ['relative/dir']\n",
    "[security]\nextra_write_roots = 'x'\n",
    "this is not toml",
])
def test_invalid_configuration_fails_closed(tmp_path, text):
    with pytest.raises(ConfigError):
        load(tmp_path, text)


def test_no_host_key_exists_anywhere():
    assert all("host" not in key for key in config._APP_KEYS + config._HUB_KEYS)


def test_xdg_paths_and_fallbacks(tmp_path):
    paths = config.resolve_paths({"HOME": "/home/alice"})
    assert str(paths.config_dir) == "/home/alice/.config/libremodular-mcp"
    assert str(paths.state_dir) == "/home/alice/.local/state/libremodular-mcp"
    assert paths.runtime_dir is None
    with pytest.raises(ConfigError):
        paths.require_runtime_dir()
    env = {"HOME": "/home/alice", "XDG_CONFIG_HOME": "/x/c", "XDG_STATE_HOME": "/x/s",
           "XDG_RUNTIME_DIR": "/run/user/1000"}
    paths = config.resolve_paths(env)
    assert str(paths.token_file) == "/x/c/libremodular-mcp/token"
    assert str(paths.audit_file) == "/x/s/libremodular-mcp/audit.jsonl"
    assert str(paths.require_runtime_dir()) == "/run/user/1000/libremodular-mcp"


def test_relative_xdg_values_are_ignored_and_relative_overrides_refused():
    paths = config.resolve_paths({"HOME": "/home/alice", "XDG_RUNTIME_DIR": "run"})
    assert paths.runtime_dir is None
    with pytest.raises(ConfigError):
        config.resolve_paths({"HOME": "/home/alice", "LIBREMODULAR_CONFIG_DIR": "rel"})


def test_overrides_replace_the_base_directories():
    env = {"HOME": "/home/alice", "LIBREMODULAR_CONFIG_DIR": "/t/c",
           "LIBREMODULAR_STATE_DIR": "/t/s", "LIBREMODULAR_RUNTIME_DIR": "/t/r",
           "XDG_CONFIG_HOME": "/ignored"}
    paths = config.resolve_paths(env)
    assert str(paths.config_dir) == "/t/c/libremodular-mcp"
    assert str(paths.runtime_dir) == "/t/r/libremodular-mcp"
