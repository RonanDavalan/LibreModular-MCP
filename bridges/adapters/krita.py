"""Adaptateur Krita : les noms qu'un script reçoit, et comment l'état et les captures de Krita sont lus.

Pourquoi ce module existe
L'API Python de Krita doit être utilisée depuis le fil de son interface, qu'un autre fil atteint par
un signal Qt mis en file. Ce fichier est le seul endroit qui le sait, pour que le cœur reste le
même pour toutes les applications. Écrit d'après l'API Python publique de Krita 5 ; pas essayé
dans un vrai Krita.
"""

import threading

DEFAULT_PORT = 9302


def _krita_class():
    """La classe Krita de l'API Python de Krita, ou None hors de Krita."""
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
    """QtCore du PyQt que Krita embarque, ou None."""
    try:
        from PyQt5 import QtCore
    except Exception:
        try:
            from PyQt6 import QtCore
        except Exception:
            return None
    return QtCore


def _inside_krita(QtCore):
    """Dit si ce processus est celui de Krita : l'instance Krita existe et une application Qt exécute sa
    boucle d'événements. Ailleurs, même avec PyQt installé, un appel mis en file attendrait indéfiniment."""
    if QtCore is None or _instance() is None:
        return False
    try:
        return QtCore.QCoreApplication.instance() is not None
    except Exception:
        return False


def _dispatcher(QtCore):
    """Un objet qui vit dans le fil de l'interface de Krita : un signal qui lui est émis depuis un autre
    fil y est mis en file, et son créneau exécute la tâche."""

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
    """Ce dont le cœur a besoin de Krita. Chaque appel dans Krita se fait dans une méthode, pour que ce
    fichier se charge sans Krita."""

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
        """Exécute `function` dans le fil de l'interface de Krita et rend sa valeur ou son exception. Hors
        de Krita, ou depuis ce fil lui-même, c'est un appel direct."""
        if self._dispatcher is None or threading.current_thread() is threading.main_thread():
            return function()
        done = threading.Event()
        outcome = {}

        def task():
            try:
                outcome["value"] = function()
            except BaseException as error:  # relevée dans le fil qui attend
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
        """Enregistre la projection du document actif, l'image fusionnée que Krita montre, en PNG. La
        projection est une copie : le document n'est jamais changé."""
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
