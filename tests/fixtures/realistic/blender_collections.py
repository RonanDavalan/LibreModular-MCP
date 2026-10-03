import bpy
col = bpy.data.collections.new("Props")
bpy.context.scene.collection.children.link(col)
for obj in list(bpy.context.scene.collection.objects):
    bpy.context.scene.collection.objects.unlink(obj)
    col.objects.link(obj)
for img in bpy.data.images:
    if img.users == 0:
        bpy.data.images.remove(img)
