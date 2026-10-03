doc = XSCRIPTCONTEXT.getDocument()
text = doc.getText()
cursor = text.createTextCursor()
for line in ("Report", "Summary of the quarter", "Next steps"):
    text.insertString(cursor, line, False)
    text.insertControlCharacter(cursor, 0, False)
