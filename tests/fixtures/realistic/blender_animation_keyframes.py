import math
import bpy

scene = bpy.context.scene
scene.frame_start, scene.frame_end = 1, 48
obj = bpy.context.active_object
for frame in range(1, 49, 6):
    obj.location.x = math.sin(frame / 8.0) * 3
    obj.keyframe_insert(data_path="location", frame=frame)
for fcurve in obj.animation_data.action.fcurves:
    for point in fcurve.keyframe_points:
        point.interpolation = "BEZIER"
result = len(obj.animation_data.action.fcurves)
