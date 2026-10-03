doc = XSCRIPTCONTEXT.getDocument()
pages = doc.getDrawPages()
for index in range(3):
    pages.insertNewByIndex(index + 1)
    slide = pages.getByIndex(index + 1)
    shape = doc.createInstance("com.sun.star.drawing.TextShape")
    slide.add(shape)
    shape.setString(f"Slide {index + 1}")
