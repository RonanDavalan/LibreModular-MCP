"""Adaptateur GIMP 3 : les noms qu'un script reçoit, et comment l'état et les captures de GIMP sont lus.

Pourquoi ce module existe
GIMP s'atteint par GObject Introspection et n'accepte les appels que depuis son fil principal. Ce
fichier est le seul endroit qui connaît l'un ou l'autre, pour que le cœur reste le même pour toutes
les applications. Écrit d'après l'API publique de GIMP 3.0 ; essayé dans GIMP 3.0.4 sans son
interface, pas dans la console interactive.
"""

import threading

DEFAULT_PORT = 9301


def _gimp():
    """Le module Gimp, ou None quand ce processus ne peut pas le charger."""
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


def _inside_gimp():
    """Dit si ce processus est un processus que GIMP a démarré : ailleurs, même avec `gi` installé,
    `Gimp.version()` vaut None et aucune boucle principale n'exécutera jamais un appel mis en file."""
    Gimp = _gimp()
    if Gimp is None:
        return False
    try:
        return bool(Gimp.version())
    except Exception:
        return False


def _open_images(Gimp):
    return list(Gimp.get_images())


def images():
    """Les images ouvertes dans GIMP ; la première est celle que le pont décrit comme active."""
    Gimp = _gimp()
    return _open_images(Gimp) if Gimp is not None and _inside_gimp() else []


def _nick(value):
    """Le nom court d'une valeur d'énumération GObject, comme « rgb » ou « normal »."""
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
    # L'introspection rend (success, non_empty, x1, y1, x2, y2) ; les cinq dernières valeurs sont lues
    # pour qu'une liaison sans l'indicateur de tête se lise de la même façon.
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
    """Ce dont le cœur a besoin de GIMP. Chaque appel dans GIMP se fait dans une méthode, pour que ce
    fichier se charge sans GIMP."""

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
        """Exécute `function` dans le fil principal de GIMP et rend sa valeur ou son exception. Sans GLib,
        hors de GIMP, ou depuis le fil principal lui-même, c'est un appel direct : un appel mis en file là
        où aucune boucle ne tourne attendrait indéfiniment."""
        GLib = _glib()
        if (
            GLib is None
            or threading.current_thread() is threading.main_thread()
            or not _inside_gimp()
        ):
            return function()
        done = threading.Event()
        outcome = {}

        def call():
            try:
                outcome["value"] = function()
            except BaseException as error:  # relevée dans le fil qui attend
                outcome["error"] = error
            finally:
                done.set()
            return False  # exécuté une seule fois

        GLib.idle_add(call)
        done.wait()
        if "error" in outcome:
            raise outcome["error"]
        return outcome.get("value")

    def state(self):
        Gimp = _gimp()
        if Gimp is None or not _inside_gimp():
            return {}
        open_images = _open_images(Gimp)
        return {
            "images": [_describe_image(image) for image in open_images],
            "active_image": _describe_active(Gimp, open_images[0]) if open_images else None,
        }

    def screenshot(self, path):
        """Exporte une copie aplatie de l'image active en PNG. La copie laisse intacte l'image de
        l'utilisateur : l'exporter changerait le nom de son fichier d'export."""
        Gimp, Gio = _gimp(), _gio()
        if Gimp is None or Gio is None or not _inside_gimp():
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
