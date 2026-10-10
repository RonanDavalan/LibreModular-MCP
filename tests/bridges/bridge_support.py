"""Assistants partagés par les tests des ponts : le cœur testé, ses copies défectueuses, un faux
adaptateur, un client écrit à la main et des fichiers de jeton privés.

Pourquoi ce module existe
Chaque contrôle s'exécute sur le vrai texte de bridges/_core.py et sur une copie de ce texte cassée
exprès, par un client qui ne partage aucun code avec le Hub ni avec le pont. Garder ces pièces
ensemble garde les contrôles courts et comparables.
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
import time
import types

sys.dont_write_bytecode = True

ROOT = pathlib.Path(__file__).resolve().parents[2]
CORE_PATH = ROOT / "bridges" / "_core.py"
ADAPTERS = ROOT / "bridges" / "adapters"
GENERATED = ROOT / "bridges" / "generated"
BUILD_SCRIPT = ROOT / "scripts" / "build_bridges.py"

LOOPBACK = "127.0.0.1"
# Manifestement faux, avec la forme d'un vrai jeton : 64 caractères hexadécimaux.
FAKE_TOKEN = "0" * 64
WRONG_TOKEN = "f" * 64
EXPECTED_MAX_LINE_BYTES = 8 * 1024 * 1024

# Le plus petit PNG valide : un pixel transparent.
TINY_PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010806000000"
    "1f15c4890000000a49444154789c63000100000500010d0a2db40000000049454e44ae426082"
)


def load_text(text, name, filename):
    """Exécute `text` comme un module neuf nommé `name`, jamais enregistré dans sys.modules."""
    module = types.ModuleType(name)
    module.__file__ = filename
    exec(compile(text, filename, "exec"), module.__dict__)
    return module


def core_text():
    return CORE_PATH.read_text(encoding="utf-8")


def load_core():
    return load_text(core_text(), "lmm_core_under_test", str(CORE_PATH))


def defective_text(text, old, new):
    """`text` avec exactement un endroit changé ; un défaut qui ne change rien ne prouve rien."""
    if text.count(old) != 1:
        raise AssertionError("the defect must match exactly one place: %r" % old)
    return text.replace(old, new)


def defective_core(old, new, *more):
    """Le cœur avec un ou plusieurs endroits changés ; `more` contient d'autres paires (ancien, nouveau)."""
    text = defective_text(core_text(), old, new)
    for other_old, other_new in more:
        text = defective_text(text, other_old, other_new)
    return load_text(text, "lmm_defective_core", "<defective core>")


@contextlib.contextmanager
def quiet_error_streams():
    """Capture ce qui est écrit sur les flux d'erreur pendant le bloc."""
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
    """L'adaptateur `name` avec un endroit changé exprès."""
    text = (ADAPTERS / (name + ".py")).read_text(encoding="utf-8")
    return load_text(
        defective_text(text, old, new), "lmm_defective_" + name, "<defective %s adapter>" % name
    )


def core_reading_the_version_once():
    """Un cœur qui lit la version une fois, au démarrage, et ne la redemande jamais, quelle qu'elle ait
    été : le défaut que la relecture supprime."""
    return defective_core(
        "        if self._app_version == UNKNOWN_VERSION:\n"
        "            self._app_version = _version_of(self._adapter, on_main=True)\n",
        "",
    )


def returns_in_time(function, seconds=3.0):
    """Appelle `function` dans un fil démon neuf ; (True, value) quand elle a rendu dans les `seconds`,
    (False, None) quand elle attend encore."""
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
    """Un répertoire temporaire en mode 700, supprimé par `cleanup`."""

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
    """Un adaptateur sans application : il enregistre comment le cœur l'appelle."""

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


class LateVersionAdapter(FakeAdapter):
    """Un adaptateur dont l'application n'est pas prête d'abord : `app_version` rapporte « unknown »
    pour les `unknown_reads` premières lectures puis `ready`. Il enregistre chaque lecture et le fil
    dans lequel chacune s'est exécutée."""

    def __init__(self, unknown_reads, ready="25.2.3.2", main_thread=None):
        super().__init__(version="unknown")
        self.unknown_reads = unknown_reads
        self.ready = ready
        self.reads = 0
        self.read_threads = []
        self._main_thread = main_thread

    def app_version(self):
        import threading

        self.reads += 1
        self.read_threads.append(threading.current_thread())
        return "unknown" if self.reads <= self.unknown_reads else self.ready

    def run_on_main(self, function):
        """Sans `main_thread`, l'appel est direct. Avec un (un `MainThread`), l'appel s'y exécute et ce fil
        attend, comme le fait la boucle principale d'une application."""
        self.main_calls += 1
        if self._main_thread is None:
            return function()
        return self._main_thread.call(function)


class MainThread:
    """Un fil à lui qui exécute les fonctions remises à `call`, tenant lieu du fil principal d'une application."""

    def __init__(self):
        import queue
        import threading

        self._queue = queue.Queue()
        self.thread = threading.Thread(target=self._loop, name="fake-main", daemon=True)
        self.thread.start()

    def _loop(self):
        while True:
            item = self._queue.get()
            if item is None:
                return
            function, outcome, done = item
            try:
                outcome["value"] = function()
            except BaseException as error:  # rendue à l'appelant
                outcome["error"] = error
            done.set()

    def call(self, function):
        import threading

        outcome, done = {}, threading.Event()
        self._queue.put((function, outcome, done))
        if not done.wait(timeout=10.0):
            raise TimeoutError("the fake main thread did not answer")
        if "error" in outcome:
            raise outcome["error"]
        return outcome["value"]

    def stop(self):
        self._queue.put(None)
        self.thread.join(timeout=5.0)


class FamilyAdapter:
    """Un adaptateur qui sert plusieurs familles avec un seul pont, comme celui de LibreOffice. Il
    enregistre chaque appel que le cœur lui fait après le démarrage."""

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
        """Tout ce que le pont a écrit sur les flux d'erreur jusqu'ici."""
        return self._stderr.getvalue()


@contextlib.contextmanager
def running(core, adapter=None, token_path=None, hello_timeout=None):
    """Démarre le pont de `core` sur un port libre, avec les flux d'erreur capturés pendant tout le
    bloc, et l'arrête toujours."""
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
    """Un client écrit d'après la description du protocole, sur une socket ordinaire."""

    def __init__(self, port, timeout=5.0):
        self.socket = socket.create_connection((LOOPBACK, port), timeout=timeout)
        self.buffer = b""
        self.received = b""

    def send(self, data):
        self.socket.sendall(data)

    def send_json(self, value):
        self.send(json.dumps(value).encode("utf-8") + b"\n")

    def read_line(self):
        """La ligne suivante, ou None une fois que le pont a fermé la connexion."""
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
        """("closed", octets) quand le pont a fermé la connexion dans les `wait` secondes, ("open", octets)
        quand elle est encore ouverte ; les octets sont ce qui est arrivé."""
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


def hello_when_free(port, timeout=5.0):
    """Se connecte et dit hello, de nouveau tant que le pont refuse la connexion.

    Un pont n'a qu'une place et la libère un instant après la fermeture de la connexion précédente ;
    jusque-là il ferme aussitôt une nouvelle connexion. Le temps que cela prend dépend de la charge
    de la machine : un test qui se connecte deux fois de suite redemande donc au lieu de dormir. Rend
    (client, réponse) ; la réponse vaut None quand le pont n'a pas répondu dans les `timeout` secondes."""
    deadline = time.monotonic() + timeout
    while True:
        client = Client(port)
        try:
            answer = client.hello()
        except OSError:
            answer = None
        if answer is not None:
            return client, answer
        client.close()
        if time.monotonic() >= deadline:
            return None, None
        time.sleep(0.05)


def execute(script, work_dir="/home/alice/work", request_id="r1"):
    return {
        "id": request_id,
        "type": "execute",
        "script": script,
        "timeout_s": 30,
        "work_dir": work_dir,
    }
