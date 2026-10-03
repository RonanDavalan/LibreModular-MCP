"""Realistic scripts a model would send to GIMP, Blender and LibreOffice.

Why this module exists
----------------------
The guard is only acceptable if it lets ordinary application scripts through;
these samples measure that.
"""

from __future__ import annotations

REALISTIC_SCRIPTS: dict[str, str] = {
    "gimp-new-image": '''
import gi
gi.require_version("Gimp", "3.0")
from gi.repository import Gimp
import math

image = Gimp.Image.new(800, 600, Gimp.ImageBaseType.RGB)
layer = Gimp.Layer.new(image, "Background", 800, 600, Gimp.ImageType.RGB_IMAGE, 100.0,
                       Gimp.LayerMode.NORMAL)
image.insert_layer(layer, None, 0)
for index in range(4):
    angle = math.radians(90 * index)
    layer.set_offsets(int(100 * math.cos(angle)), int(100 * math.sin(angle)))
result = {"layers": len(image.get_layers()), "width": image.get_width()}
''',
    "gimp-export": '''
import os
import gi
from gi.repository import Gimp, Gio

image = Gimp.get_images()[0]
target = Gio.File.new_for_path(os.path.join(LMM_WORK_DIR, "export", "out.png"))
Gimp.file_save(Gimp.RunMode.NONINTERACTIVE, image, target, None)
result = target.get_path()
''',
    "gimp-text-layers": '''
from gi.repository import Gimp
image = Gimp.get_images()[0]
font = Gimp.Font.get_by_name("Sans")
for i, label in enumerate(["Title", "Subtitle", "Footer"]):
    text = Gimp.TextLayer.new(image, label, font, 24.0, Gimp.Unit.pixel())
    image.insert_layer(text, None, i)
    text.set_offsets(10, 40 * i)
''',
    "blender-cube-material": '''
import bpy
import math

bpy.ops.mesh.primitive_cube_add(size=2, location=(0, 0, 1))
cube = bpy.context.active_object
cube.name = "Crate"
cube.rotation_euler[2] = math.radians(45)
mat = bpy.data.materials.new("Wood")
mat.use_nodes = True
for socket in mat.node_tree.nodes["Principled BSDF"].inputs:
    if socket.name == "Roughness":
        socket.default_value = 0.4
cube.data.materials.append(mat)
result = [obj.name for obj in bpy.data.objects]
''',
    "blender-collections": '''
import bpy
col = bpy.data.collections.new("Props")
bpy.context.scene.collection.children.link(col)
for obj in list(bpy.context.scene.collection.objects):
    bpy.context.scene.collection.objects.unlink(obj)
    col.objects.link(obj)
for img in bpy.data.images:
    if img.users == 0:
        bpy.data.images.remove(img)
''',
    "blender-render-save": '''
import os
import bpy
scene = bpy.context.scene
scene.render.resolution_x = 1280
scene.render.resolution_y = 720
scene.render.filepath = os.path.join(LMM_WORK_DIR, "render.png")
bpy.ops.render.render(write_still=True)
bpy.ops.wm.save_as_mainfile(filepath=os.path.join(LMM_WORK_DIR, "scene.blend"))
''',
    "blender-bmesh": '''
import bpy, bmesh, math
from mathutils import Vector

mesh = bpy.data.meshes.new("Ring")
bm = bmesh.new()
for i in range(16):
    t = 2 * math.pi * i / 16
    bm.verts.new(Vector((math.cos(t), math.sin(t), 0.0)))
bm.to_mesh(mesh)
bm.free()
obj = bpy.data.objects.new("Ring", mesh)
bpy.context.collection.objects.link(obj)
''',
    "calc-fill-sheet": '''
import math
doc = XSCRIPTCONTEXT.getDocument()
sheet = doc.getSheets().getByIndex(0)
for row in range(1, 11):
    sheet.getCellByPosition(0, row).setValue(row)
    sheet.getCellByPosition(1, row).setValue(round(math.sqrt(row), 3))
sheet.getCellByPosition(2, 1).setFormula("=SUM(A2:A11)")
result = sheet.getCellByPosition(2, 1).getValue()
''',
    "writer-paragraphs": '''
doc = XSCRIPTCONTEXT.getDocument()
text = doc.getText()
cursor = text.createTextCursor()
for line in ("Report", "Summary of the quarter", "Next steps"):
    text.insertString(cursor, line, False)
    text.insertControlCharacter(cursor, 0, False)
''',
    "impress-slides": '''
doc = XSCRIPTCONTEXT.getDocument()
pages = doc.getDrawPages()
for index in range(3):
    pages.insertNewByIndex(index + 1)
    slide = pages.getByIndex(index + 1)
    shape = doc.createInstance("com.sun.star.drawing.TextShape")
    slide.add(shape)
    shape.setString(f"Slide {index + 1}")
''',
    "writer-export-notes": '''
import os
notes = os.path.join(LMM_WORK_DIR, "notes.txt")
with open(notes, "w", encoding="utf-8") as handle:
    handle.write("generated")
with open(notes) as handle:
    result = handle.read()
''',
    "pathlib-in-work-dir": '''
from pathlib import Path
out = Path(LMM_WORK_DIR) / "data" / "values.json"
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text("[1, 2, 3]")
result = out.read_text()
''',
}
