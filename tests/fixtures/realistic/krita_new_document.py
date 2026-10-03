from krita import Krita, InfoObject
import math

app = Krita.instance()
doc = app.createDocument(1024, 768, "Poster", "RGBA", "U8", "", 120.0)
app.activeWindow().addView(doc)
root = doc.rootNode()
background = doc.createNode("Background", "paintlayer")
root.addChildNode(background, None)
for index in range(3):
    layer = doc.createNode("Layer {}".format(index + 1), "paintlayer")
    layer.setOpacity(int(255 * (1 - index * 0.25)))
    root.addChildNode(layer, None)
doc.refreshProjection()
result = {"layers": [child.name() for child in root.childNodes()], "diag": math.hypot(1024, 768)}
