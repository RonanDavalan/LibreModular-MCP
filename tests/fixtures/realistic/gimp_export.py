import os
import gi
from gi.repository import Gimp, Gio

image = Gimp.get_images()[0]
target = Gio.File.new_for_path(os.path.join(LMM_WORK_DIR, "export", "out.png"))
Gimp.file_save(Gimp.RunMode.NONINTERACTIVE, image, target, None)
result = target.get_path()
