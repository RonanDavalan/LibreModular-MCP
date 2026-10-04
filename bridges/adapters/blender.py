"""Blender adapter: the names a script receives, and how Blender's state is read.

Why this module exists
Blender's `bpy` must be used from Blender's main thread, which only a timer can
reach from another thread. This file is the only place that knows it, so the core
stays the same for every application. Written from the public Blender Python API;
not tried in a real Blender.
"""

import queue
import threading

DEFAULT_PORT = 9303
# How often, in seconds, the timer in Blender's main thread looks for work.
_POLL_INTERVAL = 0.05


def _bpy():
    """The bpy module, or None when this process cannot load it."""
    try:
        import bpy
    except Exception:
        return None
    return bpy


def _timers_run(bpy):
    """Whether a timer registered here will ever run: only in Blender with its
    interface. In background mode, and when `bpy` is the stand-alone package, no
    event loop runs timers, and a queued call would wait forever."""
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
    """What the core needs from Blender. Every call into Blender happens in a
    method, so this file loads without Blender."""

    app = "blender"

    def __init__(self):
        self._pending = queue.Queue()
        self._polling = False
        bpy = _bpy()
        # Registered once, from the thread that builds the adapter: in Blender,
        # the main thread that runs the bridge file. Other threads only use the queue.
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
        # "4.2.3 LTS" reads as "4.2.3".
        words = text.split() if isinstance(text, str) else []
        return words[0] if words else "unknown"

    def namespace(self):
        bpy = _bpy()
        return {"bpy": bpy} if bpy is not None else {}

    def run_on_main(self, function):
        """Run `function` in Blender's main thread and hand back its value or its
        exception. Outside Blender's interface, or from the main thread itself, it
        is a direct call."""
        if not self._polling or threading.current_thread() is threading.main_thread():
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

        self._pending.put(task)
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
        """No capture: a render would have to be started implicitly, and a copy
        of the 3D view is not safely available without the interface."""
        return False


ADAPTER = BlenderAdapter
