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
