"""Un faux LibreOffice et un faux paquet `uno` pour les tests de l'adaptateur UNO.

Pourquoi ce module existe
LibreOffice peut être installé ou non là où les tests s'exécutent, et `import uno` peut y marcher ou
non. Les tests doivent donner le même verdict dans les deux cas : ils installent donc ce faux dans
`sys.modules` le temps d'un contrôle, ou font échouer `import uno` exprès, et ne regardent jamais la machine.
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
    """Ce qu'UNO lève tant que rien n'écoute sur le port."""


class Forbidden(AssertionError):
    """Un appel que le pont ne doit jamais faire."""


class PropertyValue:
    def __init__(self):
        self.Name = ""
        self.Value = None


class Element:
    """Une chose qu'un texte Writer liste : un paragraphe ou un tableau."""

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
    """Un document d'une famille. `forbidden_calls` liste les appels qui ne doivent jamais être faits ;
    chacun d'eux lève aussi, pour qu'un pont qui en fait un échoue bruyamment."""

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
        """Ce qu'un script appelle pour créer un document : un nouveau de la famille que la fabrique nomme,
        qui devient le document courant, comme dans LibreOffice."""
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
    """Un composant qui est un document d'aucune des trois familles."""

    Title = "Drawing"

    def __init__(self):
        self.exports = []
        self.forbidden_calls = []

    def supportsService(self, name):
        return False


class Office:
    """Le processus LibreOffice : il écoute ou non, et a un bureau et une version."""

    def __init__(self, version="25.2.3", listening=True, desktop=None):
        self.version = version
        self.listening = listening
        self.desktop = desktop or Desktop()
        self.resolutions = []
        self.configuration_requests = []
        self.attempts = 0
        # Avec un nombre, LibreOffice commence à écouter à cette tentative.
        self.listens_from_attempt = None
        # Un pont qui n'abandonne jamais ferait tourner un fil pour le reste de l'exécution : au-delà de ce
        # nombre de tentatives le faux termine le fil (SystemExit n'est pas une Exception, aucune boucle de
        # nouvel essai ne l'avale).
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
    """Installe le faux paquet `uno` pour `office`, ou fait échouer `import uno` quand `absent`, quelle
    que soit la machine. Les vrais modules reviennent ensuite."""
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
