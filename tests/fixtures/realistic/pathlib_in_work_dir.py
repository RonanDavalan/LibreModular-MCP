from pathlib import Path
out = Path(LMM_WORK_DIR) / "data" / "values.json"
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text("[1, 2, 3]")
result = out.read_text()
