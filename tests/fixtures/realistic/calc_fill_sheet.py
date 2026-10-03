import math
doc = XSCRIPTCONTEXT.getDocument()
sheet = doc.getSheets().getByIndex(0)
for row in range(1, 11):
    sheet.getCellByPosition(0, row).setValue(row)
    sheet.getCellByPosition(1, row).setValue(round(math.sqrt(row), 3))
sheet.getCellByPosition(2, 1).setFormula("=SUM(A2:A11)")
result = sheet.getCellByPosition(2, 1).getValue()
