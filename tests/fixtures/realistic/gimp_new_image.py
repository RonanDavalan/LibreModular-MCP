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
