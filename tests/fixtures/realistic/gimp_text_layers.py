from gi.repository import Gimp
image = Gimp.get_images()[0]
font = Gimp.Font.get_by_name("Sans")
for i, label in enumerate(["Title", "Subtitle", "Footer"]):
    text = Gimp.TextLayer.new(image, label, font, 24.0, Gimp.Unit.pixel())
    image.insert_layer(text, None, i)
    text.set_offsets(10, 40 * i)
