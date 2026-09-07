"""Select attested release variants and enforce each platform's artifact set."""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import re
from datetime import datetime, timezone

from free_model_evidence import validated_models


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def select_variants(report_path: Path, binary: Path, outcome: str) -> dict:
    result = {"variants": ["core", "skin"], "verifiedAt": datetime.now(timezone.utc).isoformat(),
              "free": {"status": "unavailable", "probeOutcome": outcome}}
    report = {}
    try:
        report = json.loads(report_path.read_text(encoding="utf-8"))
        if outcome != "success":
            raise ValueError("免费模型完整运行链路校验未通过")
        binary_hash = digest(binary)
        rows = validated_models(report, binary_hash)
    except Exception as error:
        reasons = []
        if isinstance(report, dict):
            rows = report.get("models")
            reasons = [row["reason"] for row in rows if isinstance(row, dict) and isinstance(row.get("reason"), str)] if isinstance(rows, list) else []
            failure = report.get("verificationError")
            if isinstance(failure, dict) and isinstance(failure.get("reason"), str):
                reasons.append(failure["reason"])
        result["free"]["reason"] = "；".join(dict.fromkeys(reasons))[:1800] or str(error)[:1800]
    else:
        result["variants"].append("free")
        result["binarySha256"] = binary_hash
        result["free"].update(status="verified", includedModels=[{"provider": row["provider"], "model": row["model"]} for row in rows])
    return result


def checked_variants(variants: object) -> list[str]:
    if variants not in (["core", "skin"], ["core", "skin", "free"]):
        raise ValueError("release variants must contain core/skin and optionally attested free")
    return list(variants)


def expected_artifacts(prefix: str, platform: str, variants: object) -> set[str]:
    if re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", prefix) is None:
        raise ValueError("invalid release prefix")
    variants = checked_variants(variants)
    portable = "zip" if platform == "windows" else "tar.gz"
    installer = {"windows": "setup.exe", "linux": "deb", "macos": "pkg"}[platform]
    names = {f"{prefix}-{variant}-portable.{portable}" for variant in variants}
    names.update(f"{prefix}-{variant}-{installer}" if platform == "windows" else f"{prefix}-{variant}.{installer}" for variant in variants)
    return names


def write_checksums(directory: Path, prefix: str, platform: str, variants: object, output: Path) -> None:
    expected = expected_artifacts(prefix, platform, variants)
    files = [path for path in directory.glob(f"{prefix}-*") if path.is_file()]
    actual = {path.name for path in files}
    if actual != expected:
        raise ValueError(f"unexpected release artifact set: missing={sorted(expected-actual)}, extra={sorted(actual-expected)}")
    output.write_text("".join(f"{digest(path)}  {path.name}\n" for path in sorted(files)), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    select = subparsers.add_parser("select")
    select.add_argument("--report", type=Path, required=True)
    select.add_argument("--binary", type=Path, required=True)
    select.add_argument("--probe-outcome", choices=["success", "failure", "cancelled", "skipped"], required=True)
    select.add_argument("--selection-report", type=Path, required=True)
    select.add_argument("--github-output", type=Path)
    select.add_argument("--summary", type=Path)
    checksums = subparsers.add_parser("checksums")
    checksums.add_argument("--directory", type=Path, required=True)
    checksums.add_argument("--prefix", required=True)
    checksums.add_argument("--platform", choices=["windows", "linux", "macos"], required=True)
    checksums.add_argument("--variants-json", required=True)
    checksums.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "checksums":
        write_checksums(args.directory, args.prefix, args.platform, json.loads(args.variants_json), args.output)
        return
    result = select_variants(args.report, args.binary, args.probe_outcome)
    args.selection_report.parent.mkdir(parents=True, exist_ok=True)
    args.selection_report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as stream:
            stream.write("variants=" + " ".join(result["variants"]) + "\n")
            stream.write("variants_json=" + json.dumps(result["variants"], separators=(",", ":")) + "\n")
    if args.summary:
        reason = result["free"].get("reason", "匿名流式、工具往返和当前正式二进制校验通过")
        with args.summary.open("a", encoding="utf-8") as stream:
            stream.write(f"\n发布版本：{', '.join(result['variants'])}。\n\n免费版：{result['free']['status']}；{reason}\n")
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
