"""Adaptateur LibreOffice : Writer, Calc et Impress par UNO, depuis un processus autonome.

Pourquoi ce module existe
LibreOffice écoute UNO sur un port local et n'exécute aucun script de nous en son sein : ce pont
est donc un processus à part qui se connecte à ce port pour chaque requête. Ce fichier est le seul
endroit qui connaît UNO, pour que le cœur reste le même pour toutes les applications. Écrit
d'après l'API publique d'UNO et des mesures sur LibreOffice en exécution sans interface ; pas
essayé contre l'installation de l'opérateur.
"""

import os
import time

DEFAULT_PORT = 9304

# La seule adresse à laquelle cet adaptateur se connecte, écrite comme un nombre exprès : un nom
# d'hôte peut se résoudre en une adresse IPv6 sur laquelle LibreOffice n'écoute pas.
UNO_HOST = "127.0.0.1"
DEFAULT_UNO_PORT = 2002
UNO_PORT_VARIABLE = "LIBREMODULAR_UNO_PORT"
# Combien de temps une requête attend que LibreOffice accepte la connexion, et combien de temps
# elle fait pause entre deux tentatives. Passé le total, la réponse est « unavailable ».
CONNECT_TOTAL_S = 5.0
CONNECT_STEP_S = 0.25

UNKNOWN = "unknown"
VERSION_NODE = "/org.openoffice.Setup/Product"
VERSION_KEY = "ooSetupVersionAboutBox"
PARAGRAPH_SERVICE = "com.sun.star.text.Paragraph"

# Le service qui identifie chaque famille de document : un document n'en a qu'un.
FAMILY_SERVICES = {
    "writer": "com.sun.star.text.TextDocument",
    "calc": "com.sun.star.sheet.SpreadsheetDocument",
    "impress": "com.sun.star.presentation.PresentationDocument",
}
# Le filtre d'export qui fait un PNG de chaque famille.
PNG_FILTERS = {
    "writer": "writer_png_Export",
    "calc": "calc_png_Export",
    "impress": "impress_png_Export",
}


class LibreOfficeUnavailable(Exception):
    """Le module `uno` ne peut pas être importé, ou LibreOffice n'accepte pas la connexion."""


class WrongDocumentType(Exception):
    """Le document courant n'est pas de la famille que la requête nomme."""


def _uno():
    """Le module `uno`, ou None quand cet interpréteur ne peut pas l'importer."""
    try:
        import uno
    except Exception:
        return None
    return uno


def _property_value():
    """Un nouveau PropertyValue, ou None quand `com.sun.star.beans` ne peut pas être importé."""
    try:
        from com.sun.star.beans import PropertyValue
    except Exception:
        return None
    return PropertyValue()


def _named_value(name, value):
    entry = _property_value()
    if entry is None:
        raise LibreOfficeUnavailable("com.sun.star.beans cannot be imported by this interpreter")
    entry.Name = name
    entry.Value = value
    return entry


def _uno_port():
    text = os.environ.get(UNO_PORT_VARIABLE)
    if text is None or text == "":
        return DEFAULT_UNO_PORT
    if len(text) > 5 or any(character not in "0123456789" for character in text):
        raise LibreOfficeUnavailable("%s must be a port number" % UNO_PORT_VARIABLE)
    port = int(text)
    if not 1 <= port <= 65535:
        raise LibreOfficeUnavailable("%s must be a port number" % UNO_PORT_VARIABLE)
    return port


def _resolve(uno, port):
    """Une tentative : le contexte distant et le bureau du LibreOffice qui écoute sur `port`."""
    local = uno.getComponentContext()
    resolver = local.ServiceManager.createInstanceWithContext(
        "com.sun.star.bridge.UnoUrlResolver", local
    )
    url = "uno:socket,host=%s,port=%d;urp;StarOffice.ComponentContext" % (UNO_HOST, port)
    context = resolver.resolve(url)
    desktop = context.ServiceManager.createInstanceWithContext(
        "com.sun.star.frame.Desktop", context
    )
    return context, desktop


def _connect(total_s=None):
    """Le contexte distant et le bureau. LibreOffice n'écoute pas avant d'avoir fini de démarrer : une
    connexion refusée est donc retentée, jusqu'à ce que `total_s` secondes se soient écoulées : jamais
    plus longtemps, et une seule tentative quand il vaut zéro."""
    uno = _uno()
    if uno is None:
        raise LibreOfficeUnavailable(
            "the Python module uno cannot be imported by this interpreter; "
            "run this bridge with the system Python of the LibreOffice installation"
        )
    port = _uno_port()
    budget = CONNECT_TOTAL_S if total_s is None else total_s
    deadline = time.monotonic() + budget
    while True:
        try:
            return _resolve(uno, port)
        except Exception as error:
            failure = error
        if time.monotonic() >= deadline:
            break
        time.sleep(CONNECT_STEP_S)
    raise LibreOfficeUnavailable(
        "LibreOffice does not accept a connection on %s:%d (%s: %s)"
        % (UNO_HOST, port, type(failure).__name__, str(failure)[:200])
    )


def _family_of(component):
    """La famille d'un document, ou None pour tout le reste (un dessin, une base de données, un composant
    qui n'est pas un document)."""
    for family, service in FAMILY_SERVICES.items():
        try:
            if component.supportsService(service):
                return family
        except Exception:
            continue
    return None


def _title(component):
    try:
        title = component.Title
    except Exception:
        return ""
    return title if isinstance(title, str) else ""


def _documents(desktop):
    """Chaque document ouvert des trois familles, dans l'ordre où LibreOffice les liste."""
    found = []
    enumeration = desktop.getComponents().createEnumeration()
    while enumeration.hasMoreElements():
        component = enumeration.nextElement()
        family = _family_of(component)
        if family is not None:
            found.append({"title": _title(component), "family": family})
    return found


def _current_document(desktop, family):
    """Le document courant, qui doit être de `family` ; None quand LibreOffice n'a aucun document
    courant, pour qu'un script puisse créer le premier."""
    component = desktop.getCurrentComponent()
    if component is None:
        return None
    actual = _family_of(component)
    if actual != family:
        raise WrongDocumentType(
            "the active document is %s, not a %s document"
            % ("a %s document" % actual if actual else "of another kind", family)
        )
    return component


def _count_paragraphs(document):
    # Un tableau est listé à côté des paragraphes, sans être l'un d'eux.
    count = 0
    enumeration = document.Text.createEnumeration()
    while enumeration.hasMoreElements():
        if enumeration.nextElement().supportsService(PARAGRAPH_SERVICE):
            count += 1
    return count


def _describe_writer(document):
    controller = document.CurrentController
    return {
        "paragraphs": _count_paragraphs(document),
        "pages": controller.PageCount if controller is not None else None,
    }


def _describe_calc(document):
    sheets = []
    for name in document.Sheets.ElementNames:
        cursor = document.Sheets.getByName(name).createCursor()
        cursor.gotoEndOfUsedArea(False)
        used = cursor.RangeAddress
        sheets.append({"name": name, "last_column": used.EndColumn, "last_row": used.EndRow})
    return {"sheets": sheets}


def _describe_impress(document):
    return {"slides": document.DrawPages.Count}


_DESCRIBE = {
    "writer": _describe_writer,
    "calc": _describe_calc,
    "impress": _describe_impress,
}


class UnoAdapter:
    """Ce dont le cœur a besoin de LibreOffice. Chaque appel dans UNO se fait dans une méthode, pour que
    ce fichier se charge sans lui."""

    app = "libreoffice"
    families = tuple(FAMILY_SERVICES)

    def app_version(self):
        # Une tentative, jamais une attente : le cœur redemande à chaque hello et à chaque état tant que la
        # réponse est « unknown », et le Hub cesse d'attendre un hello au bout de quelques secondes, ce qui
        # est la durée de l'attente de _connect.
        try:
            context, _ = _connect(0)
            provider = context.ServiceManager.createInstanceWithContext(
                "com.sun.star.configuration.ConfigurationProvider", context
            )
            node = provider.createInstanceWithArguments(
                "com.sun.star.configuration.ConfigurationAccess",
                (_named_value("nodepath", VERSION_NODE),),
            )
            version = node.getByName(VERSION_KEY)
        except Exception:
            return UNKNOWN
        return version if isinstance(version, str) and version else UNKNOWN

    def namespace(self, family):
        """Les noms qu'un script reçoit : le module uno, le contexte distant, le bureau et le document courant
        de `family`, qui vaut None quand il n'y a aucun document courant. Un document courant d'une autre
        famille arrête la requête avant qu'aucune ligne du script ne s'exécute : les documents ne sont
        jamais convertis."""
        context, desktop = _connect()
        document = _current_document(desktop, family)
        return {"uno": _uno(), "ctx": context, "desktop": desktop, "doc": document}

    def run_on_main(self, function):
        """Ce processus n'est pas LibreOffice : rien à qui confier le travail."""
        return function()

    def state(self, family):
        if _uno() is None:
            return {}
        _, desktop = _connect()
        document = desktop.getCurrentComponent()
        active = None
        if document is not None and _family_of(document) == family:
            active = _DESCRIBE[family](document)
        return {"documents": _documents(desktop), "active": active}

    def screenshot(self, path, family):
        """Exporte le document actif de `family` en PNG avec le filtre propre de LibreOffice. Le document
        n'est ni enregistré ni fermé : un export le laisse non enregistré."""
        uno = _uno()
        if uno is None or not os.path.isabs(path):
            return False
        try:
            _, desktop = _connect(0)
            document = desktop.getCurrentComponent()
            if document is None or _family_of(document) != family:
                return False
            document.storeToURL(
                uno.systemPathToFileUrl(path),
                (_named_value("FilterName", PNG_FILTERS[family]),),
            )
        except Exception:
            return False
        return True


ADAPTER = UnoAdapter
