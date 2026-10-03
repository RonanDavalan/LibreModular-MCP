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
