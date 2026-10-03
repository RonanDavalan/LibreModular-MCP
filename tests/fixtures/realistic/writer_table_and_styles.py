doc = XSCRIPTCONTEXT.getDocument()
text = doc.getText()
cursor = text.createTextCursor()
cursor.setPropertyValue("ParaStyleName", "Heading 1")
text.insertString(cursor, "Quarterly figures", False)
text.insertControlCharacter(cursor, 0, False)
table = doc.createInstance("com.sun.star.text.TextTable")
table.initialize(3, 2)
text.insertTextContent(cursor, table, False)
rows = [("Revenue", 120), ("Costs", 80), ("Margin", 40)]
for row, (label, value) in enumerate(rows):
    table.getCellByName("A%d" % (row + 1)).setString(label)
    table.getCellByName("B%d" % (row + 1)).setValue(value)
result = table.getRows().getCount()
