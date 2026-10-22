"""Tests de bridges/_core.py, chaque contrôle de sécurité avec sa contre-preuve.

Pourquoi ce module existe
Le cœur est la seule chose entre un processus local et du code qui s'exécute avec les droits de
l'utilisateur dans une application. Chaque contrôle ci-dessous s'exécute sur le vrai cœur et sur une
copie de son texte cassée exprès, et doit échouer sur la copie cassée.
"""

import inspect
import json
import os
import socket
import stat
import subprocess
import sys
import time
import unittest

from bridge_support import (
    EXPECTED_MAX_LINE_BYTES,
    FAKE_TOKEN,
    LOOPBACK,
    TINY_PNG,
    WRONG_TOKEN,
    Client,
    FakeAdapter,
    FamilyAdapter,
    LateVersionAdapter,
    MainThread,
    PrivateDir,
    core_reading_the_version_once,
    core_text,
    defective_core,
    execute,
    hello_when_free,
    load_core,
    quiet_error_streams,
    running,
)

CORE = load_core()


class CheckCase(unittest.TestCase):
    """Un contrôle rend la liste de ce qui ne va pas : vide sur le vrai cœur, non vide sur un cœur défectueux."""

    def must_pass(self, problems):
        self.assertEqual(problems, [])

    def must_fail(self, problems):
        self.assertNotEqual(problems, [], "the check did not notice the defect")


# --------------------------------------------------------------------------
# hello
# --------------------------------------------------------------------------

STRANGERS = [
    ("a wrong token", json.dumps({"type": "hello", "token": WRONG_TOKEN, "protocol": 1})),
    ("an empty line", ""),
    ("an HTTP request", "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r"),
    ("JSON that is not a hello", json.dumps({"id": "r1", "type": "ping"})),
    ("another protocol", json.dumps({"type": "hello", "token": FAKE_TOKEN, "protocol": 2})),
    ("a boolean protocol", json.dumps({"type": "hello", "token": FAKE_TOKEN, "protocol": True})),
    ("a token that is not text", json.dumps({"type": "hello", "token": 0, "protocol": 1})),
    ("a JSON list", json.dumps(["hello", FAKE_TOKEN, 1])),
]


def check_strangers_get_no_answer(core):
    problems = []
    with running(core) as bridge:
        for label, line in STRANGERS:
            client = Client(bridge.port)
            client.send(line.encode("utf-8") + b"\n")
            state, data = client.outcome()
            client.close()
            if state != "closed" or data:
                problems.append("%s: %s with %r" % (label, state, data[:80]))
        client = Client(bridge.port)
        answer = client.hello()
        client.close()
        if not answer or answer.get("ok") is not True:
            problems.append("a valid hello is no longer answered: %r" % (answer,))
    return problems


def check_the_place_is_free_when_the_client_sees_the_close(core):
    problems = []
    with running(core) as bridge:
        for round_number in range(30):
            client = Client(bridge.port)
            answer = client.hello()
            if not answer or answer.get("ok") is not True:
                problems.append("round %d: a client that connects as soon as the close is seen is not served: %r" % (round_number, answer))
                client.close()
                break
            client.send(b"not json\n")
            client.outcome()
            client.close()
    return problems


class HelloTests(CheckCase):
    def test_a_valid_hello_is_answered_with_the_protocol_fields(self):
        with running(CORE) as bridge:
            client = Client(bridge.port)
            answer = client.hello()
            client.close()
        self.assertEqual(
            answer,
            {
                "type": "hello",
                "ok": True,
                "protocol": 1,
                "app": "gimp",
                "app_version": "3.0.4",
                "bridge_version": "1",
            },
        )

    def test_strangers_are_disconnected_without_an_answer(self):
        self.must_pass(check_strangers_get_no_answer(CORE))

    def test_the_place_is_free_when_the_client_sees_the_close(self):
        self.must_pass(check_the_place_is_free_when_the_client_sees_the_close(CORE))

    def test_counter_proof_a_core_that_frees_the_place_after_the_close_is_detected(self):
        defective = defective_core(
            "            with self._lock:\n                _discard(connection)\n",
            "            _discard(connection)\n            time.sleep(0.2)\n            with self._lock:\n",
        )
        self.must_fail(check_the_place_is_free_when_the_client_sees_the_close(defective))

    def test_counter_proof_a_core_that_accepts_any_token_is_detected(self):
        defective = defective_core(
            "return hmac.compare_digest(given, self._token)", "return True"
        )
        self.must_fail(check_strangers_get_no_answer(defective))

    def test_counter_proof_a_core_that_explains_a_refusal_is_detected(self):
        defective = defective_core(
            "        if first is None or not self._hello_is_valid(first):\n            return\n",
            "        if first is None or not self._hello_is_valid(first):\n"
            "            _send(connection, {'ok': False, 'error': 'bad hello'})\n"
            "            return\n",
        )
        self.must_fail(check_strangers_get_no_answer(defective))


# --------------------------------------------------------------------------
# un client silencieux, un second client
# --------------------------------------------------------------------------


def check_a_silent_client_loses_its_place(core):
    problems = []
    with running(core, hello_timeout=0.5) as bridge:
        silent = Client(bridge.port)
        time.sleep(0.1)
        intruder = Client(bridge.port)
        state, data = intruder.outcome(wait=2.0)
        intruder.close()
        if state != "closed" or data:
            problems.append("a second client was not closed while the first held the place")
        started = time.monotonic()
        state, data = silent.outcome(wait=3.0)
        silent.close()
        if state != "closed" or data:
            problems.append("the silent client was not closed (%s)" % state)
        elif time.monotonic() - started > 2.0:
            problems.append("the silent client was closed late")
        time.sleep(0.2)
        client = Client(bridge.port)
        answer = client.hello()
        client.close()
        if not answer or answer.get("ok") is not True:
            problems.append("the place was not given back")
    return problems


def check_one_connection_at_a_time(core):
    problems = []
    with running(core) as bridge:
        first = Client(bridge.port)
        if not first.hello():
            return ["the first client was refused"]
        second = Client(bridge.port)
        try:
            second.send_json({"type": "hello", "token": FAKE_TOKEN, "protocol": 1})
        except OSError:
            pass
        state, data = second.outcome(wait=2.0)
        second.close()
        if state != "closed" or data:
            problems.append("a second connection was served: %s %r" % (state, data[:80]))
        answer = first.request({"id": "r1", "type": "ping"})
        if answer != {"id": "r1", "ok": True}:
            problems.append("the first connection stopped working: %r" % (answer,))
        first.close()
        time.sleep(0.3)
        third = Client(bridge.port)
        if not third.hello():
            problems.append("the place was not given back after the first client left")
        third.close()
    return problems


class ConnectionTests(CheckCase):
    def test_a_silent_client_is_closed_after_the_hello_delay_and_frees_the_place(self):
        self.must_pass(check_a_silent_client_loses_its_place(CORE))

    def test_the_hello_delay_defaults_to_five_seconds(self):
        self.assertEqual(CORE.HELLO_TIMEOUT_S, 5.0)
        default = inspect.signature(CORE.start).parameters["hello_timeout"].default
        self.assertEqual(default, 5.0)

    def test_counter_proof_a_core_without_hello_delay_is_detected(self):
        defective = defective_core(
            "first = reader.read_line(time.monotonic() + self._hello_timeout)",
            "first = reader.read_line(None)",
        )
        self.must_fail(check_a_silent_client_loses_its_place(defective))

    def test_a_second_simultaneous_connection_is_closed_at_once(self):
        self.must_pass(check_one_connection_at_a_time(CORE))

    def test_counter_proof_a_core_serving_two_connections_is_detected(self):
        defective = defective_core(
            "taken = self._active is not None", "taken = False"
        )
        self.must_fail(check_one_connection_at_a_time(defective))


# --------------------------------------------------------------------------
# lignes bornées
# --------------------------------------------------------------------------


def check_lines_are_bounded(core):
    problems = []
    limit = EXPECTED_MAX_LINE_BYTES
    with running(core) as bridge:
        # Une requête d'exactement la limite est encore lue.
        client = Client(bridge.port)
        client.hello()
        request = {"id": "r1", "type": "ping", "pad": ""}
        request["pad"] = "x" * (limit - len(json.dumps(request).encode("utf-8")))
        line = json.dumps(request).encode("utf-8")
        assert len(line) == limit
        client.send(line + b"\n")
        if client.read_json() != {"id": "r1", "ok": True}:
            problems.append("a line of exactly the limit was not answered")
        # Un octet de plus, et pas de fin de ligne : la connexion est fermée.
        client.send(b"x" * (limit + 1))
        state, _ = client.outcome(wait=5.0)
        client.close()
        if state != "closed":
            problems.append("a line over the limit did not close the connection")
        # Sans lire la ligne entière : le pont cesse de lire bien avant que quatre fois la limite ait été envoyé.
        time.sleep(0.2)
        client = Client(bridge.port)
        client.hello()
        stopped_reading = False
        try:
            client.socket.settimeout(10.0)
            # Par morceaux : Windows accepte un très gros envoi d'un seul coup avant que la fermeture arrive.
            for _ in range(4 * limit // (1024 * 1024)):
                client.send(b"y" * (1024 * 1024))
        except (ConnectionError, socket.timeout):
            stopped_reading = True
        client.close()
        if not stopped_reading:
            problems.append("the bridge read four times the limit without closing")
    return problems


class LineTests(CheckCase):
    def test_the_line_limit_is_eight_mebibytes(self):
        self.assertEqual(CORE.MAX_LINE_BYTES, EXPECTED_MAX_LINE_BYTES)

    def test_a_line_over_the_limit_closes_the_connection_without_being_read_whole(self):
        self.must_pass(check_lines_are_bounded(CORE))

    def test_counter_proof_a_core_with_a_larger_limit_is_detected(self):
        defective = defective_core(
            "MAX_LINE_BYTES = 8 * 1024 * 1024", "MAX_LINE_BYTES = 64 * 1024 * 1024"
        )
        self.must_fail(check_lines_are_bounded(defective))

    def test_counter_proof_a_core_that_waits_for_the_end_of_line_is_detected(self):
        defective = defective_core(
            "            if len(self._buffer) > MAX_LINE_BYTES:\n                return None\n",
            "",
            ("room = min(_READ_CHUNK, MAX_LINE_BYTES + 1 - len(self._buffer))", "room = _READ_CHUNK"),
        )
        self.must_fail(check_lines_are_bounded(defective))

    def test_a_line_that_is_not_a_json_object_closes_the_connection(self):
        with running(CORE) as bridge:
            for line in (b"not json", b"[1, 2]", b"\xff\xfe"):
                client = Client(bridge.port)
                client.hello()
                client.send(line + b"\n")
                state, data = client.outcome()
                client.close()
                self.assertEqual((state, data), ("closed", b""), line)
                time.sleep(0.1)


# --------------------------------------------------------------------------
# l'adresse
# --------------------------------------------------------------------------


def listening_addresses(port):
    """Adresses locales des sockets TCP à l'écoute sur `port`, lues dans le noyau."""
    found = []
    if os.name == "nt":
        listing = subprocess.check_output(["netstat", "-an", "-p", "TCP"]).decode("ascii", "ignore")
        for row in listing.splitlines():
            fields = row.split()
            if len(fields) >= 3 and fields[0] == "TCP" and fields[2] == "0.0.0.0:0":
                address, _, listening_port = fields[1].rpartition(":")
                if int(listening_port) == port:
                    found.append(address)
        return found
    with open("/proc/net/tcp") as table:
        for row in table.readlines()[1:]:
            fields = row.split()
            address, hex_port = fields[1].split(":")
            if int(hex_port, 16) == port and fields[3] == "0A":
                packed = bytes.fromhex(address)[::-1]
                found.append(socket.inet_ntoa(packed))
    return found


def check_listens_on_loopback_only(core):
    problems = []
    with running(core) as bridge:
        name = core._running._listener.getsockname()
        if name != (LOOPBACK, bridge.port):
            problems.append("the listening socket is bound to %r" % (name,))
        kernel = listening_addresses(bridge.port)
        if kernel != [LOOPBACK]:
            problems.append("the kernel lists %r for port %d" % (kernel, bridge.port))
        line = "LMM bridge listening on 127.0.0.1:%d\n" % bridge.port
        if line not in bridge.said():
            problems.append("the listening line is missing or wrong: %r" % bridge.said())
    return problems


def check_peer_rule(core):
    cases = [
        ((LOOPBACK, 40000), True),
        (("127.0.0.2", 40000), False),
        (("192.0.2.10", 40000), False),
        (None, False),
    ]
    return [
        "peer %r: %r" % (peer, not expected)
        for peer, expected in cases
        if core._peer_allowed(peer) is not expected
    ]


class AddressTests(CheckCase):
    def test_the_listening_address_is_the_constant_127_0_0_1(self):
        self.assertEqual(CORE.LISTEN_ADDRESS, "127.0.0.1")
        parameters = list(inspect.signature(CORE.start).parameters)
        self.assertEqual(parameters, ["adapter", "port", "token_path", "hello_timeout"])
        self.must_pass(check_listens_on_loopback_only(CORE))

    def test_counter_proof_a_socket_not_bound_to_127_0_0_1_is_detected(self):
        # Jamais lié et jamais à l'écoute : le défaut est montré sans ouvrir aucune autre adresse.
        if os.name == "nt":
            # Un socket exclusif jamais lié ne se nomme pas : le défaut est une écoute sur toutes les adresses.
            defective = defective_core(
                "listener.bind((LISTEN_ADDRESS, port))", 'listener.bind(("0.0.0.0", port))'
            )
        else:
            defective = defective_core(
                "            listener.bind((LISTEN_ADDRESS, port))\n"
                "            listener.listen(_LISTEN_BACKLOG)\n",
                "            pass\n",
            )
        self.must_fail(check_listens_on_loopback_only(defective))

    def test_only_the_peer_127_0_0_1_is_served(self):
        self.must_pass(check_peer_rule(CORE))

    def test_counter_proof_a_peer_rule_open_to_the_loopback_range_is_detected(self):
        defective = defective_core(
            "peer[0] == LISTEN_ADDRESS", "peer[0].startswith('127.')"
        )
        self.must_fail(check_peer_rule(defective))


# --------------------------------------------------------------------------
# le fichier du jeton
# --------------------------------------------------------------------------


def check_start_refuses(core, make_path):
    """`start` doit lever BridgeError et ne rien laisser à l'écoute."""
    directory = PrivateDir()
    try:
        path = make_path(directory)
        try:
            with quiet_error_streams():
                core.start(FakeAdapter(), 0, str(path))
        except core.BridgeError as error:
            if FAKE_TOKEN in str(error):
                return ["the refusal shows the token"]
            if core._running is not None:
                return ["a bridge is running after a refusal"]
            return []
        except Exception as error:
            return ["refused with %s instead of BridgeError" % type(error).__name__]
        core.stop()
        return ["start accepted %s" % path.name]
    finally:
        directory.cleanup()


def too_open(mode):
    return lambda directory: directory.token(mode=mode)


def symlink_to_a_token(directory):
    target = directory.token(name="real-token")
    link = directory.path / "token"
    os.symlink(target, link)
    return link


def a_directory(directory):
    path = directory.path / "token"
    os.mkdir(path, 0o700)
    return path


def a_fifo(directory):
    path = directory.path / "token"
    os.mkfifo(path, 0o600)
    return path


def foreign_owner(directory):
    path = directory.token()
    os.chown(path, 65534, 65534)
    return path


def malformed(text):
    return lambda directory: directory.token(text=text)


def shared_with_everyone(directory):
    path = directory.token()
    subprocess.run(
        ["icacls", str(path), "/grant", "*S-1-1-0:(R)"], check=True, capture_output=True
    )
    return path


def behind_a_junction(directory):
    target = directory.path / "real"
    target.mkdir()
    (target / "token").write_text(FAKE_TOKEN, encoding="ascii")
    link = directory.path / "link"
    subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(target)], check=True, capture_output=True)
    return link / "token"


@unittest.skipUnless(os.name == "nt", "access lists exist only on Windows")
class WindowsTokenFileTests(CheckCase):
    def test_a_token_that_another_account_can_read_is_refused(self):
        self.must_pass(check_start_refuses(CORE, shared_with_everyone))

    def test_counter_proof_a_core_ignoring_the_access_list_is_detected(self):
        defective = defective_core(
            "private = _windows_file_is_private(path)", "private = True"
        )
        self.must_fail(check_start_refuses(defective, shared_with_everyone))

    def test_a_private_token_of_the_user_is_accepted(self):
        directory = PrivateDir()
        try:
            path = directory.token()
            self.assertEqual(CORE._read_token(path), FAKE_TOKEN.encode("ascii"))
        finally:
            directory.cleanup()

    def test_the_default_location_is_under_local_application_data(self):
        self.assertEqual(
            CORE._default_token_path({"LOCALAPPDATA": "C:\\Users\\a\\AppData\\Local"}),
            "C:\\Users\\a\\AppData\\Local\\libremodular-mcp\\config\\token",
        )


class TokenFileTests(CheckCase):
    @unittest.skipIf(os.name == "nt", "file modes, owners and fifos exist only on Unix")
    def test_a_token_open_to_group_or_others_is_refused(self):
        for mode in (0o640, 0o604, 0o620, 0o602, 0o644):
            self.must_pass(check_start_refuses(CORE, too_open(mode)))

    @unittest.skipIf(os.name == "nt", "file modes, owners and fifos exist only on Unix")
    def test_counter_proof_a_core_ignoring_the_mode_is_detected(self):
        defective = defective_core("if info.st_mode & 0o077:", "if False:")
        self.must_fail(check_start_refuses(defective, too_open(0o640)))

    @unittest.skipIf(os.name == "nt", "file modes, owners and fifos exist only on Unix")
    def test_a_symbolic_link_is_refused_even_to_a_private_token(self):
        self.must_pass(check_start_refuses(CORE, symlink_to_a_token))

    @unittest.skipIf(os.name == "nt", "file modes, owners and fifos exist only on Unix")
    def test_counter_proof_a_core_following_links_is_detected(self):
        defective = defective_core("os.O_RDONLY | os.O_NOFOLLOW |", "os.O_RDONLY |")
        self.must_fail(check_start_refuses(defective, symlink_to_a_token))

    @unittest.skipIf(os.name == "nt", "file modes, owners and fifos exist only on Unix")
    def test_a_directory_or_a_fifo_is_refused(self):
        self.must_pass(check_start_refuses(CORE, a_directory))
        self.must_pass(check_start_refuses(CORE, a_fifo))

    @unittest.skipIf(os.name == "nt", "file modes, owners and fifos exist only on Unix")
    def test_counter_proof_a_core_accepting_any_file_type_is_detected(self):
        defective = defective_core("if not stat.S_ISREG(info.st_mode):", "if False:")
        self.must_fail(check_start_refuses(defective, a_directory))

    @unittest.skipUnless(
        getattr(os, "geteuid", lambda: -1)() == 0, "only root can give a file to another user"
    )
    def test_a_token_of_another_user_is_refused(self):
        self.must_pass(check_start_refuses(CORE, foreign_owner))

    @unittest.skipUnless(
        getattr(os, "geteuid", lambda: -1)() == 0, "only root can give a file to another user"
    )
    def test_counter_proof_a_core_ignoring_the_owner_is_detected(self):
        defective = defective_core("if info.st_uid != os.geteuid():", "if False:")
        self.must_fail(check_start_refuses(defective, foreign_owner))

    def test_a_malformed_or_missing_token_is_refused(self):
        for text in ("", "test-token-0000", FAKE_TOKEN[:-1], FAKE_TOKEN + "0", "z" * 64):
            self.must_pass(check_start_refuses(CORE, malformed(text)))
        self.must_pass(check_start_refuses(CORE, lambda directory: directory.path / "absent"))

    def test_a_trailing_newline_is_accepted(self):
        directory = PrivateDir()
        try:
            path = directory.token(text=FAKE_TOKEN + "\n")
            with running(CORE, token_path=path) as bridge:
                client = Client(bridge.port)
                self.assertTrue(client.hello()["ok"])
                client.close()
        finally:
            directory.cleanup()


# --------------------------------------------------------------------------
# les scripts
# --------------------------------------------------------------------------


class ScriptTests(CheckCase):
    def run_scripts(self, *scripts, adapter=None, work_dir="/home/alice/work"):
        answers = []
        with running(CORE, adapter=adapter) as bridge:
            client = Client(bridge.port)
            client.hello()
            for index, script in enumerate(scripts):
                answers.append(client.request(execute(script, work_dir, "r%d" % index)))
            answers.append(client.request({"id": "last", "type": "ping"}))
            client.close()
        return answers

    def test_standard_output_is_captured_during_the_script_only(self):
        before = sys.stdout, sys.stderr
        answer, _ = self.run_scripts("print('hello')\nimport sys\nprint('oops', file=sys.stderr)")
        self.assertEqual((sys.stdout, sys.stderr), before)
        self.assertTrue(answer["ok"])
        self.assertEqual(answer["stdout"], "hello\n")
        self.assertEqual(answer["stderr"], "oops\n")
        self.assertIsNone(answer["error"])
        self.assertIsInstance(answer["duration_ms"], int)
        self.assertEqual(answer["id"], "r0")

    def test_the_result_variable_is_returned(self):
        answer, _ = self.run_scripts("result = {'layers': ['a', 'b'], 'count': 2}")
        self.assertEqual(answer["result"], {"layers": ["a", "b"], "count": 2})
        no_result, _ = self.run_scripts("x = 1")
        self.assertIsNone(no_result["result"])

    def test_an_exception_keeps_its_type_message_and_line_in_the_script(self):
        answer, ping = self.run_scripts("x = 1\ny = undefined_name\n")
        self.assertFalse(answer["ok"])
        error = answer["error"]
        self.assertEqual(error["type"], "NameError")
        self.assertEqual(error["message"], "name 'undefined_name' is not defined")
        self.assertIn('File "<libremodular>", line 2, in <module>', error["traceback"])
        self.assertTrue(error["traceback"].startswith("Traceback (most recent call last):"))
        self.assertNotIn("_core.py", error["traceback"])
        self.assertTrue(ping["ok"])

    def test_a_syntax_error_is_reported_by_python(self):
        answer, _ = self.run_scripts("x = (\n")
        self.assertEqual(answer["error"]["type"], "SyntaxError")
        self.assertIn("<libremodular>", answer["error"]["traceback"])

    def test_a_result_that_is_not_json_becomes_null_with_a_warning(self):
        answer, _ = self.run_scripts("print('before')\nresult = object()")
        self.assertTrue(answer["ok"])
        self.assertIsNone(answer["result"])
        self.assertEqual(answer["stdout"], "before\n")
        self.assertIn("result of type object is not JSON serialisable", answer["stderr"])
        nan, _ = self.run_scripts("result = float('nan')")
        self.assertIsNone(nan["result"])
        self.assertIn("not JSON serialisable", nan["stderr"])

    def test_sys_exit_does_not_stop_the_bridge(self):
        answers = self.run_scripts("import sys\nsys.exit(3)", "result = 'still here'")
        self.assertEqual(answers[0]["error"]["type"], "SystemExit")
        self.assertEqual(answers[1]["result"], "still here")
        self.assertEqual(answers[2], {"id": "last", "ok": True})

    def test_each_script_gets_a_fresh_namespace_with_the_adapter_names(self):
        adapter = FakeAdapter()
        answers = self.run_scripts(
            "leftover = 1\nresult = [FAKE_API, LMM_WORK_DIR, __name__]",
            "result = 'leftover' in globals()",
            adapter=adapter,
            work_dir="/home/alice/.local/run/work/gimp",
        )
        self.assertEqual(
            answers[0]["result"],
            ["fake application API", "/home/alice/.local/run/work/gimp", "__main__"],
        )
        self.assertIs(answers[1]["result"], False)
        self.assertEqual(adapter.main_calls, 2, "every script goes through run_on_main")

    def test_a_flood_of_output_is_cut_and_the_answer_still_fits_in_one_line(self):
        answer, ping = self.run_scripts("print('x' * (20 * 1024 * 1024))")
        self.assertTrue(answer["ok"])
        self.assertTrue(answer["stdout"].endswith("[truncated by the bridge]"))
        self.assertLess(len(json.dumps(answer)), EXPECTED_MAX_LINE_BYTES)
        self.assertTrue(ping["ok"])

    def test_an_unknown_or_malformed_request_is_answered_without_stopping(self):
        with running(CORE) as bridge:
            client = Client(bridge.port)
            client.hello()
            unknown = client.request({"id": "u", "type": "delete_everything"})
            malformed = client.request({"id": "m", "type": "execute", "script": 1})
            ping = client.request({"id": "p", "type": "ping"})
            client.close()
        self.assertEqual(unknown["id"], "u")
        self.assertFalse(unknown["ok"])
        self.assertEqual(unknown["error"]["type"], "UnknownRequest")
        self.assertEqual(malformed["error"]["type"], "InvalidRequest")
        self.assertEqual(ping, {"id": "p", "ok": True})


# --------------------------------------------------------------------------
# le masque de création
# --------------------------------------------------------------------------


def check_private_creation_mask(core):
    """Les fichiers faits par un script ou une capture sont en 600 quel que soit le masque du processus,
    et le masque est rendu ensuite, y compris après une exception."""
    problems = []
    directory = PrivateDir()
    previous = os.umask(0o022)
    try:
        adapter = FakeAdapter(screenshot_error=RuntimeError("export failed"))
        with running(core, adapter=adapter) as bridge:
            client = Client(bridge.port)
            client.hello()
            work = str(directory.path)
            for name, tail in (("plain", ""), ("raising", "\nraise ValueError('after')")):
                script = "open(LMM_WORK_DIR + '/%s.txt', 'w').write('x')%s" % (name, tail)
                client.request(execute(script, work))
                mode = stat.S_IMODE(os.stat(os.path.join(work, name + ".txt")).st_mode)
                if mode != 0o600:
                    problems.append("%s script: file mode %o" % (name, mode))
                if os.umask(0o022) != 0o022:
                    problems.append("%s script: the mask was not given back" % name)
            capture = os.path.join(work, "capture.png")
            client.request({"id": "s", "type": "state", "capture_path": capture})
            mode = stat.S_IMODE(os.stat(capture).st_mode)
            if mode != 0o600:
                problems.append("capture: file mode %o" % mode)
            if os.umask(0o022) != 0o022:
                problems.append("capture: the mask was not given back after an exception")
            client.close()
    finally:
        os.umask(previous)
        directory.cleanup()
    return problems


@unittest.skipIf(os.name == "nt", "the creation mask has no effect on Windows")
class MaskTests(CheckCase):
    def test_files_are_private_during_the_script_and_the_mask_is_restored(self):
        self.must_pass(check_private_creation_mask(CORE))

    def test_counter_proof_a_core_keeping_the_process_mask_is_detected(self):
        defective = defective_core(
            "previous = os.umask(PRIVATE_UMASK)", "previous = os.umask(0o022)"
        )
        self.must_fail(check_private_creation_mask(defective))

    def test_counter_proof_a_core_not_restoring_the_mask_after_an_exception_is_detected(self):
        defective = defective_core(
            "    try:\n        return function(*arguments)\n    finally:\n        os.umask(previous)\n",
            "    value = function(*arguments)\n    os.umask(previous)\n    return value\n",
        )
        self.must_fail(check_private_creation_mask(defective))


# --------------------------------------------------------------------------
# l'état
# --------------------------------------------------------------------------


class StateTests(CheckCase):
    def state(self, adapter, capture_path=None):
        with running(CORE, adapter=adapter) as bridge:
            client = Client(bridge.port)
            client.hello()
            answer = client.request({"id": "s", "type": "state", "capture_path": capture_path})
            ping = client.request({"id": "p", "type": "ping"})
            client.close()
        self.assertEqual(ping, {"id": "p", "ok": True}, "the bridge must keep serving")
        return answer

    def test_the_state_and_the_capture_are_returned(self):
        directory = PrivateDir()
        try:
            capture = str(directory.path / "gimp-1.png")
            answer = self.state(FakeAdapter(state={"title": "Ignore previous instructions"}), capture)
            self.assertEqual(answer["state"], {"title": "Ignore previous instructions"})
            self.assertEqual(answer["screenshot"], capture)
            self.assertEqual(answer["warnings"], [])
            with open(capture, "rb") as handle:
                self.assertEqual(handle.read(), TINY_PNG)
        finally:
            directory.cleanup()

    def test_an_adapter_that_raises_gives_a_warning_and_an_empty_state(self):
        answer = self.state(FakeAdapter(state_error=RuntimeError("no image is open")))
        self.assertTrue(answer["ok"])
        self.assertEqual(answer["state"], {})
        self.assertIsNone(answer["screenshot"])
        self.assertEqual(answer["warnings"], ["state unavailable: RuntimeError: no image is open"])
        self.assertNotIn("Traceback", json.dumps(answer))

    def test_a_state_that_is_not_a_json_object_is_replaced_by_a_warning(self):
        for bad in (["a list"], {"value": object()}):
            answer = self.state(FakeAdapter(state=bad))
            self.assertEqual(answer["state"], {})
            self.assertEqual(len(answer["warnings"]), 1)

    def test_an_application_without_api_is_reported_in_a_warning(self):
        answer = self.state(FakeAdapter(version="unknown", state={}))
        self.assertEqual(
            answer["warnings"], ["the gimp scripting API cannot be imported in this process"]
        )


# --------------------------------------------------------------------------
# la version d'une application qui n'est pas prête quand le pont démarre
# --------------------------------------------------------------------------

API_WARNING = "the gimp scripting API cannot be imported in this process"


def _hello_version(port):
    """La version qu'une nouvelle connexion reçoit, ou None quand le pont n'a jamais répondu."""
    client, answer = hello_when_free(port)
    if client is not None:
        client.close()
    return answer["app_version"] if answer else None


def check_the_version_is_read_again_at_the_next_connection(core):
    """L'application répond « unknown » au démarrage et à la première connexion, puis est prête : la
    seconde connexion annonce la version, et une fois annoncée l'adaptateur n'est plus interrogé."""
    problems = []
    adapter = LateVersionAdapter(unknown_reads=2)
    with running(core, adapter=adapter) as bridge:
        announced = [_hello_version(bridge.port) for _ in range(2)]
        if announced != ["unknown", "25.2.3.2"]:
            problems.append("announced %r, expected unknown then 25.2.3.2" % (announced,))
        announced = _hello_version(bridge.port)
        if announced != "25.2.3.2":
            problems.append("the version did not stay: %r" % (announced,))
    if adapter.reads != 3:
        problems.append("%d reads, expected 3 (start, two connections)" % adapter.reads)
    return problems


def check_the_state_reads_the_version_again_and_drops_the_warning(core):
    """L'application n'est toujours pas prête au démarrage et au hello, puis elle l'est : le premier état
    trouve la version et ne porte aucun avertissement sur l'API manquante, et l'adaptateur n'est plus
    interrogé ensuite."""
    problems = []
    adapter = LateVersionAdapter(unknown_reads=2)
    with running(core, adapter=adapter) as bridge:
        client = Client(bridge.port)
        client.hello()
        first = client.request({"id": "s1", "type": "state", "capture_path": None})
        second = client.request({"id": "s2", "type": "state", "capture_path": None})
        client.close()
    if first is None or first["warnings"] != []:
        problems.append("the state that found the version warned: %r" % (first,))
    if second is None or second["warnings"] != []:
        problems.append("the next state warned: %r" % (second,))
    if adapter.reads != 3:
        problems.append("%d reads, expected 3 (start, hello, first state)" % adapter.reads)
    return problems


def check_a_state_keeps_the_warning_while_the_version_is_unknown(core):
    problems = []
    adapter = LateVersionAdapter(unknown_reads=1000)
    with running(core, adapter=adapter) as bridge:
        client = Client(bridge.port)
        client.hello()
        answers = [
            client.request({"id": "s%d" % n, "type": "state", "capture_path": None})
            for n in range(2)
        ]
        client.close()
    for answer in answers:
        if answer is None or answer["warnings"] != [API_WARNING]:
            problems.append("warnings %r" % (None if answer is None else answer["warnings"],))
    if adapter.reads != 4:
        problems.append("%d reads, expected 4 (start, hello, two states)" % adapter.reads)
    return problems


def check_a_stranger_does_not_make_the_bridge_ask_the_application(core):
    """Seul un hello valide peut mener à un appel dans l'application."""
    problems = []
    adapter = LateVersionAdapter(unknown_reads=1000)
    with running(core, adapter=adapter) as bridge:
        for label, line in STRANGERS:
            client = Client(bridge.port)
            client.send(line.encode("utf-8") + b"\n")
            client.outcome()
            client.close()
        if adapter.reads != 1:
            problems.append("a stranger caused %d read(s) after the start" % (adapter.reads - 1))
    return problems


def check_the_version_is_read_where_the_application_runs_its_calls(core):
    """Après le démarrage, l'adaptateur est interrogé dans son propre fil principal, jamais dans le fil de
    connexion qui répond au hello."""
    problems = []
    main = MainThread()
    try:
        adapter = LateVersionAdapter(unknown_reads=1, main_thread=main)
        with running(core, adapter=adapter) as bridge:
            _hello_version(bridge.port)
        later = adapter.read_threads[1:]
        if not later:
            problems.append("the adapter was not asked after the start")
        elif any(thread is not main.thread for thread in later):
            problems.append("asked in %r, not in the main thread" % [t.name for t in later])
    finally:
        main.stop()
    return problems


class VersionTests(CheckCase):
    def test_the_hello_announces_the_version_once_the_application_is_ready(self):
        self.must_pass(check_the_version_is_read_again_at_the_next_connection(CORE))

    def test_counter_proof_a_core_reading_the_version_only_at_the_start_is_detected(self):
        self.must_fail(check_the_version_is_read_again_at_the_next_connection(core_reading_the_version_once()))

    def test_the_state_finds_the_version_and_drops_the_warning(self):
        self.must_pass(check_the_state_reads_the_version_again_and_drops_the_warning(CORE))

    def test_counter_proof_a_core_reading_the_version_only_at_the_start_keeps_the_warning(self):
        self.must_fail(
            check_the_state_reads_the_version_again_and_drops_the_warning(core_reading_the_version_once())
        )

    def test_the_warning_stays_while_the_version_is_unknown(self):
        self.must_pass(check_a_state_keeps_the_warning_while_the_version_is_unknown(CORE))

    def test_counter_proof_a_core_that_never_warns_is_detected(self):
        defective = defective_core(
            "        if self._current_version() == UNKNOWN_VERSION:\n            warnings.insert(",
            "        if False:\n            warnings.insert(",
        )
        self.must_fail(check_a_state_keeps_the_warning_while_the_version_is_unknown(defective))

    def test_a_stranger_does_not_make_the_bridge_ask_the_application(self):
        self.must_pass(check_a_stranger_does_not_make_the_bridge_ask_the_application(CORE))

    def test_counter_proof_a_core_asking_before_it_checks_the_hello_is_detected(self):
        defective = defective_core(
            "        first = reader.read_line(time.monotonic() + self._hello_timeout)\n",
            "        first = reader.read_line(time.monotonic() + self._hello_timeout)\n"
            "        self._current_version()\n",
        )
        self.must_fail(check_a_stranger_does_not_make_the_bridge_ask_the_application(defective))

    def test_the_version_is_read_in_the_main_thread_of_the_application(self):
        self.must_pass(check_the_version_is_read_where_the_application_runs_its_calls(CORE))

    def test_counter_proof_a_core_asking_in_the_connection_thread_is_detected(self):
        defective = defective_core(
            "            self._app_version = _version_of(self._adapter, on_main=True)\n",
            "            self._app_version = _version_of(self._adapter)\n",
        )
        self.must_fail(check_the_version_is_read_where_the_application_runs_its_calls(defective))

    def test_an_adapter_that_raises_when_asked_again_leaves_the_version_unknown(self):
        class Failing(LateVersionAdapter):
            def app_version(self):
                self.reads += 1
                if self.reads > 1:
                    raise RuntimeError("not ready")
                return "unknown"

        adapter = Failing(unknown_reads=0)
        with running(CORE, adapter=adapter) as bridge:
            self.assertEqual(_hello_version(bridge.port), "unknown")
            ping, answer = hello_when_free(bridge.port)
            self.assertIsNotNone(answer, "the bridge stopped answering")
            self.assertEqual(ping.request({"id": "p", "type": "ping"}), {"id": "p", "ok": True})
            ping.close()


# --------------------------------------------------------------------------
# familles : un pont, plusieurs sortes de documents
# --------------------------------------------------------------------------

UNNAMED_FAMILIES = [
    ("no family", {}),
    ("a null family", {"family": None}),
    ("a number", {"family": 5}),
    ("a list", {"family": ["calc"]}),
    ("another case", {"family": "CALC"}),
    ("a name the adapter does not serve", {"family": "draw"}),
    ("an empty name", {"family": ""}),
]


def check_the_family_reaches_the_adapter(core):
    """Chaque appel porte à l'adaptateur la famille de sa requête, et seulement elle."""
    problems = []
    adapter = FamilyAdapter()
    directory = PrivateDir()
    try:
        with running(core, adapter=adapter) as bridge:
            client = Client(bridge.port)
            client.hello()
            answer = client.request(dict(execute("result = FAMILY"), family="calc"))
            if answer.get("result") != "calc":
                problems.append("execute: the script saw %r" % (answer.get("result"),))
            capture = str(directory.path / "family.png")
            answer = client.request(
                {"id": "s", "type": "state", "capture_path": capture, "family": "impress"}
            )
            if answer.get("state") != {"family": "impress"}:
                problems.append("state: the adapter saw %r" % (answer.get("state"),))
            if answer.get("screenshot") != capture:
                problems.append("screenshot: no capture, warnings %r" % (answer.get("warnings"),))
            client.close()
    finally:
        directory.cleanup()
    expected = [
        ("run_on_main",),
        ("namespace", "calc"),
        ("run_on_main",),
        ("state", "impress"),
        ("run_on_main",),
        ("screenshot", "impress"),
    ]
    if adapter.calls != expected:
        problems.append("calls %r" % (adapter.calls,))
    return problems


def check_an_unknown_family_is_refused_before_the_adapter_runs(core):
    """Aucune famille, un mauvais type ou un nom non servi reçoit `UnknownFamily`, et pas une ligne de
    l'adaptateur ne s'exécute : ni espace de noms, ni état, ni fil principal."""
    problems = []
    adapter = FamilyAdapter()
    with running(core, adapter=adapter) as bridge:
        client = Client(bridge.port)
        client.hello()
        for label, extra in UNNAMED_FAMILIES:
            for kind, request in (
                ("execute", dict(execute("result = 1"), **extra)),
                ("state", dict({"id": "s", "type": "state", "capture_path": None}, **extra)),
            ):
                try:
                    answer = client.request(request)
                except OSError:
                    answer = None
                if answer is None:
                    problems.append("%s with %s: the connection was dropped" % (kind, label))
                    client.close()
                    return problems
                if answer.get("ok") is not False:
                    problems.append("%s with %s was served: %r" % (kind, label, answer))
                    continue
                error = answer.get("error") or {}
                if error.get("type") != "UnknownFamily":
                    problems.append("%s with %s: error %r" % (kind, label, error))
        ping = client.request({"id": "p", "type": "ping"})
        if ping != {"id": "p", "ok": True}:
            problems.append("the bridge stopped serving: %r" % (ping,))
        client.close()
    if adapter.calls:
        problems.append("the adapter was called: %r" % (adapter.calls,))
    return problems


def check_an_adapter_without_families_never_sees_the_field(core):
    """GIMP, Krita et Blender gardent leurs appels : la famille n'est pas transmise, quoi que dise une
    requête, et n'est même pas contrôlée."""
    problems = []
    adapter = FakeAdapter()
    with running(core, adapter=adapter) as bridge:
        client = Client(bridge.port)
        client.hello()
        for label, extra in [("a name", {"family": "calc"}), ("a number", {"family": 5}), ("none", {})]:
            answer = client.request(dict(execute("result = FAKE_API"), **extra))
            if answer.get("result") != "fake application API":
                problems.append("execute with %s: %r" % (label, answer))
            answer = client.request(
                dict({"id": "s", "type": "state", "capture_path": None}, **extra)
            )
            if answer.get("state") != {"documents": 1}:
                problems.append("state with %s: %r" % (label, answer))
        client.close()
    return problems


class FamilyTests(CheckCase):
    def test_the_family_of_a_request_reaches_the_adapter_and_only_it(self):
        self.must_pass(check_the_family_reaches_the_adapter(CORE))

    def test_counter_proof_a_core_that_keeps_the_family_to_itself_is_detected(self):
        defective = defective_core(
            "namespace.update(adapter.namespace(*extra))", "namespace.update(adapter.namespace())"
        )
        self.must_fail(check_the_family_reaches_the_adapter(defective))

    def test_counter_proof_a_core_that_forgets_the_family_of_the_state_is_detected(self):
        defective = defective_core(
            "adapter.run_on_main(lambda: adapter.state(*extra))",
            "adapter.run_on_main(lambda: adapter.state('writer'))",
        )
        self.must_fail(check_the_family_reaches_the_adapter(defective))

    def test_an_unknown_family_is_refused_before_any_line_of_the_adapter_runs(self):
        self.must_pass(check_an_unknown_family_is_refused_before_the_adapter_runs(CORE))

    def test_counter_proof_a_core_that_accepts_any_name_is_detected(self):
        defective = defective_core(
            "if isinstance(family, str) and family in self._families:",
            "if isinstance(family, str):",
        )
        self.must_fail(check_an_unknown_family_is_refused_before_the_adapter_runs(defective))

    def test_counter_proof_a_core_that_refuses_nothing_is_detected(self):
        defective = defective_core(
            "        if self._families is None:\n            return None\n        family = request.get",
            "        if True:\n            return None\n        family = request.get",
        )
        self.must_fail(check_an_unknown_family_is_refused_before_the_adapter_runs(defective))

    def test_counter_proof_a_core_that_checks_after_the_adapter_ran_is_detected(self):
        # Le refus arrive trop tard : l'adaptateur a déjà construit son espace de noms.
        defective = defective_core(
            "        unknown = self._unknown_family(request)\n"
            "        if unknown is not None:\n"
            '            unknown.update({"stdout": "", "stderr": "", "result": None})\n'
            "            return unknown\n"
            "        extra = self._family_arguments(request)\n"
            "        started = time.monotonic()\n",
            "        extra = self._family_arguments(request)\n"
            "        started = time.monotonic()\n"
            "        self._adapter.run_on_main(lambda: None)\n"
            "        unknown = self._unknown_family(request)\n"
            "        if unknown is not None:\n"
            '            unknown.update({"stdout": "", "stderr": "", "result": None})\n'
            "            return unknown\n",
        )
        self.must_fail(check_an_unknown_family_is_refused_before_the_adapter_runs(defective))

    def test_an_adapter_without_families_never_sees_the_field(self):
        self.must_pass(check_an_adapter_without_families_never_sees_the_field(CORE))

    def test_counter_proof_a_core_that_hands_the_field_to_every_adapter_is_detected(self):
        defective = defective_core(
            "        if self._families is None:\n            return ()\n        return (request[\"family\"],)",
            "        return (request.get(\"family\"),)",
        )
        self.must_fail(check_an_adapter_without_families_never_sees_the_field(defective))

    def test_the_families_an_adapter_declares_must_be_a_tuple_of_names(self):
        for bad in ((), [], "writer", ("writer", 3), ("",), ("calc", None)):
            adapter = FamilyAdapter()
            adapter.families = bad
            directory = PrivateDir()
            try:
                with quiet_error_streams():
                    with self.assertRaises(CORE.BridgeError, msg=repr(bad)):
                        CORE.start(adapter, 0, str(directory.token()))
            finally:
                CORE.stop()
                directory.cleanup()


# --------------------------------------------------------------------------
# importer le cœur
# --------------------------------------------------------------------------

PROBE = r"""
import json, os, sys, threading
def sockets():
    found = 0
    for name in os.listdir("/proc/self/fd"):
        try:
            if os.readlink("/proc/self/fd/" + name).startswith("socket:"):
                found += 1
        except OSError:
            pass
    return found
before = (threading.active_count(), sockets())
path = sys.argv[1]
namespace = {"__name__": "lmm_core_probe"}
exec(compile(open(path, encoding="utf-8").read(), path, "exec"), namespace)
after = (threading.active_count(), sockets())
print(json.dumps({"before": before, "after": after}))
"""


def check_import_has_no_effect(path):
    completed = subprocess.run(
        [sys.executable, "-c", PROBE, str(path)],
        capture_output=True,
        text=True,
        timeout=60,
        env={"PATH": os.environ.get("PATH", ""), "PYTHONDONTWRITEBYTECODE": "1"},
    )
    if completed.returncode != 0:
        return ["the probe failed: %s" % completed.stderr[-400:]]
    report = json.loads(completed.stdout)
    if report["before"] != report["after"]:
        return ["importing changed (threads, sockets) from %s to %s" % (report["before"], report["after"])]
    return []


@unittest.skipIf(os.name == "nt", "the probe lists /proc/self/fd")
class ImportTests(CheckCase):
    def test_importing_the_core_starts_no_thread_and_opens_no_socket(self):
        from bridge_support import CORE_PATH

        self.must_pass(check_import_has_no_effect(CORE_PATH))

    def test_counter_proof_a_core_that_starts_on_import_is_detected(self):
        directory = PrivateDir()
        try:
            path = directory.path / "core_with_side_effect.py"
            path.write_text(
                core_text()
                + "\n_EARLY = socket.socket(socket.AF_INET, socket.SOCK_STREAM)\n"
                + "threading.Thread(target=time.sleep, args=(2,), daemon=True).start()\n",
                encoding="utf-8",
            )
            self.must_fail(check_import_has_no_effect(path))
        finally:
            directory.cleanup()


# --------------------------------------------------------------------------
# le jeton ne quitte jamais le pont
# --------------------------------------------------------------------------


def check_token_never_sent(core):
    """Chaque réponse que le pont compose lui-même, et chaque ligne qu'il écrit, doit être exempte du jeton.
    Les scripts sont du code d'utilisateur et ne sont pas exercés."""
    seen = []
    adapter = FakeAdapter(state_error=RuntimeError("no image"))
    with running(core, adapter=adapter) as bridge:
        stranger = Client(bridge.port)
        stranger.send_json({"type": "hello", "token": WRONG_TOKEN, "protocol": 1})
        seen.append(stranger.outcome()[1])
        stranger.close()
        client = Client(bridge.port)
        client.hello()
        for request in (
            {"id": "1", "type": "ping"},
            {"id": "2", "type": "unknown"},
            {"id": "3", "type": "execute", "script": None, "work_dir": "/home/alice"},
            execute("print('out')\nresult = 1"),
            execute("raise RuntimeError('failure')"),
            execute("result = object()"),
            execute("x = ("),
            {"id": "4", "type": "state", "capture_path": None},
        ):
            client.request(request)
        seen.append(client.received)
        client.close()
        said = bridge.said()
    problems = []
    for data in seen:
        if FAKE_TOKEN.encode("ascii") in data:
            problems.append("an answer holds the token")
    if FAKE_TOKEN in said:
        problems.append("the error stream shows the token")
    if not said.startswith("LMM bridge listening on 127.0.0.1:"):
        problems.append("the listening line is missing")
    return problems


class SecrecyTests(CheckCase):
    def test_the_token_never_appears_in_what_the_bridge_says_on_its_own(self):
        self.must_pass(check_token_never_sent(CORE))

    def test_counter_proof_a_core_that_echoes_the_token_is_detected(self):
        defective = defective_core(
            '"bridge_version": BRIDGE_VERSION,',
            '"bridge_version": BRIDGE_VERSION, "echo": self._token.decode("ascii"),',
        )
        self.must_fail(check_token_never_sent(defective))


# --------------------------------------------------------------------------
# lancement depuis l'environnement
# --------------------------------------------------------------------------


class LaunchTests(unittest.TestCase):
    def test_the_port_comes_from_the_environment_or_the_default(self):
        self.assertEqual(CORE._port_from_environment(None, 9301), 9301)
        self.assertEqual(CORE._port_from_environment("", 9301), 9301)
        self.assertEqual(CORE._port_from_environment("0", 9301), 0)
        self.assertEqual(CORE._port_from_environment("9401", 9301), 9401)
        for bad in ("-1", "70000", "9301 ", "nine", "１２"):
            with self.assertRaises(CORE.BridgeError):
                CORE._port_from_environment(bad, 9301)

    @unittest.skipIf(os.name == "nt", "Windows uses LOCALAPPDATA")
    def test_the_default_token_path_follows_xdg_with_the_usual_fallback(self):
        path = CORE._default_token_path
        self.assertEqual(
            path({"XDG_CONFIG_HOME": "/home/alice/.cfg", "HOME": "/home/alice"}),
            "/home/alice/.cfg/libremodular-mcp/token",
        )
        self.assertEqual(
            path({"XDG_CONFIG_HOME": "relative/cfg", "HOME": "/home/alice"}),
            "/home/alice/.config/libremodular-mcp/token",
        )
        self.assertEqual(
            path({"HOME": "/home/alice"}), "/home/alice/.config/libremodular-mcp/token"
        )
        with self.assertRaises(CORE.BridgeError):
            path({"HOME": "relative"})

    def test_a_second_start_in_the_same_process_is_refused(self):
        with running(CORE):
            directory = PrivateDir()
            try:
                with self.assertRaises(CORE.BridgeError):
                    CORE.start(FakeAdapter(), 0, str(directory.token()))
            finally:
                directory.cleanup()

    def test_stop_closes_the_listening_socket(self):
        with running(CORE) as bridge:
            port = bridge.port
        with self.assertRaises(OSError):
            socket.create_connection((LOOPBACK, port), timeout=2.0)


if __name__ == "__main__":
    unittest.main()
