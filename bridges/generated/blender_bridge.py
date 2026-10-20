# Généré par scripts/build_bridges.py à partir de bridges/_core.py et bridges/adapters/blender.py.
# Ne pas modifier : changer ces fichiers puis lancer python3 scripts/build_bridges.py.
# Ne contient aucun jeton : le pont le lit dans LIBREMODULAR_TOKEN_PATH ou au chemin par défaut.

"""Cœur commun des ponts de LibreModular-MCP : protocole, jeton et exécution des scripts.

Pourquoi ce module existe
Un pont exécute des scripts avec les droits de l'utilisateur dans une application. Qui peut lui
parler, combien il lit et comment un script s'exécute sont décidés une seule fois, ici, pour
qu'une application n'ajoute qu'un adaptateur et qu'aucun adaptateur ne puisse affaiblir un contrôle.
"""

import errno
import hmac
import io
import json
import os
import socket
import stat
import sys
import threading
import time
import traceback

# La seule adresse sur laquelle un pont écoute. C'est une constante exprès : aucun paramètre,
# variable d'environnement ou clé de configuration ne peut la changer.
LISTEN_ADDRESS = "127.0.0.1"
PROTOCOL_VERSION = 1
BRIDGE_VERSION = "1"
# Une ligne plus longue que cela, en octets et sans son saut de ligne, ferme la connexion.
MAX_LINE_BYTES = 8 * 1024 * 1024
# Combien de temps une nouvelle connexion peut rester silencieuse avant sa première ligne.
HELLO_TIMEOUT_S = 5.0
# Le nom de fichier sous lequel les scripts sont compilés, pour que les traces d'appels pointent vers leur texte.
SCRIPT_FILENAME = "<libremodular>"
# Ce qu'un adaptateur rapporte comme version quand l'API de l'application ne peut pas être importée.
UNKNOWN_VERSION = "unknown"
# Les fichiers créés pendant qu'un script ou une capture s'exécute sont privés à l'utilisateur.
PRIVATE_UMASK = 0o077
TOKEN_HEX_LENGTH = 64
TOKEN_READ_CAP = 4096

_READ_CHUNK = 64 * 1024
_ACCEPT_POLL_S = 0.2
_LISTEN_BACKLOG = 8
# Ce qu'un script peut afficher, par flux, avant que le reste soit jeté.
_CAPTURE_LIMIT = MAX_LINE_BYTES // 16
# Chaque texte d'une réponse qui ne tiendrait pas dans une ligne est coupé à cette longueur.
_FALLBACK_TEXT = 64 * 1024
_TRUNCATED = "\n[truncated by the bridge]"
_WARNING = "libremodular bridge warning: "
_HEX_DIGITS = b"0123456789abcdefABCDEF"


class BridgeError(Exception):
    """Le pont ne peut pas démarrer. Le message ne contient jamais le jeton."""


# --------------------------------------------------------------------------
# Le jeton
# --------------------------------------------------------------------------


def _read_token(token_path):
    """Lit le jeton dans un fichier ordinaire privé de l'utilisateur courant."""
    path = os.fspath(token_path)
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK | os.O_NOCTTY
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        if error.errno == errno.ELOOP:
            raise BridgeError("the token file %s is a symbolic link" % path) from None
        raise BridgeError(
            "cannot open the token file %s: %s" % (path, error.strerror)
        ) from None
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode):
            raise BridgeError("the token file %s is not a regular file" % path)
        if info.st_uid != os.geteuid():
            raise BridgeError("the token file %s belongs to another user" % path)
        if info.st_mode & 0o077:
            raise BridgeError(
                "the token file %s is open to the group or to others; run chmod 600 on it"
                % path
            )
        data = b""
        while len(data) <= TOKEN_READ_CAP:
            chunk = os.read(descriptor, TOKEN_READ_CAP + 1 - len(data))
            if not chunk:
                break
            data += chunk
    finally:
        os.close(descriptor)
    while data.endswith(b"\n"):
        data = data[:-1]
    if len(data) != TOKEN_HEX_LENGTH or any(byte not in _HEX_DIGITS for byte in data):
        raise BridgeError(
            "the token file %s does not hold a token made by libremodular-mcp init" % path
        )
    return data


# --------------------------------------------------------------------------
# Lignes et réponses
# --------------------------------------------------------------------------


class _LineReader:
    """Lit des lignes sur une socket sans jamais tenir beaucoup plus d'une ligne."""

    def __init__(self, connection):
        self._connection = connection
        self._buffer = bytearray()

    def read_line(self, deadline):
        """Une ligne sans son saut de ligne ; None quand la connexion se termine, que l'échéance
        monotone `deadline` passe, ou que la ligne dépasse MAX_LINE_BYTES."""
        while True:
            end = self._buffer.find(b"\n")
            if end >= 0:
                if end > MAX_LINE_BYTES:
                    return None
                line = bytes(self._buffer[:end])
                del self._buffer[: end + 1]
                return line
            if len(self._buffer) > MAX_LINE_BYTES:
                return None
            if deadline is None:
                self._connection.settimeout(None)
            else:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return None
                self._connection.settimeout(remaining)
            room = min(_READ_CHUNK, MAX_LINE_BYTES + 1 - len(self._buffer))
            try:
                chunk = self._connection.recv(room)
            except socket.timeout:
                return None
            except OSError:
                return None
            if not chunk:
                return None
            self._buffer += chunk


def _parse_object(line):
    """L'objet JSON qu'une ligne contient, ou None pour tout autre contenu."""
    try:
        value = json.loads(line.decode("utf-8"))
    except Exception:
        return None
    return value if isinstance(value, dict) else None


def _dump(value):
    # Un substitut isolé ne peut pas voyager en UTF-8 : il devient un caractère de remplacement.
    text = json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
    return text.encode("utf-8", "replace")


def _json_copy(value):
    """Une copie JSON simple de `value`, ou une exception quand elle n'est pas sérialisable."""
    return json.loads(json.dumps(value, allow_nan=False))


def _cut(text):
    if isinstance(text, str) and len(text) > _FALLBACK_TEXT:
        return text[:_FALLBACK_TEXT] + _TRUNCATED
    return text


def _encode(answer):
    """La réponse en une seule ligne qui tient dans MAX_LINE_BYTES."""
    try:
        line = _dump(answer)
    except (TypeError, ValueError):
        line = None
    if line is not None and len(line) <= MAX_LINE_BYTES:
        return line
    smaller = dict(answer)
    warning = "the answer was too large for one protocol line and was reduced\n"
    if "result" in smaller:
        smaller["result"] = None
        smaller["stderr"] = _cut(smaller.get("stderr", "")) + "\n" + _WARNING + warning
    if "state" in smaller:
        smaller["state"] = {}
        smaller["warnings"] = [_cut(item) for item in smaller.get("warnings", [])[:16]]
        smaller["warnings"].append(warning.strip())
    for key in ("stdout", "stderr"):
        if key in smaller:
            smaller[key] = _cut(smaller[key])
    error = smaller.get("error")
    if isinstance(error, dict):
        smaller["error"] = {key: _cut(value) for key, value in error.items()}
    try:
        line = _dump(smaller)
    except (TypeError, ValueError):
        line = None
    if line is not None and len(line) <= MAX_LINE_BYTES:
        return line
    return _dump(
        {
            "id": answer.get("id"),
            "ok": False,
            "error": {
                "type": "AnswerTooLarge",
                "message": "the answer could not fit in one protocol line",
                "traceback": "",
            },
        }
    )


def _send(connection, answer):
    connection.sendall(_encode(answer) + b"\n")


def _failure(kind, message, traceback_text=""):
    return {
        "ok": False,
        "error": {"type": kind, "message": message, "traceback": traceback_text},
    }


def _text_of(error):
    try:
        return str(error)
    except Exception:
        return "<the exception could not be shown>"


def _peer_allowed(peer):
    return isinstance(peer, tuple) and len(peer) >= 1 and peer[0] == LISTEN_ADDRESS


# --------------------------------------------------------------------------
# Exécuter un script
# --------------------------------------------------------------------------


class _Capture(io.TextIOBase):
    """Recueille ce qu'un script affiche, jusqu'à une borne, pour qu'une boucle d'affichages sans fin
    ne puisse pas épuiser la mémoire de l'application."""

    encoding = "utf-8"
    errors = "strict"

    def __init__(self):
        super().__init__()
        self._parts = []
        self._size = 0
        self._dropped = False

    def writable(self):
        return True

    def write(self, text):
        if not isinstance(text, str):
            raise TypeError("write() argument must be str, not %s" % type(text).__name__)
        room = _CAPTURE_LIMIT - self._size
        if room > 0:
            kept = text[:room]
            self._parts.append(kept)
            self._size += len(kept)
        if len(text) > max(room, 0):
            self._dropped = True
        return len(text)

    def text(self):
        joined = "".join(self._parts)
        return joined + _TRUNCATED if self._dropped else joined


def _with_private_umask(function, *arguments):
    """Exécute `function` avec le masque de création privé, rétabli ensuite quoi qu'il arrive."""
    previous = os.umask(PRIVATE_UMASK)
    try:
        return function(*arguments)
    finally:
        os.umask(previous)


def _script_error(error):
    """Le type, le message et la trace d'appels standard de `error`, à partir du script : les cadres
    du pont lui-même sont laissés de côté."""
    frames = error.__traceback__
    while frames is not None and frames.tb_frame.f_code.co_filename != SCRIPT_FILENAME:
        frames = frames.tb_next
    return {
        "type": type(error).__name__,
        "message": _text_of(error),
        "traceback": "".join(traceback.format_exception(type(error), error, frames)),
    }


def _script_result(namespace):
    """Le `result` que le script a laissé, et un avertissement quand il ne peut pas être envoyé."""
    if "result" not in namespace:
        return None, ""
    value = namespace["result"]
    try:
        return _json_copy(value), ""
    except Exception as error:
        return None, "%sresult of type %s is not JSON serialisable (%s); null returned\n" % (
            _WARNING,
            type(value).__name__,
            type(error).__name__,
        )


def _run_script(adapter, script, work_dir, extra=()):
    """Compile et exécute un script dans un espace de noms neuf ; ne lève jamais pour ce que fait
    le script, SystemExit compris. `extra` contient les arguments qu'un adaptateur qui sert
    plusieurs familles prend après les arguments habituels."""
    namespace = {"__name__": "__main__"}
    namespace.update(adapter.namespace(*extra))
    namespace["LMM_WORK_DIR"] = work_dir
    stdout, stderr = _Capture(), _Capture()
    outcome = {}

    def run():
        saved = sys.stdout, sys.stderr
        sys.stdout, sys.stderr = stdout, stderr
        try:
            code = compile(script, SCRIPT_FILENAME, "exec", dont_inherit=True)
            exec(code, namespace)
        except BaseException as error:  # un script ne doit jamais arrêter le pont
            outcome["error"] = _script_error(error)
        finally:
            sys.stdout, sys.stderr = saved

    _with_private_umask(run)
    error = outcome.get("error")
    result, warning = (None, "") if error else _script_result(namespace)
    if warning:
        stderr.write(warning)
    return {
        "ok": error is None,
        "stdout": stdout.text(),
        "stderr": stderr.text(),
        "result": result,
        "error": error,
    }


# --------------------------------------------------------------------------
# Le serveur
# --------------------------------------------------------------------------


def _version_of(adapter, on_main=False):
    """La version que l'adaptateur rapporte, ou UNKNOWN_VERSION. `on_main` la demande là où
    l'adaptateur exécute les appels de l'application au lieu de ce fil."""
    try:
        version = adapter.run_on_main(adapter.app_version) if on_main else adapter.app_version()
    except Exception:
        return UNKNOWN_VERSION
    return version if isinstance(version, str) and version else UNKNOWN_VERSION


def _discard(connection):
    try:
        connection.close()
    except OSError:
        pass


def _families_of(adapter):
    """Les familles qu'un adaptateur sert avec un seul pont, ou None quand il sert une seule
    application : son attribut `families`, un n-uplet de noms."""
    families = getattr(adapter, "families", None)
    if families is None:
        return None
    if (
        not isinstance(families, tuple)
        or not families
        or not all(isinstance(name, str) and name for name in families)
    ):
        raise BridgeError("the adapter's families must be a non-empty tuple of names")
    return families


class _Bridge:
    """Une socket d'écoute et au plus une connexion servie à la fois."""

    def __init__(self, adapter, port, token, hello_timeout):
        app = getattr(adapter, "app", None)
        if not isinstance(app, str) or not app:
            raise BridgeError("the adapter has no application name")
        self._adapter = adapter
        self._app = app
        self._families = _families_of(adapter)
        # Lu ici, dans le fil qui démarre le pont. Tant qu'elle est inconnue, l'application n'est peut-être
        # tout simplement pas prête : _current_version redemande.
        self._app_version = _version_of(adapter)
        self._token = token
        self._hello_timeout = hello_timeout
        self._stop = threading.Event()
        self._lock = threading.Lock()
        self._active = None
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind((LISTEN_ADDRESS, port))
            listener.listen(_LISTEN_BACKLOG)
            listener.settimeout(_ACCEPT_POLL_S)
        except BaseException:
            listener.close()
            raise
        self._listener = listener
        self.port = listener.getsockname()[1]
        self._thread = threading.Thread(
            target=self._accept_loop, name="libremodular-bridge", daemon=True
        )

    def begin(self):
        self._thread.start()

    def close(self):
        self._stop.set()
        _discard(self._listener)
        with self._lock:
            active = self._active
        if active is not None:
            try:
                active.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        if self._thread.is_alive() and self._thread is not threading.current_thread():
            self._thread.join(timeout=2.0)

    def _accept_loop(self):
        while not self._stop.is_set():
            try:
                connection, peer = self._listener.accept()
            except socket.timeout:
                continue
            except OSError:
                if self._stop.is_set():
                    break
                time.sleep(_ACCEPT_POLL_S)
                continue
            if not _peer_allowed(peer):
                _discard(connection)
                continue
            with self._lock:
                taken = self._active is not None
                if not taken:
                    self._active = connection
            if taken:
                _discard(connection)
                continue
            worker = threading.Thread(
                target=self._serve,
                args=(connection,),
                name="libremodular-bridge-connection",
                daemon=True,
            )
            worker.start()

    def _serve(self, connection):
        try:
            self._converse(connection)
        except OSError:
            pass
        except Exception as error:
            _say("LMM bridge: connection ended on %s" % type(error).__name__)
        finally:
            _discard(connection)
            with self._lock:
                if self._active is connection:
                    self._active = None

    def _current_version(self):
        """La version de l'application. Une fois connue, elle est gardée ; jusque-là chaque appel redemande
        à l'adaptateur, là où l'adaptateur exécute les appels de l'application, pour qu'un pont démarré
        avant son application rapporte la version dès que l'application est là. Une connexion est servie
        à la fois, donc deux fils ne demandent jamais en même temps."""
        if self._app_version == UNKNOWN_VERSION:
            self._app_version = _version_of(self._adapter, on_main=True)
        return self._app_version

    def _hello_is_valid(self, line):
        greeting = _parse_object(line)
        if greeting is None or greeting.get("type") != "hello":
            return False
        protocol = greeting.get("protocol")
        if type(protocol) is not int or protocol != PROTOCOL_VERSION:
            return False
        given = greeting.get("token")
        if not isinstance(given, str):
            return False
        try:
            given = given.encode("utf-8")
        except UnicodeError:
            return False
        return hmac.compare_digest(given, self._token)

    def _converse(self, connection):
        reader = _LineReader(connection)
        first = reader.read_line(time.monotonic() + self._hello_timeout)
        if first is None or not self._hello_is_valid(first):
            return
        connection.settimeout(None)
        _send(
            connection,
            {
                "type": "hello",
                "ok": True,
                "protocol": PROTOCOL_VERSION,
                "app": self._app,
                "app_version": self._current_version(),
                "bridge_version": BRIDGE_VERSION,
            },
        )
        while not self._stop.is_set():
            line = reader.read_line(None)
            if line is None:
                return
            request = _parse_object(line)
            if request is None:
                return
            answer = self._answer(request)
            answer["id"] = request.get("id")
            _send(connection, answer)

    def _answer(self, request):
        kind = request.get("type")
        if kind == "ping":
            return {"ok": True}
        if kind == "execute":
            return self._execute(request)
        if kind == "state":
            return self._state(request)
        return _failure("UnknownRequest", "unknown request type")

    def _unknown_family(self, request):
        """L'échec à répondre quand cet adaptateur sert des familles et que la requête n'en nomme aucune,
        avant qu'aucune ligne de l'adaptateur ne s'exécute ; None sinon."""
        if self._families is None:
            return None
        family = request.get("family")
        if isinstance(family, str) and family in self._families:
            return None
        return _failure(
            "UnknownFamily",
            "the request must name one of the families of this bridge: %s"
            % ", ".join(self._families),
        )

    def _family_arguments(self, request):
        """Ce qu'un adaptateur qui sert des familles reçoit après ses arguments habituels : la famille de
        la requête. Rien pour les autres, qui ne voient jamais le champ."""
        if self._families is None:
            return ()
        return (request["family"],)

    def _execute(self, request):
        script = request.get("script")
        work_dir = request.get("work_dir")
        if not isinstance(script, str) or not isinstance(work_dir, str):
            return _failure(
                "InvalidRequest", "an execute request needs a script and a work_dir as text"
            )
        unknown = self._unknown_family(request)
        if unknown is not None:
            unknown.update({"stdout": "", "stderr": "", "result": None})
            return unknown
        extra = self._family_arguments(request)
        started = time.monotonic()
        adapter = self._adapter
        try:
            answer = adapter.run_on_main(lambda: _run_script(adapter, script, work_dir, extra))
        except Exception as error:
            answer = _failure(
                type(error).__name__,
                _text_of(error),
                "".join(traceback.format_exception(type(error), error, error.__traceback__)),
            )
            answer.update({"stdout": "", "stderr": "", "result": None})
        if not isinstance(answer, dict):
            answer = _failure("BridgeError", "the adapter did not hand back the outcome")
            answer.update({"stdout": "", "stderr": "", "result": None})
        answer["duration_ms"] = int((time.monotonic() - started) * 1000)
        return answer

    def _state(self, request):
        capture_path = request.get("capture_path")
        if capture_path is not None and not isinstance(capture_path, str):
            return _failure("InvalidRequest", "capture_path must be text or null")
        unknown = self._unknown_family(request)
        if unknown is not None:
            return unknown
        extra = self._family_arguments(request)
        adapter = self._adapter
        warnings = []
        state = {}
        try:
            reported = adapter.run_on_main(lambda: adapter.state(*extra))
            if not isinstance(reported, dict):
                raise TypeError("the adapter returned a %s" % type(reported).__name__)
            state = _json_copy(reported)
        except Exception as error:
            # Le type et le message seulement : une trace d'appels décrirait le pont, pas le document.
            warnings.append("state unavailable: %s: %s" % (type(error).__name__, _text_of(error)))
        # Après l'état, qui a pu attendre que l'application soit là.
        if self._current_version() == UNKNOWN_VERSION:
            warnings.insert(
                0, "the %s scripting API cannot be imported in this process" % self._app
            )
        screenshot = None
        if capture_path:
            try:
                written = adapter.run_on_main(
                    lambda: _with_private_umask(adapter.screenshot, capture_path, *extra)
                )
            except Exception as error:
                warnings.append(
                    "screenshot unavailable: %s: %s" % (type(error).__name__, _text_of(error))
                )
            else:
                if written is True:
                    screenshot = capture_path
        return {"ok": True, "state": state, "screenshot": screenshot, "warnings": warnings}


# --------------------------------------------------------------------------
# Démarrer et arrêter
# --------------------------------------------------------------------------

_registry_lock = threading.Lock()
_running = None


def _say(line):
    """Écrit une ligne sur le flux d'erreur du processus, et sur le flux d'erreur courant quand une
    application l'a redirigé vers une console."""
    streams = []
    for stream in (sys.__stderr__, sys.stderr):
        if stream is not None and all(stream is not seen for seen in streams):
            streams.append(stream)
    for stream in streams:
        try:
            stream.write(line + "\n")
            stream.flush()
        except Exception:
            pass


def _check_port(port):
    if type(port) is not int or not 0 <= port <= 65535:
        raise BridgeError("the port must be a number between 0 and 65535")
    return port


def start(adapter, port, token_path, hello_timeout=HELLO_TIMEOUT_S):
    """Démarre le pont dans un fil d'arrière-plan et rend le port sur lequel il écoute.

    `port` 0 laisse le système choisir un port libre. `hello_timeout` est le temps dont dispose une
    nouvelle connexion pour envoyer sa première ligne ; seuls les tests le changent.
    """
    global _running
    _check_port(port)
    with _registry_lock:
        if _running is not None:
            raise BridgeError("a bridge is already running in this process; call stop() first")
        token = _read_token(token_path)
        bridge = _Bridge(adapter, port, token, float(hello_timeout))
        _running = bridge
    bridge.begin()
    _say("LMM bridge listening on %s:%d" % (LISTEN_ADDRESS, bridge.port))
    return bridge.port


def stop():
    """Cesse d'écouter et ferme la connexion. Un script encore en cours se termine de lui-même ;
    l'application elle-même n'est jamais touchée."""
    global _running
    with _registry_lock:
        bridge, _running = _running, None
    if bridge is not None:
        bridge.close()


def serve_main_thread(seconds=None):
    """Cède le fil appelant à l'application tant que le pont tourne. Un fichier de démarrage sans
    interface l'appelle à la place de sa boucle d'attente : seul le fil principal peut exécuter
    certains appels de l'application, un opérateur de rendu par exemple. Rend la main à l'arrêt du
    pont, après `seconds` s'il est donné, ou sur Ctrl+C."""
    bridge = _running
    if bridge is None:
        raise BridgeError("no bridge is running in this process; start it first")
    deadline = None if seconds is None else time.monotonic() + seconds

    def finished():
        return bridge._stop.is_set() or (deadline is not None and time.monotonic() >= deadline)

    serve = getattr(bridge._adapter, "serve_main_thread", None)
    try:
        if serve is not None:
            serve(finished)
        else:
            while not finished():
                time.sleep(_ACCEPT_POLL_S)
    except KeyboardInterrupt:
        pass


def _port_from_environment(text, default_port):
    if text is None or text == "":
        return default_port
    if len(text) > 5 or any(character not in "0123456789" for character in text):
        raise BridgeError("LIBREMODULAR_BRIDGE_PORT must be a port number")
    return _check_port(int(text))


def _default_token_path(environment):
    base = environment.get("XDG_CONFIG_HOME", "")
    if not os.path.isabs(base):
        home = environment.get("HOME", "")
        if not os.path.isabs(home):
            raise BridgeError(
                "cannot locate the token: set LIBREMODULAR_TOKEN_PATH or HOME"
            )
        base = os.path.join(home, ".config")
    return os.path.join(base, "libremodular-mcp", "token")


def _running_as_program():
    """Vrai quand ce fichier est le programme que Python a été prié d'exécuter, faux quand son texte
    a été exécuté dans la console d'une application."""
    own = _running_as_program.__code__.co_filename
    program = sys.argv[0] if sys.argv else ""
    if not program or own.startswith("<"):
        return False
    try:
        return os.path.samefile(own, program)
    except OSError:
        return False


def launch(adapter, default_port):
    """Démarre le pont avec le port et le chemin du jeton donnés par l'environnement.

    Dans une application cela rend aussitôt et le pont sert en arrière-plan ; exécuté comme
    programme, il sert jusqu'à l'arrêt du processus.
    """
    environment = os.environ
    port = _port_from_environment(environment.get("LIBREMODULAR_BRIDGE_PORT"), default_port)
    token_path = environment.get("LIBREMODULAR_TOKEN_PATH") or _default_token_path(environment)
    port = start(adapter, port, token_path)
    if _running_as_program():
        try:
            while True:
                time.sleep(3600)
        except KeyboardInterrupt:
            pass
        finally:
            stop()
    return port

# --- adaptateur : bridges/adapters/blender.py -----------------------------------

"""Adaptateur Blender : les noms qu'un script reçoit, et comment l'état de Blender est lu.

Pourquoi ce module existe
Le `bpy` de Blender doit être utilisé depuis le fil principal de Blender, qu'un autre fil ne peut
atteindre que par un minuteur avec l'interface, ou, sans interface, que si le fil principal lui cède la
main (`serve_main_thread`). Ce fichier est le seul endroit qui le sait, pour que le cœur reste le même
pour toutes les applications.
"""

import queue
import threading

DEFAULT_PORT = 9303
# Combien de fois, en secondes, le minuteur du fil principal de Blender cherche du travail.
_POLL_INTERVAL = 0.05


def _bpy():
    """Le module bpy, ou None quand ce processus ne peut pas le charger."""
    try:
        import bpy
    except Exception:
        return None
    return bpy


def _timers_run(bpy):
    """Dit si un minuteur enregistré ici s'exécutera jamais : seulement dans Blender avec son
    interface. En mode arrière-plan, et quand `bpy` est le paquet autonome, aucune boucle
    d'événements n'exécute de minuteurs, et un appel mis en file attendrait indéfiniment."""
    app = getattr(bpy, "app", None)
    if app is None or getattr(app, "background", True) is not False:
        return False
    timers = getattr(app, "timers", None)
    return timers is not None and callable(getattr(timers, "register", None))


def _selected(item):
    try:
        return bool(item.select_get())
    except Exception:
        return False


class BlenderAdapter:
    """Ce dont le cœur a besoin de Blender. Chaque appel dans Blender se fait dans une méthode, pour
    que ce fichier se charge sans Blender."""

    app = "blender"

    def __init__(self):
        self._pending = queue.Queue()
        self._polling = False
        self._serving = None
        self._serving_lock = threading.Lock()
        bpy = _bpy()
        # Enregistré une fois, depuis le fil qui construit l'adaptateur : dans Blender, le fil principal qui
        # exécute le fichier du pont. Les autres fils n'utilisent que la file.
        if (
            bpy is not None
            and threading.current_thread() is threading.main_thread()
            and _timers_run(bpy)
        ):
            bpy.app.timers.register(self._drain, first_interval=0.0, persistent=True)
            self._polling = True

    def _drain(self):
        while True:
            try:
                task = self._pending.get_nowait()
            except queue.Empty:
                return _POLL_INTERVAL
            task()

    def app_version(self):
        bpy = _bpy()
        if bpy is None:
            return "unknown"
        try:
            text = bpy.app.version_string
        except Exception:
            return "unknown"
        # « 4.2.3 LTS » se lit « 4.2.3 ».
        words = text.split() if isinstance(text, str) else []
        return words[0] if words else "unknown"

    def namespace(self):
        bpy = _bpy()
        return {"bpy": bpy} if bpy is not None else {}

    def serve_main_thread(self, finished):
        """Sans interface, exécute dans le fil appelant les appels que le pont met en file, jusqu'à ce que
        `finished()` soit vrai. Le fichier de démarrage appelle `serve_main_thread()` du cœur depuis le
        fil principal de Blender : un opérateur de rendu ne s'exécute que là."""
        with self._serving_lock:
            self._serving = threading.current_thread()
        try:
            while not finished():
                try:
                    task = self._pending.get(timeout=_POLL_INTERVAL)
                except queue.Empty:
                    continue
                task()
        finally:
            with self._serving_lock:
                self._serving = None
            # Un appel mis en file entre la dernière boucle et l'arrêt du service ne doit pas attendre sans fin.
            while True:
                try:
                    task = self._pending.get_nowait()
                except queue.Empty:
                    break
                task()

    def run_on_main(self, function):
        """Exécute `function` dans le fil principal de Blender et rend sa valeur ou son exception. Sans
        boucle qui serve (ni minuterie de l'interface ni `serve_main_thread`), ou depuis le fil qui sert,
        c'est un appel direct."""
        with self._serving_lock:
            serving = self._serving
            queued = self._polling or serving is not None
            if queued and (
                threading.current_thread() is threading.main_thread()
                or threading.current_thread() is serving
            ):
                queued = False
            if queued:
                done = threading.Event()
                outcome = {}

                def task():
                    try:
                        outcome["value"] = function()
                    except BaseException as error:  # relevée dans le fil qui attend
                        outcome["error"] = error
                    finally:
                        done.set()

                self._pending.put(task)
        if not queued:
            return function()
        done.wait()
        if "error" in outcome:
            raise outcome["error"]
        return outcome.get("value")

    def state(self):
        bpy = _bpy()
        if bpy is None:
            return {}
        scene = bpy.context.scene
        if scene is None:
            return {
                "scene": None,
                "objects": [],
                "selected_objects": [],
                "frame_range": None,
                "render_engine": None,
            }
        items = list(scene.objects)
        return {
            "scene": scene.name,
            "objects": [{"name": item.name, "type": item.type} for item in items],
            "selected_objects": [item.name for item in items if _selected(item)],
            "frame_range": {"start": scene.frame_start, "end": scene.frame_end},
            "render_engine": scene.render.engine,
        }

    def screenshot(self, path):
        """Pas de capture : un rendu devrait être lancé implicitement, et une copie de la vue 3D n'est pas
        disponible en sécurité sans l'interface."""
        return False


ADAPTER = BlenderAdapter

# --- point d'entrée, posé par scripts/build_bridges.py -----------------------

launch(ADAPTER(), DEFAULT_PORT)
