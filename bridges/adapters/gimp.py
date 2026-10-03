"""GIMP 3 adapter: the names a script receives, and how GIMP's state and captures are read.

Why this module exists
GIMP is reached through GObject Introspection and only accepts calls from its main
thread. This file is the only place that knows either, so the core stays the same
for every application. Written from the public GIMP 3.0 API; not tried in a real GIMP.
"""

import threading

DEFAULT_PORT = 9301


def _gimp():
    """The Gimp module, or None when this process cannot load it."""
    try:
        import gi

        gi.require_version("Gimp", "3.0")
        from gi.repository import Gimp
    except Exception:
        return None
    return Gimp


def _gio():
    try:
        from gi.repository import Gio
    except Exception:
        return None
    return Gio


def _glib():
    try:
        from gi.repository import GLib
    except Exception:
        return None
    return GLib


def _open_images(Gimp):
    return list(Gimp.get_images())


def images():
    """The images open in GIMP; the first one is the one the bridge describes
    as active."""
    Gimp = _gimp()
    return _open_images(Gimp) if Gimp is not None else []


def _nick(value):
    """The short name of a GObject enumeration value, such as "rgb" or "normal"."""
    nick = getattr(value, "value_nick", None)
    return nick if isinstance(nick, str) else str(value)


def _describe_image(image):
    return {
        "id": image.get_id(),
        "name": image.get_name(),
        "width": image.get_width(),
        "height": image.get_height(),
        "color_mode": _nick(image.get_base_type()),
    }


def _describe_layer(layer):
    return {
        "id": layer.get_id(),
        "name": layer.get_name(),
        "visible": bool(layer.get_visible()),
        "opacity": float(layer.get_opacity()),
        "blend_mode": _nick(layer.get_mode()),
    }


def _selection(Gimp, image):
    # Introspection returns (success, non_empty, x1, y1, x2, y2); the last five
    # values are read so that a binding without the leading flag reads the same.
    non_empty, x1, y1, x2, y2 = tuple(Gimp.Selection.bounds(image))[-5:]
    if not non_empty:
        return None
    return {"x": x1, "y": y1, "width": x2 - x1, "height": y2 - y1}


def _describe_active(Gimp, image):
    selected = list(image.get_selected_layers())
    return {
        "id": image.get_id(),
        "name": image.get_name(),
        "layers": [_describe_layer(layer) for layer in image.get_layers()],
        "active_layer": selected[0].get_id() if selected else None,
        "selection": _selection(Gimp, image),
    }


class GimpAdapter:
    """What the core needs from GIMP. Every call into GIMP happens in a method,
    so this file loads without GIMP."""

    app = "gimp"

    def app_version(self):
        Gimp = _gimp()
        if Gimp is None:
            return "unknown"
        try:
            version = Gimp.version()
        except Exception:
            return "unknown"
        return version if isinstance(version, str) and version else "unknown"

    def namespace(self):
        names = {}
        for name, module in (("Gimp", _gimp()), ("Gio", _gio()), ("GLib", _glib())):
            if module is not None:
                names[name] = module
        names["images"] = images
        return names

    def run_on_main(self, function):
        """Run `function` in GIMP's main thread and hand back its value or its
        exception. Without GLib, or from the main thread itself, it is a direct call."""
        GLib = _glib()
        if GLib is None or threading.current_thread() is threading.main_thread():
            return function()
        done = threading.Event()
        outcome = {}

        def call():
            try:
                outcome["value"] = function()
            except BaseException as error:  # raised again in the waiting thread
                outcome["error"] = error
            finally:
                done.set()
            return False  # run once

        GLib.idle_add(call)
        done.wait()
        if "error" in outcome:
            raise outcome["error"]
        return outcome.get("value")

    def state(self):
        Gimp = _gimp()
        if Gimp is None:
            return {}
        open_images = _open_images(Gimp)
        return {
            "images": [_describe_image(image) for image in open_images],
            "active_image": _describe_active(Gimp, open_images[0]) if open_images else None,
        }

    def screenshot(self, path):
        """Export a flattened copy of the active image as PNG. The copy keeps the
        user's image untouched: exporting it would change its export file name."""
        Gimp, Gio = _gimp(), _gio()
        if Gimp is None or Gio is None:
            return False
        open_images = _open_images(Gimp)
        if not open_images:
            return False
        copy = open_images[0].duplicate()
        try:
            copy.merge_visible_layers(Gimp.MergeType.CLIP_TO_IMAGE)
            saved = Gimp.file_save(
                Gimp.RunMode.NONINTERACTIVE, copy, Gio.File.new_for_path(path), None
            )
        finally:
            copy.delete()
        return bool(saved)


ADAPTER = GimpAdapter
