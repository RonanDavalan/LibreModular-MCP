doc = XSCRIPTCONTEXT.getDocument()
slide = doc.getDrawPages().getByIndex(0)
title = doc.createInstance("com.sun.star.drawing.TitleTextShape")
slide.add(title)
title.setString("Roadmap")
size = title.getSize()
size.Width = 20000
size.Height = 2500
title.setSize(size)
subtitle = doc.createInstance("com.sun.star.drawing.SubtitleShape")
slide.add(subtitle)
subtitle.setString("Next three releases")
result = slide.getCount()
