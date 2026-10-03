"""Tests of the init command.

Why this module exists
----------------------
The operator's first command creates the token and must do it safely.
"""

from __future__ import annotations

import io
import os
import stat

from libremodular_mcp import cli, config
from libremodular_mcp.security import load_token


def paths_in(tmp_path):
    return config.resolve_paths({
        "HOME": str(tmp_path),
        "LIBREMODULAR_CONFIG_DIR": str(tmp_path / "c"),
        "LIBREMODULAR_STATE_DIR": str(tmp_path / "s"),
    })


def init(paths, rotate=False):
    out = io.StringIO()
    code = cli.run_init(paths, rotate=rotate, out=out)
    return code, out.getvalue()


def mode(path) -> int:
    return stat.S_IMODE(path.stat().st_mode)


def test_init_creates_everything_with_private_modes_and_never_shows_the_token(tmp_path):
    paths = paths_in(tmp_path)
    code, text = init(paths)
    assert code == 0
    assert mode(paths.config_dir) == 0o700 and mode(paths.state_dir) == 0o700
    assert mode(paths.config_file) == 0o600 and mode(paths.token_file) == 0o600
    token = load_token(paths.token_file)
    assert token not in text
    assert str(paths.token_file) in text


def test_example_configuration_is_valid_and_enables_nothing(tmp_path):
    paths = paths_in(tmp_path)
    init(paths)
    cfg = config.load_config(paths.config_file)
    assert cfg.active_names() == ()
    assert {app.name for app in cfg.apps} == set(config.APP_DEFAULTS)
    assert "host" not in paths.config_file.read_text().lower()


def test_second_run_changes_nothing_and_says_so(tmp_path):
    paths = paths_in(tmp_path)
    init(paths)
    before = (paths.token_file.read_text(), paths.config_file.read_text())
    paths.config_file.write_text("[hub]\nmax_timeout_s = 60\n")
    os.chmod(paths.config_file, 0o600)
    code, text = init(paths)
    assert code == 0 and "unchanged" in text
    assert paths.token_file.read_text() == before[0]
    assert paths.config_file.read_text() == "[hub]\nmax_timeout_s = 60\n"


def test_rotate_changes_the_token_and_warns_about_bridges(tmp_path):
    paths = paths_in(tmp_path)
    init(paths)
    old = paths.token_file.read_text()
    code, text = init(paths, rotate=True)
    new = paths.token_file.read_text()
    assert code == 0 and new != old and mode(paths.token_file) == 0o600
    assert "restart every bridge" in text
    assert old not in text and new not in text


def test_init_refuses_an_existing_token_with_wide_permissions(tmp_path):
    paths = paths_in(tmp_path)
    init(paths)
    os.chmod(paths.token_file, 0o644)
    code, _ = init(paths)
    assert code == 1
    code, _ = init(paths, rotate=True)
    assert code == 0 and mode(paths.token_file) == 0o600


def test_main_without_command_prints_help(capsys):
    assert cli.main([]) == 0
    assert "init" in capsys.readouterr().out


def test_launch_reports_that_no_application_is_supported(capsys):
    assert cli.launch_main(["gimp"]) == 2
    assert "no application" in capsys.readouterr().err
