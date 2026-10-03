import statistics

doc = XSCRIPTCONTEXT.getDocument()
sheet = doc.getSheets().getByName("Sheet1")
values = [12, 18, 9, 22, 15, 30]
for index, value in enumerate(values):
    sheet.getCellByPosition(0, index).setValue(value)
sheet.getCellByPosition(1, 0).setFormula("=AVERAGE(A1:A6)")
cells = sheet.getCellRangeByName("A1:A6")
cells.setPropertyValue("CharWeight", 150.0)
result = {
    "mean": statistics.mean(values),
    "formula_value": sheet.getCellByPosition(1, 0).getValue(),
}
