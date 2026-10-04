"""LibreOffice adapter: Writer, Calc and Impress through UNO, from a stand-alone process.

Why this module exists
LibreOffice listens for UNO on a local port and runs no script of ours inside it, so
this bridge is a process of its own that connects to that port for each request.
This file is the only place that knows UNO, so the core stays the same for every
application. Written from the public UNO API and from measurements on LibreOffice
in a headless run; not tried against the operator's own installation.
"""

import os
import time

DEFAULT_PORT = 9304

# The only address this adapter connects to, written as a number on purpose: a
# host name can resolve to an IPv6 address LibreOffice does not listen on.
UNO_HOST = "127.0.0.1"
DEFAULT_UNO_PORT = 2002
UNO_PORT_VARIABLE = "LIBREMODULAR_UNO_PORT"
# How long a request waits for LibreOffice to accept the connection, and how long
# it pauses between two attempts. Past the total, the answer is "unavailable".
CONNECT_TOTAL_S = 5.0
CONNECT_STEP_S = 0.25

UNKNOWN = "unknown"
VERSION_NODE = "/org.openoffice.Setup/Product"
VERSION_KEY = "ooSetupVersionAboutBox"
PARAGRAPH_SERVICE = "com.sun.star.text.Paragraph"

# The service that identifies each family of document: a document has only one.
FAMILY_SERVICES = {
    "writer": "com.sun.star.text.TextDocument",
    "calc": "com.sun.star.sheet.SpreadsheetDocument",
    "impress": "com.sun.star.presentation.PresentationDocument",
}
# The export filter that makes a PNG of each family.
PNG_FILTERS = {
    "writer": "writer_png_Export",
    "calc": "calc_png_Export",
    "impress": "impress_png_Export",
}


class LibreOfficeUnavailable(Exception):
    """The `uno` module cannot be imported, or LibreOffice does not accept the connection."""


class WrongDocumentType(Exception):
    """The current document is not of the family the request names."""


def _uno():
    """The `uno` module, or None when this interpreter cannot import it."""
    try:
        import uno
    except Exception:
        return None
    return uno


def _property_value():
    """A new PropertyValue, or None when `com.sun.star.beans` cannot be imported."""
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
    """One attempt: the remote context and the desktop of the LibreOffice that
    listens on `port`."""
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
    """The remote context and the desktop. LibreOffice does not listen until it
    has finished starting, so a refused connection is tried again, until `total_s`
    seconds have passed: never longer, and a single attempt when it is zero."""
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
    """The family of a document, or None for anything else (a drawing, a
    database, a component that is not a document)."""
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
    """Every open document of the three families, in the order LibreOffice lists them."""
    found = []
    enumeration = desktop.getComponents().createEnumeration()
    while enumeration.hasMoreElements():
        component = enumeration.nextElement()
        family = _family_of(component)
        if family is not None:
            found.append({"title": _title(component), "family": family})
    return found


def _current_document(desktop, family):
    """The current document, which must be of `family`; None when LibreOffice has
    no current document, so that a script can create the first one."""
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
    # A table is listed next to the paragraphs, and is not one of them.
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
    """What the core needs from LibreOffice. Every call into UNO happens in a
    method, so this file loads without it."""

    app = "libreoffice"
    families = tuple(FAMILY_SERVICES)

    def app_version(self):
        # One attempt, never a wait: the core asks again at each hello and each
        # state while the answer is "unknown", and the Hub stops waiting for a
        # hello after a few seconds, which is the length of the wait of _connect.
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
        """The names a script receives: the uno module, the remote context, the
        desktop and the current document of `family`, which is None when there is
        no current document. A current document of another family stops the request
        before any line of the script runs: documents are never converted."""
        context, desktop = _connect()
        document = _current_document(desktop, family)
        return {"uno": _uno(), "ctx": context, "desktop": desktop, "doc": document}

    def run_on_main(self, function):
        """This process is not LibreOffice: nothing to hand work over to."""
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
        """Export the active document of `family` as PNG with LibreOffice's own
        filter. The document is neither saved nor closed: an export leaves it
        unsaved."""
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
