"""Validate and render the machine-derived memory evidence block."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys

START = "<!-- MEMORY-EVIDENCE:START -->"
END = "<!-- MEMORY-EVIDENCE:END -->"


def _records(report: pathlib.Path) -> list[dict[str, object]]:
    raw = report.read_bytes()
    if b"DeepSeek Harness" in raw:
        raise RuntimeError("plain home path leaked into report")
    try:
        records = [json.loads(line) for line in raw.splitlines() if line.strip()]
    except json.JSONDecodeError as error:
        raise RuntimeError("invalid JSONL report") from error
    if not records or any(row.get("schema_version") != 1 for row in records):
        raise RuntimeError("unsupported memory report schema")
    return records


def render_evidence_markdown(report: pathlib.Path) -> str:
    records = _records(report)
    snapshots = [row for row in records if row.get("type") == "snapshot"]
    workloads = [row for row in records if row.get("type") == "workload"]
    pids = {row.get("pid") for row in snapshots}
    binaries = {row.get("binary_sha256") for row in snapshots}
    homes = {row.get("home_path_sha256") for row in snapshots}
    if len(pids) != 1 or len(binaries) != 1 or len(homes) != 1:
        raise RuntimeError("snapshot production identity drift")
    lines = [
        START,
        "## 机器派生证据（请勿手工编辑）",
        "",
        f"- PID：`{next(iter(pids))}`",
        f"- 二进制SHA-256：`{next(iter(binaries))}`",
        f"- 报告SHA-256：`{hashlib.sha256(report.read_bytes()).hexdigest()}`",
        f"- 记录：{len(records)}（snapshot {len(snapshots)} / workload {len(workloads)}）",
        "",
        "| 采样点 | Working Set MB | Private MB | 线程 | Handles |",
        "|---|---:|---:|---:|---:|",
    ]
    for row in snapshots:
        lines.append(
            f"| {row['label']} | {row['working_set_bytes']/1048576:.1f} | "
            f"{row['private_bytes']/1048576:.1f} | {row['threads']} | {row['handles']} |"
        )
    lines += ["", "| 工作负载 | 批次请求 | 累计请求 | 响应MB | 秒 |", "|---|---:|---:|---:|---:|"]
    for row in workloads:
        lines.append(
            f"| {row['label']} | {row['requests']} | {row['cumulative_requests']} | "
            f"{row['response_bytes']/1048576:.2f} | {row['elapsed_seconds']:.3f} |"
        )
    lines += [END, ""]
    return "\n".join(lines)


def update_markdown(report: pathlib.Path, markdown: pathlib.Path) -> None:
    block = render_evidence_markdown(report)
    text = markdown.read_text(encoding="utf-8") if markdown.exists() else "# Rust Harness 正式内存基线\n\n"
    if START in text and END in text:
        prefix, rest = text.split(START, 1)
        _, suffix = rest.split(END, 1)
        text = prefix + block + suffix.lstrip("\n")
    else:
        text = text.rstrip() + "\n\n" + block
    markdown.write_text(text, encoding="utf-8")


def validate(report: pathlib.Path, markdown: pathlib.Path) -> dict[str, int]:
    expected = render_evidence_markdown(report).strip()
    text = markdown.read_text(encoding="utf-8")
    if START not in text or END not in text:
        raise RuntimeError("markdown evidence drift: block missing")
    actual = (START + text.split(START, 1)[1].split(END, 1)[0] + END).strip()
    if actual != expected:
        raise RuntimeError("markdown evidence drift")
    records = _records(report)
    return {
        "records": len(records),
        "snapshots": sum(row.get("type") == "snapshot" for row in records),
        "workloads": sum(row.get("type") == "workload" for row in records),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", required=True)
    parser.add_argument("--markdown", required=True)
    parser.add_argument("--update", action="store_true")
    args = parser.parse_args(argv)
    report, markdown = pathlib.Path(args.report), pathlib.Path(args.markdown)
    if args.update:
        update_markdown(report, markdown)
    print(json.dumps(validate(report, markdown), separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
