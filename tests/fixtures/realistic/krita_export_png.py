import os
from krita import Krita, InfoObject

doc = Krita.instance().activeDocument()
options = InfoObject()
options.setProperty("alpha", True)
options.setProperty("compression", 6)
target = os.path.join(LMM_WORK_DIR, "export", "poster.png")
os.makedirs(os.path.dirname(target), exist_ok=True)
doc.exportImage(target, options)
result = {"path": target, "size": os.path.getsize(target)}
