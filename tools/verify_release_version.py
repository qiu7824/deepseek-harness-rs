from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess


ROOT = pathlib.Path(__file__).resolve().parents[1]
SEMVER = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")


def workspace_version() -> str:
    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.MULTILINE)
    if match is None:
        raise ValueError("workspace package version is missing")
    return match.group(1)


def web_version() -> str:
    return json.loads((ROOT / "web" / "package.json").read_text(encoding="utf-8"))["version"]


def verify(version: str, binary: pathlib.Path | None = None) -> None:
    if SEMVER.fullmatch(version) is None:
        raise ValueError(f"invalid release version: {version}")
    expected = workspace_version()
    if version != expected:
        raise ValueError(f"release version {version} does not match workspace {expected}")
    if web_version() != expected:
        raise ValueError(f"web version {web_version()} does not match workspace {expected}")
    installer = (ROOT / "packaging/windows/deepseek-harness-rs.iss").read_text(encoding="utf-8")
    declared = re.search(r'^#define MyAppVersion "([^"]+)"', installer, re.MULTILINE)
    if declared is None or declared.group(1) != expected:
        raise ValueError("installer version does not match workspace")
    if binary is not None:
        if not binary.is_file():
            raise FileNotFoundError(f"missing release binary: {binary}")
        output = subprocess.check_output([str(binary), "--version"], text=True).strip()
        reported = output.rsplit(" ", 1)[-1]
        if reported != expected:
            raise ValueError(f"binary version {output!r} does not match {expected}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version")
    parser.add_argument("--print-version", action="store_true")
    parser.add_argument("--binary", type=pathlib.Path)
    args = parser.parse_args()
    if args.print_version:
        print(workspace_version())
        return
    if args.version is None:
        parser.error("--version is required unless --print-version is used")
    verify(args.version, args.binary.resolve() if args.binary else None)
    print(args.version)


if __name__ == "__main__":
    main()
