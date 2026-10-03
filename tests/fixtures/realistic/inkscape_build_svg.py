import os
import xml.etree.ElementTree as ET

SVG = "http://www.w3.org/2000/svg"
ET.register_namespace("", SVG)
root = ET.Element("{%s}svg" % SVG, {"width": "400", "height": "300", "viewBox": "0 0 400 300"})
for index in range(5):
    ET.SubElement(root, "{%s}rect" % SVG, {
        "x": str(20 + index * 70), "y": "40", "width": "50",
        "height": str(40 + index * 30), "fill": "#3366cc",
    })
label = ET.SubElement(root, "{%s}text" % SVG, {"x": "20", "y": "280"})
label.text = "Bars"
out = os.path.join(LMM_WORK_DIR, "bars.svg")
ET.ElementTree(root).write(out, encoding="utf-8", xml_declaration=True)
result = out
