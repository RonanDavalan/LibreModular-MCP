import os
import re
import xml.etree.ElementTree as ET

source = os.path.join(LMM_WORK_DIR, "input.svg")
tree = ET.parse(source)
changed = 0
for element in tree.getroot().iter():
    style = element.get("style")
    if style and "fill:" in style:
        element.set("style", re.sub(r"fill:#[0-9a-fA-F]{6}", "fill:#cc3333", style))
        changed += 1
tree.write(os.path.join(LMM_WORK_DIR, "recolored.svg"))
result = changed
