import os
import bpy
scene = bpy.context.scene
scene.render.resolution_x = 1280
scene.render.resolution_y = 720
scene.render.filepath = os.path.join(LMM_WORK_DIR, "render.png")
bpy.ops.render.render(write_still=True)
bpy.ops.wm.save_as_mainfile(filepath=os.path.join(LMM_WORK_DIR, "scene.blend"))
