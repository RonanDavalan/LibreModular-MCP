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
