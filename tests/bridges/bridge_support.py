"""Helpers shared by the bridge tests: the core under test, its defective copies,
a fake adapter, a hand-written client and private token files.

Why this module exists
Every check runs against the real text of bridges/_core.py and against a copy of
that text broken on purpose, through a client that shares no code with the Hub or
the bridge. Keeping these pieces together keeps the checks short and comparable.
"""

import contextlib
import io
import json
import os
import pathlib
import shutil
import socket
import sys
import tempfile
import types

sys.dont_write_bytecode = True

ROOT = pathlib.Path(__file__).resolve().parents[2]
CORE_PATH = ROOT / "bridges" / "_core.py"
ADAPTERS = ROOT / "bridges" / "adapters"
GENERATED = ROOT / "bridges" / "generated"
BUILD_SCRIPT = ROOT / "scripts" / "build_bridges.py"

LOOPBACK = "127.0.0.1"
# Obviously fake, with the shape of a real token: 64 hexadecimal characters.
FAKE_TOKEN = "0" * 64
WRONG_TOKEN = "f" * 64
EXPECTED_MAX_LINE_BYTES = 8 * 1024 * 1024

# The smallest valid PNG: one transparent pixel.
TINY_PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010806000000"
    "1f15c4890000000a49444154789c63000100000500010d0a2db40000000049454e44ae426082"
)


def load_text(text, name, filename):
    """Run `text` as a fresh module named `name`, never registered in sys.modules."""
    module = types.ModuleType(name)
    module.__file__ = filename
    exec(compile(text, filename, "exec"), module.__dict__)
    return module


def core_text():
    return CORE_PATH.read_text(encoding="utf-8")


def load_core():
    return load_text(core_text(), "lmm_core_under_test", str(CORE_PATH))


def defective_text(text, old, new):
    """`text` with exactly one place changed; a defect that changes nothing proves nothing."""
    if text.count(old) != 1:
        raise AssertionError("the defect must match exactly one place: %r" % old)
    return text.replace(old, new)


def defective_core(old, new, *more):
    """The core with one or more places changed; `more` holds further (old, new) pairs."""
    text = defective_text(core_text(), old, new)
    for other_old, other_new in more:
        text = defective_text(text, other_old, other_new)
    return load_text(text, "lmm_defective_core", "<defective core>")


@contextlib.contextmanager
def quiet_error_streams():
    """Capture what is written on the error streams during the block."""
    stream = io.StringIO()
    saved = sys.__stderr__, sys.stderr
    sys.__stderr__ = sys.stderr = stream
    try:
        yield stream
    finally:
        sys.__stderr__, sys.stderr = saved


def load_adapter(name):
    path = ADAPTERS / (name + ".py")
    return load_text(path.read_text(encoding="utf-8"), "lmm_adapter_" + name, str(path))


def defective_adapter(name, old, new):
    """The adapter `name` with one place changed on purpose."""
    text = (ADAPTERS / (name + ".py")).read_text(encoding="utf-8")
    return load_text(
        defective_text(text, old, new), "lmm_defective_" + name, "<defective %s adapter>" % name
    )


def returns_in_time(function, seconds=3.0):
    """Call `function` in a fresh daemon thread; (True, value) when it returned
    within `seconds`, (False, None) when it is still waiting."""
    import threading

    outcome = {}

    def call():
        outcome["value"] = function()
        outcome["thread"] = threading.current_thread()

    worker = threading.Thread(target=call, daemon=True)
    worker.start()
    worker.join(timeout=seconds)
    if worker.is_alive() or "value" not in outcome:
        return False, None
    return True, outcome


class PrivateDir:
    """A temporary directory in mode 700, removed by `cleanup`."""

    def __init__(self):
        self.path = pathlib.Path(tempfile.mkdtemp(prefix="lmm-bridge-test-"))
        os.chmod(self.path, 0o700)

    def token(self, text=FAKE_TOKEN, mode=0o600, name="token"):
        path = self.path / name
        path.write_text(text, encoding="ascii")
        os.chmod(path, mode)
        return path

    def cleanup(self):
        shutil.rmtree(self.path, ignore_errors=True)


class FakeAdapter:
    """An adapter without application: it records how the core calls it."""

    app = "gimp"

    def __init__(self, version="3.0.4", state=None, state_error=None, screenshot_error=None):
        self.version = version
        self._state = {"documents": 1} if state is None else state
        self.state_error = state_error
        self.screenshot_error = screenshot_error
        self.main_calls = 0

    def app_version(self):
        return self.version

    def namespace(self):
        return {"FAKE_API": "fake application API"}

    def run_on_main(self, function):
        self.main_calls += 1
        return function()

    def state(self):
        if self.state_error is not None:
            raise self.state_error
        return self._state

    def screenshot(self, path):
        with open(path, "wb") as handle:
            handle.write(TINY_PNG)
        if self.screenshot_error is not None:
            raise self.screenshot_error
        return True


class FamilyAdapter:
    """An adapter that serves several families with one bridge, as the LibreOffice
    one does. It records every call the core makes to it after the start."""

    app = "libreoffice"
    families = ("writer", "calc", "impress")

    def __init__(self):
        self.calls = []

    def app_version(self):
        return "25.2.3.2"

    def namespace(self, family):
        self.calls.append(("namespace", family))
        return {"FAMILY": family}

    def run_on_main(self, function):
        self.calls.append(("run_on_main",))
        return function()

    def state(self, family):
        self.calls.append(("state", family))
        return {"family": family}

    def screenshot(self, path, family):
        self.calls.append(("screenshot", family))
        with open(path, "wb") as handle:
            handle.write(TINY_PNG)
        return True


class Running:
    def __init__(self, core, port, stderr):
        self.core = core
        self.port = port
        self._stderr = stderr

    def said(self):
        """Everything the bridge wrote on the error streams so far."""
        return self._stderr.getvalue()


@contextlib.contextmanager
def running(core, adapter=None, token_path=None, hello_timeout=None):
    """Start the bridge of `core` on a free port, with the error streams
    captured for the whole block, and always stop it."""
    directory = None
    if token_path is None:
        directory = PrivateDir()
        token_path = directory.token()
    try:
        with quiet_error_streams() as stream:
            options = {} if hello_timeout is None else {"hello_timeout": hello_timeout}
            port = core.start(adapter or FakeAdapter(), 0, str(token_path), **options)
            try:
                yield Running(core, port, stream)
            finally:
                core.stop()
    finally:
        if directory is not None:
            directory.cleanup()


class Client:
    """A client written from the protocol description, on a plain socket."""

    def __init__(self, port, timeout=5.0):
        self.socket = socket.create_connection((LOOPBACK, port), timeout=timeout)
        self.buffer = b""
        self.received = b""

    def send(self, data):
        self.socket.sendall(data)

    def send_json(self, value):
        self.send(json.dumps(value).encode("utf-8") + b"\n")

    def read_line(self):
        """The next line, or None once the bridge closed the connection."""
        while b"\n" not in self.buffer:
            try:
                chunk = self.socket.recv(65536)
            except ConnectionResetError:
                return None
            if not chunk:
                return None
            self.received += chunk
            self.buffer += chunk
        line, _, self.buffer = self.buffer.partition(b"\n")
        return line

    def read_json(self):
        line = self.read_line()
        return None if line is None else json.loads(line.decode("utf-8"))

    def hello(self, token=FAKE_TOKEN):
        self.send_json({"type": "hello", "token": token, "protocol": 1})
        return self.read_json()

    def request(self, value):
        self.send_json(value)
        return self.read_json()

    def outcome(self, wait=5.0):
        """("closed", bytes) when the bridge closed the connection within `wait`
        seconds, ("open", bytes) when it is still open; bytes are what arrived."""
        self.socket.settimeout(wait)
        data = b""
        while True:
            try:
                chunk = self.socket.recv(65536)
            except socket.timeout:
                return "open", data
            except ConnectionResetError:
                return "closed", data
            if not chunk:
                return "closed", data
            data += chunk

    def close(self):
        try:
            self.socket.close()
        except OSError:
            pass


def execute(script, work_dir="/home/alice/work", request_id="r1"):
    return {
        "id": request_id,
        "type": "execute",
        "script": script,
        "timeout_s": 30,
        "work_dir": work_dir,
    }
