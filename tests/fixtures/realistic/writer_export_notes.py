import os
notes = os.path.join(LMM_WORK_DIR, "notes.txt")
with open(notes, "w", encoding="utf-8") as handle:
    handle.write("generated")
with open(notes) as handle:
    result = handle.read()
