import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent
EXPECTED = {item["id"] for item in json.loads((ROOT / "fixtures.json").read_text(encoding="utf-8"))["fixtures"]}

errors = []
for name in ("boa", "deno-core", "node-sidecar"):
    path = ROOT / name / "results.json"
    if not path.is_file():
        errors.append(f"{name}: missing results.json")
        continue
    data = json.loads(path.read_text(encoding="utf-8"))
    rows = data.get("fixtures", [])
    ids = {row.get("id") for row in rows}
    if ids != EXPECTED:
        errors.append(f"{name}: fixture ids mismatch missing={sorted(EXPECTED - ids)} extra={sorted(ids - EXPECTED)}")
    for row in rows:
        if row.get("status") not in {"PASS", "FAIL", "UNSUPPORTED"}:
            errors.append(f"{name}:{row.get('id')}: invalid status")
    if data.get("verdict") not in {"VALIDATED", "PARTIAL", "INVALIDATED"}:
        errors.append(f"{name}: invalid verdict")

if errors:
    print("\n".join(errors), file=sys.stderr)
    raise SystemExit(1)
print("results schema valid")
