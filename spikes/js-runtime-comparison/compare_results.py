import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent
FIXTURES = json.loads((ROOT / "fixtures.json").read_text(encoding="utf-8"))["fixtures"]
EXPECTED = [item["id"] for item in FIXTURES]
RISK = {"node_sidecar": 1, "deno_core": 2, "boa": 3}

rows = []
for directory in ("boa", "deno-core", "node-sidecar"):
    path = ROOT / directory / "results.json"
    if not path.is_file():
        print(f"{directory}: results missing", file=sys.stderr)
        raise SystemExit(1)
    data = json.loads(path.read_text(encoding="utf-8"))
    by_id = {item["id"]: item for item in data["fixtures"]}
    statuses = [by_id[item]["status"] for item in EXPECTED]
    build = data.get("build", {})
    rows.append({
        "directory": directory,
        "candidate": data["candidate"],
        "pass": statuses.count("PASS"),
        "fail": statuses.count("FAIL"),
        "unsupported": statuses.count("UNSUPPORTED"),
        "eligible": all(status == "PASS" for status in statuses)
            and bool(data.get("teardown", {}).get("clean"))
            and bool(build.get("success")),
        "risk": RISK.get(data["candidate"], 99),
        "build_ms": int(build.get("elapsed_ms", 2**63 - 1)),
        "artifact_bytes": int(build.get("artifact_bytes", 2**63 - 1)),
        "download_bytes": int(build.get("download_bytes", 0)),
        "verdict": data["verdict"],
        "recommendation": data.get("recommendation", ""),
    })

print("| candidate | pass | fail | unsupported | eligible | build ms | artifact bytes | download bytes |")
print("|---|---:|---:|---:|:---:|---:|---:|---:|")
for row in rows:
    print(
        f"| {row['candidate']} | {row['pass']} | {row['fail']} | {row['unsupported']} | "
        f"{'yes' if row['eligible'] else 'no'} | {row['build_ms']} | {row['artifact_bytes']} | {row['download_bytes']} |"
    )

eligible = sorted(
    (row for row in rows if row["eligible"]),
    key=lambda row: (row["risk"], row["download_bytes"], row["artifact_bytes"], row["build_ms"]),
)
if not eligible:
    print("NO_ELIGIBLE_CANDIDATE", file=sys.stderr)
    raise SystemExit(2)
print(f"SELECTED={eligible[0]['candidate']}")
