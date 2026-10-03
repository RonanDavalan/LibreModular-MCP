"""Krita adapter: the names a script receives, and how Krita's state and captures are read.

Why this module exists
Krita's Python API must be used from the thread of its interface, which another
thread reaches through a queued Qt signal. This file is the only place that knows
it, so the core stays the same for every application. Written from the public
Krita 5 Python API; not tried in a real Krita.
"""

import threading

DEFAULT_PORT = 9302


def _krita_class():
    """The Krita class of Krita's Python API, or None outside Krita."""
    try:
        from krita import Krita
    except Exception:
        return None
    return Krita


def _instance():
    Krita = _krita_class()
    if Krita is None:
        return None
    try:
        return Krita.instance()
    except Exception:
        return None


def _qt_core():
    """QtCore from the PyQt that Krita embeds, or None."""
    try:
        from PyQt5 import QtCore
    except Exception:
        try:
            from PyQt6 import QtCore
        except Exception:
            return None
    return QtCore


def _inside_krita(QtCore):
    """Whether this process is Krita's: the Krita instance exists and a Qt
    application runs its event loop. Elsewhere, even with PyQt installed, a
    queued call would wait forever."""
    if QtCore is None or _instance() is None:
        return False
    try:
        return QtCore.QCoreApplication.instance() is not None
    except Exception:
        return False


def _dispatcher(QtCore):
    """An object living in the thread of Krita's interface: a signal emitted to
    it from another thread is queued there, and its slot runs the task."""

    class Dispatcher(QtCore.QObject):
        submitted = QtCore.pyqtSignal(object)

        def __init__(self):
            super().__init__()
            self.submitted.connect(self.run_task)

        @QtCore.pyqtSlot(object)
        def run_task(self, task):
            task()

    dispatcher = Dispatcher()
    application = QtCore.QCoreApplication.instance()
    if application is not None:
        dispatcher.moveToThread(application.thread())
    return dispatcher


def _node_id(node):
    unique = node.uniqueId()
    text = unique.toString() if hasattr(unique, "toString") else str(unique)
    return str(text)


def _describe_node(node):
    return {
        "id": _node_id(node),
        "name": node.name(),
        "visible": bool(node.visible()),
        "opacity": node.opacity(),
        "type": node.type(),
    }


class KritaAdapter:
    """What the core needs from Krita. Every call into Krita happens in a method,
    so this file loads without Krita."""

    app = "krita"

    def __init__(self):
        QtCore = _qt_core()
        self._dispatcher = _dispatcher(QtCore) if _inside_krita(QtCore) else None

    def app_version(self):
        application = _instance()
        if application is None:
            return "unknown"
        try:
            version = application.version()
        except Exception:
            return "unknown"
        return version if isinstance(version, str) and version else "unknown"

    def namespace(self):
        Krita = _krita_class()
        return {"Krita": Krita} if Krita is not None else {}

    def run_on_main(self, function):
        """Run `function` in the thread of Krita's interface and hand back its
        value or its exception. Outside Krita, or from that thread itself, it is a
        direct call."""
        if self._dispatcher is None or threading.current_thread() is threading.main_thread():
            return function()
        done = threading.Event()
        outcome = {}

        def task():
            try:
                outcome["value"] = function()
            except BaseException as error:  # raised again in the waiting thread
                outcome["error"] = error
            finally:
                done.set()

        self._dispatcher.submitted.emit(task)
        done.wait()
        if "error" in outcome:
            raise outcome["error"]
        return outcome.get("value")

    def state(self):
        application = _instance()
        if application is None:
            return {}
        document = application.activeDocument()
        if document is None:
            return {"document": None}
        active = document.activeNode()
        return {
            "document": {
                "name": document.name(),
                "width": document.width(),
                "height": document.height(),
                "color_model": document.colorModel(),
                "color_depth": document.colorDepth(),
                "color_profile": document.colorProfile(),
                "layers": [_describe_node(node) for node in document.topLevelNodes()],
                "active_layer": _node_id(active) if active is not None else None,
            }
        }

    def screenshot(self, path):
        """Save the projection of the active document, the merged image Krita
        shows, as PNG. The projection is a copy: the document is never changed."""
        application = _instance()
        if application is None:
            return False
        document = application.activeDocument()
        if document is None:
            return False
        document.waitForDone()
        image = document.projection(0, 0, document.width(), document.height())
        if image is None or image.isNull():
            return False
        return bool(image.save(path, "PNG"))


ADAPTER = KritaAdapter
