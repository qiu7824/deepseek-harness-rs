"""Bind Web core publication to the built Host and platform artifact set."""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import re
from datetime import datetime, timezone



def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def select_variants(binary: Path) -> dict:
    return {"variants": ["core"], "verifiedAt": datetime.now(timezone.utc).isoformat(),
            "binarySha256": digest(binary)}


def checked_variants(variants: object) -> list[str]:
    if variants != ["core"]:
        raise ValueError("Web publication supports only the core distribution")
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
    select.add_argument("--binary", type=Path, required=True)
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
    result = select_variants(args.binary)
    args.selection_report.parent.mkdir(parents=True, exist_ok=True)
    args.selection_report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as stream:
            stream.write("variants=" + " ".join(result["variants"]) + "\n")
            stream.write("variants_json=" + json.dumps(result["variants"], separators=(",", ":")) + "\n")
    if args.summary:
        with args.summary.open("a", encoding="utf-8") as stream:
            stream.write(f"\nWeb 核心版；Host SHA-256：{result['binarySha256']}\n")
    print(json.dumps(result, ensure_ascii=True))


if __name__ == "__main__":
    main()
