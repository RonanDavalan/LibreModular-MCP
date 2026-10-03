"""Script guard tests.

Why this module exists
----------------------
The guard must refuse the dangerous constructs and stay silent on normal scripts.
"""

from __future__ import annotations

import textwrap

import pytest
from conftest import WORK_DIR, assert_check_fails
from guard_samples import REALISTIC_SCRIPTS

from libremodular_mcp import security
from libremodular_mcp.security import GuardReport


def run(guard, script: str, extra=()):
    return guard(textwrap.dedent(script).lstrip("\n"), WORK_DIR, extra)


def found(report: GuardReport) -> set[tuple[str, int]]:
    return {(f.category, f.line) for f in report.findings}


# (script, expected (category, line) pairs that must all be reported)
REFUSED = {
    "G-1": [
        ("import os\nos.remove('/home/alice/a')\n", {("G-1", 2)}),
        ("import os\nos.unlink(p)\nos.rmdir(p)\nos.removedirs(p)\n",
         {("G-1", 2), ("G-1", 3), ("G-1", 4)}),
        ("import shutil\nshutil.rmtree(d)\n", {("G-1", 2)}),
        ("from pathlib import Path\np = Path('/home/alice/a')\np.unlink()\n", {("G-1", 3)}),
        ("import pathlib\npathlib.Path('/home/alice/d').rmdir()\n", {("G-1", 2)}),
        ("import os\nf = os.remove\nf('x')\n", {("G-1", 2), ("G-1", 3)}),
        ("import os\nlist(map(os.remove, files))\n", {("G-1", 2)}),
    ],
    "G-2": [
        ("import subprocess\nsubprocess.run(['ls'])\n", {("G-2", 1), ("G-2", 2)}),
        ("import os\nos.system('ls')\n", {("G-2", 2)}),
        ("import os\nos.popen('ls')\n", {("G-2", 2)}),
        ("import os\nos.execv('/bin/ls', ['ls'])\n", {("G-2", 2)}),
        ("import os\nos.spawnl(os.P_WAIT, '/bin/ls', 'ls')\n", {("G-2", 2)}),
        ("import pty\npty.spawn('sh')\n", {("G-2", 1)}),
        ("from subprocess import Popen\nPopen(['ls'])\n", {("G-2", 1), ("G-2", 2)}),
    ],
    "G-3": [
        ("import socket\ns = socket.socket()\n", {("G-3", 1), ("G-3", 2)}),
        ("import urllib.request\nurllib.request.urlopen('http://x')\n", {("G-3", 1), ("G-3", 2)}),
        ("from http.client import HTTPSConnection\n", {("G-3", 1)}),
        ("from http import client\n", {("G-3", 1)}),
        ("import requests\n", {("G-3", 1)}),
        ("import smtplib\n", {("G-3", 1)}),
        ("import ftplib\n", {("G-3", 1)}),
    ],
    "G-4": [
        ("with open('/etc/passwd', 'w') as f:\n    f.write('x')\n", {("G-4", 1)}),
        ("open('/home/alice/o.txt', 'a').write('x')\n", {("G-4", 1)}),
        ("open('/home/alice/o.bin', mode='wb')\n", {("G-4", 1)}),
        ("from pathlib import Path\nPath('/home/alice/x').write_text('x')\n", {("G-4", 2)}),
        ("from pathlib import Path\nPath('/home/alice/x').write_bytes(b'x')\n", {("G-4", 2)}),
        ("import shutil\nshutil.copy('a', '/home/alice/b')\n", {("G-4", 2)}),
        ("import shutil\nshutil.copyfile('a', '/home/alice/b')\n", {("G-4", 2)}),
        ("import shutil\nshutil.move('/home/alice/a', LMM_WORK_DIR)\n", {("G-4", 2)}),
        ("OUT = '/home/alice/o.txt'\nopen(OUT, 'w')\n", {("G-4", 2)}),
        ("import os\nopen(os.path.join('/home/alice', 'x'), 'w')\n", {("G-4", 2)}),
        ("import os\nopen(os.path.join(LMM_WORK_DIR, '..', '..', 'x'), 'w')\n", {("G-4", 2)}),
        ("open('~/x.txt', 'w')\n", {("G-4", 1)}),
    ],
    "G-5": [
        ("eval('1+1')\n", {("G-5", 1)}),
        ("exec('x = 1')\n", {("G-5", 1)}),
        ("compile('1', 'f', 'eval')\n", {("G-5", 1)}),
        ("__import__('os').getcwd()\n", {("G-5", 1)}),
        ("import importlib\nimportlib.import_module('os')\n", {("G-5", 1), ("G-5", 2)}),
        ("import ctypes\n", {("G-5", 1)}),
        ("import builtins\nbuiltins.eval('1')\n", {("G-5", 2)}),
        ("getattr(__builtins__, 'eval')('1')\n", {("G-5", 1)}),
    ],
    "alias": [
        ("import os as o\no.remove('x')\n", {("G-1", 2)}),
        ("from shutil import rmtree as r\nr('x')\n", {("G-1", 1), ("G-1", 2)}),
        ("from os import system as run\nrun('ls')\n", {("G-2", 1), ("G-2", 2)}),
        ("import subprocess as sp\nsp.call(['ls'])\n", {("G-2", 2)}),
        ("from os import *\nremove('x')\n", {("G-1", 2)}),
        ("import os\ngetattr(os, 'remove')('x')\n", {("G-1", 2)}),
        ("import shutil as s\nfn = s.rmtree\nfn('x')\n", {("G-1", 2), ("G-1", 3)}),
    ],
}


def check_refuses_with_right_line(guard) -> None:
    for category, cases in REFUSED.items():
        for script, expected in cases:
            report = guard(script, WORK_DIR, ())
            missing = expected - found(report)
            assert not missing, f"{category}: {script!r} missed {sorted(missing)}"


def nothing_guard(script, work_dir, extra=()):
    return GuardReport()


def text_search_guard(script, work_dir, extra=()):
    report = GuardReport()
    for number, line in enumerate(script.splitlines(), 1):
        if "os.remove" in line:
            report.findings.append(security.Finding("G-1", "os.remove", number))
    return report


def test_every_category_is_refused_with_category_and_line():
    check_refuses_with_right_line(security.guard_script)


@pytest.mark.parametrize("defective", [nothing_guard, text_search_guard])
def test_guard_refusal_check_fails_on_defective_guards(defective):
    assert_check_fails(lambda g: check_refuses_with_right_line(g), defective)


def check_ignores_strings_and_comments(guard) -> None:
    script = textwrap.dedent(
        '''
        # os.remove(path) would delete the file
        message = "call os.remove(x) or subprocess.run(y)"
        """eval(socket) in a docstring"""
        print("shutil.rmtree")
        '''
    )
    report = guard(script, WORK_DIR, ())
    assert not report.findings, report.findings


def substring_guard(script, work_dir, extra=()):
    report = GuardReport()
    for number, line in enumerate(script.splitlines(), 1):
        for needle in ("os.remove", "subprocess.run", "shutil.rmtree", "eval("):
            if needle in line:
                report.findings.append(security.Finding("G-1", needle, number))
    return report


def test_strings_and_comments_are_not_findings():
    check_ignores_strings_and_comments(security.guard_script)
    assert_check_fails(check_ignores_strings_and_comments, substring_guard)


def test_local_names_that_look_like_modules_or_builtins_are_not_findings():
    script = textwrap.dedent(
        """
        import re
        pattern = re.compile("a+")
        for socket in node.inputs:
            socket.default_value = 0
        collection.objects.unlink(obj)
        bpy.data.objects.remove(obj)
        def compile(x):
            return x
        """
    )
    assert not security.guard_script(script, WORK_DIR).findings


def test_invalid_script_gives_an_empty_report_with_a_warning():
    report = security.guard_script("def (:\n", WORK_DIR)
    assert report.findings == []
    assert report.warnings == ["syntax error not analysed"]
    assert security.guard_script("x = 1\0\n", WORK_DIR).findings == []


def test_pathological_nesting_is_refused_not_skipped():
    report = security.guard_script("x = " + "(" * 3000 + "1" + ")" * 3000 + "\n", WORK_DIR)
    assert report.findings or report.warnings == ["syntax error not analysed"]


def test_computed_write_path_is_a_warning_not_a_finding():
    report = run(security.guard_script, "open(some_function(), 'w')\n")
    assert not report.findings
    assert any("not statically verifiable" in w for w in report.warnings)


def test_relative_write_path_is_a_warning():
    report = run(security.guard_script, "open('out.txt', 'w')\n")
    assert not report.findings and report.warnings


def test_reading_outside_the_work_directory_is_allowed():
    assert not run(security.guard_script, "open('/etc/hostname').read()\n").findings


def test_extra_write_roots_are_honoured():
    script = "open('/home/alice/exports/a.txt', 'w')\n"
    assert run(security.guard_script, script).findings
    assert not run(security.guard_script, script, ["/home/alice/exports"]).findings


def test_work_dir_writes_are_allowed():
    script = "import os\nopen(os.path.join(LMM_WORK_DIR, 'a.txt'), 'w')\n"
    assert not run(security.guard_script, script).findings


def test_refusal_message_names_category_construct_line_and_remedy():
    report = run(security.guard_script, "x = 1\nimport os\nos.remove('/home/alice/a')\n")
    message = security.format_refusal(report)
    assert "G-1" in message and "line 3" in message and "os.remove" in message
    assert "allow_dangerous_scripts" in message


def test_realistic_scripts_are_never_refused_and_false_positive_rate_is_zero():
    refused = {
        name: security.format_refusal(report)
        for name, script in REALISTIC_SCRIPTS.items()
        if (report := security.guard_script(script, WORK_DIR)).findings
    }
    rate = len(refused) / len(REALISTIC_SCRIPTS)
    assert len(REALISTIC_SCRIPTS) >= 10
    assert rate == 0.0, f"false positive rate {rate:.0%}: {refused}"


def import_everything_guard(script, work_dir, extra=()):
    report = GuardReport()
    if "import" in script:
        report.findings.append(security.Finding("G-2", "import", 1))
    return report


def check_no_false_positives(guard) -> None:
    for name, script in REALISTIC_SCRIPTS.items():
        assert not guard(script, WORK_DIR, ()).findings, name


def test_false_positive_check_fails_on_an_overzealous_guard():
    check_no_false_positives(security.guard_script)
    assert_check_fails(check_no_false_positives, import_everything_guard)


def test_long_flat_expressions_are_still_analysed():
    script = "x = " + "+".join(["1"] * 3000) + "\nimport os\nos.remove('x')\n"
    assert ("G-1", 3) in found(security.guard_script(script, WORK_DIR))
