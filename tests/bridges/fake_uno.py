"""A fake LibreOffice and a fake `uno` package for the tests of the UNO adapter.

Why this module exists
LibreOffice may or may not be installed where the tests run, and `import uno` may
or may not work there. The tests must give the same verdict either way, so they
install this fake in `sys.modules` for the length of a check, or make `import uno`
fail on purpose, and never look at the machine.
"""

import contextlib
import sys
import types

from bridge_support import TINY_PNG

SERVICES = {
    "writer": "com.sun.star.text.TextDocument",
    "calc": "com.sun.star.sheet.SpreadsheetDocument",
    "impress": "com.sun.star.presentation.PresentationDocument",
}
FORBIDDEN = ("store", "storeSelf", "storeAsURL", "close", "dispose", "terminate")
PARAGRAPH = "com.sun.star.text.Paragraph"
TABLE = "com.sun.star.text.TextTable"


class NoConnectException(Exception):
    """What UNO raises while nothing listens on the port."""


class Forbidden(AssertionError):
    """A call the bridge must never make."""


class PropertyValue:
    def __init__(self):
        self.Name = ""
        self.Value = None


class Element:
    """One thing a Writer text lists: a paragraph or a table."""

    def __init__(self, service):
        self.service = service

    def supportsService(self, name):
        return name == self.service


class Enumeration:
    def __init__(self, items):
        self._items = list(items)

    def hasMoreElements(self):
        return bool(self._items)

    def nextElement(self):
        return self._items.pop(0)


class Document:
    """A document of one family. `forbidden_calls` lists the calls that must never
    be made; each of them also raises, so that a bridge making one fails loudly."""

    def __init__(self, family, title="Untitled 1", **details):
        self.family = family
        self.Title = title
        self.exports = []
        self.forbidden_calls = []
        self.export_error = None
        self.reads = 0
        if family == "writer":
            elements = details.get("elements", ["paragraph"])
            self.Text = types.SimpleNamespace(
                createEnumeration=lambda: Enumeration(
                    Element(PARAGRAPH if kind == "paragraph" else TABLE) for kind in elements
                )
            )
            pages = details.get("pages", 1)
            self.CurrentController = None if pages is None else types.SimpleNamespace(PageCount=pages)
        elif family == "calc":
            self._sheets = details.get("sheets", [("Sheet1", 0, 0)])
            self.Sheets = types.SimpleNamespace(
                ElementNames=tuple(name for name, _, _ in self._sheets),
                getByName=self._sheet,
            )
        elif family == "impress":
            self.DrawPages = types.SimpleNamespace(Count=details.get("slides", 1))

    def _sheet(self, name):
        for sheet_name, column, row in self._sheets:
            if sheet_name == name:
                cursor = types.SimpleNamespace(
                    RangeAddress=types.SimpleNamespace(EndColumn=0, EndRow=0),
                    gotoEndOfUsedArea=lambda expand, c=column, r=row: self._used(cursor, c, r),
                )
                return types.SimpleNamespace(createCursor=lambda: cursor, Name=name)
        raise KeyError(name)

    @staticmethod
    def _used(cursor, column, row):
        cursor.RangeAddress.EndColumn, cursor.RangeAddress.EndRow = column, row

    def supportsService(self, name):
        return name == SERVICES[self.family]

    def storeToURL(self, url, arguments):
        self.exports.append((url, [(entry.Name, entry.Value) for entry in arguments]))
        if self.export_error is not None:
            raise self.export_error
        assert url.startswith("file://"), url
        with open(url[len("file://") :], "wb") as handle:
            handle.write(TINY_PNG)

    def _refuse(name):
        def call(self, *arguments):
            self.forbidden_calls.append(name)
            raise Forbidden("%s must never be called" % name)

        return call

    store = _refuse("store")
    storeSelf = _refuse("storeSelf")
    storeAsURL = _refuse("storeAsURL")
    close = _refuse("close")
    dispose = _refuse("dispose")


FACTORIES = {
    "private:factory/swriter": "writer",
    "private:factory/scalc": "calc",
    "private:factory/simpress": "impress",
}


class Desktop:
    def __init__(self, documents=(), current=None):
        self.documents = list(documents)
        self.current = current
        self.forbidden_calls = []
        self.loaded = []

    def getCurrentComponent(self):
        return self.current

    def loadComponentFromURL(self, url, target, flags, arguments):
        """What a script calls to create a document: a new one of the family the
        factory names, which becomes the current document, as in LibreOffice."""
        self.loaded.append((url, target, flags, tuple(arguments)))
        family = FACTORIES[url]
        document = Document(family, "Untitled %d" % (len(self.documents) + 1))
        self.documents.append(document)
        self.current = document
        return document

    def getComponents(self):
        return types.SimpleNamespace(createEnumeration=lambda: Enumeration(self.documents))

    def terminate(self):
        self.forbidden_calls.append("terminate")
        raise Forbidden("terminate must never be called")


class Other:
    """A component that is a document of none of the three families."""

    Title = "Drawing"

    def __init__(self):
        self.exports = []
        self.forbidden_calls = []

    def supportsService(self, name):
        return False


class Office:
    """The LibreOffice process: it listens or not, and has a desktop and a version."""

    def __init__(self, version="25.2.3", listening=True, desktop=None):
        self.version = version
        self.listening = listening
        self.desktop = desktop or Desktop()
        self.resolutions = []
        self.configuration_requests = []
        self.attempts = 0
        # With a number, LibreOffice starts listening at that attempt.
        self.listens_from_attempt = None
        # A bridge that never gives up would keep a thread spinning for the rest of
        # the run: past this many attempts the fake ends the thread (SystemExit is
        # not an Exception, so no retry loop swallows it).
        self.abort_after_attempts = 2000


class _Manager:
    def __init__(self, office, local):
        self.office, self.local = office, local

    def createInstanceWithContext(self, name, context):
        office = self.office
        if self.local:
            assert name == "com.sun.star.bridge.UnoUrlResolver", name
            return types.SimpleNamespace(resolve=lambda url: self._resolve(url))
        if name == "com.sun.star.frame.Desktop":
            return office.desktop
        if name == "com.sun.star.configuration.ConfigurationProvider":
            return types.SimpleNamespace(createInstanceWithArguments=self._configuration)
        raise KeyError(name)

    def _resolve(self, url):
        office = self.office
        office.attempts += 1
        if office.attempts > office.abort_after_attempts:
            raise SystemExit("the fake LibreOffice was asked too many times")
        office.resolutions.append(url)
        if office.listens_from_attempt is not None and office.attempts >= office.listens_from_attempt:
            office.listening = True
        if not office.listening:
            raise NoConnectException("Connector : couldn't connect to socket (Connection refused)")
        return types.SimpleNamespace(ServiceManager=_Manager(office, False))

    def _configuration(self, service, arguments):
        self.office.configuration_requests.append(
            (service, [(entry.Name, entry.Value) for entry in arguments])
        )
        return types.SimpleNamespace(
            getByName=lambda key: self.office.version if key == "ooSetupVersionAboutBox" else None
        )


def _fake_modules(office):
    uno = types.ModuleType("uno")
    uno.getComponentContext = lambda: types.SimpleNamespace(ServiceManager=_Manager(office, True))
    uno.systemPathToFileUrl = lambda path: "file://" + path
    beans = types.ModuleType("com.sun.star.beans")
    beans.PropertyValue = PropertyValue
    parents = {}
    for name in ("com", "com.sun", "com.sun.star"):
        parents[name] = types.ModuleType(name)
    return {"uno": uno, "com.sun.star.beans": beans, **parents}


NAMES = ("uno", "com", "com.sun", "com.sun.star", "com.sun.star.beans")


@contextlib.contextmanager
def installed(office=None, absent=False):
    """Install the fake `uno` package for `office`, or make `import uno` fail when
    `absent`, whatever the machine has. The real modules come back afterwards."""
    saved = {name: sys.modules[name] for name in NAMES if name in sys.modules}
    for name in NAMES:
        sys.modules.pop(name, None)
    if absent:
        sys.modules["uno"] = None
        sys.modules["com.sun.star.beans"] = None
    else:
        sys.modules.update(_fake_modules(office))
    try:
        yield office
    finally:
        for name in NAMES:
            sys.modules.pop(name, None)
        sys.modules.update(saved)
