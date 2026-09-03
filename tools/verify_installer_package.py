from __future__ import annotations

import argparse
import pathlib
import re
import shutil
import subprocess
import tempfile


ROOT = pathlib.Path(__file__).resolve().parents[1]
SAFE_COMPONENT = re.compile(r"^[0-9A-Za-z][0-9A-Za-z._-]*$")


def safe(field: str, value: str) -> str:
    if SAFE_COMPONENT.fullmatch(value) is None or value in {".", ".."}:
        raise ValueError(f"invalid {field}: {value!r}")
    return value


def expected_stage(version: str, platform: str, arch: str, variant: str) -> pathlib.Path:
    return ROOT / "dist" / f"deepseek-harness-rs-v{version}-{platform}-{arch}-{variant}"


def assert_same_tree(actual: pathlib.Path, expected: pathlib.Path) -> None:
    actual_files = {
        path.relative_to(actual).as_posix(): path
        for path in actual.rglob("*")
        if path.is_file()
    }
    expected_files = {
        path.relative_to(expected).as_posix(): path
        for path in expected.rglob("*")
        if path.is_file()
    }
    if set(actual_files) != set(expected_files):
        raise SystemExit(
            f"installer payload differs from verified stage: "
            f"missing={sorted(set(expected_files)-set(actual_files))[:20]}, "
            f"extra={sorted(set(actual_files)-set(expected_files))[:20]}"
        )
    for name, expected_path in expected_files.items():
        if actual_files[name].read_bytes() != expected_path.read_bytes():
            raise SystemExit(f"installer payload byte mismatch: {name}")


def verify_deb(package: pathlib.Path, stage: pathlib.Path, variant: str) -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = pathlib.Path(temporary) / "root"
        subprocess.run(["dpkg-deb", "-x", str(package), str(root)], check=True)
        payload = root / "opt" / "deepseek-harness-rs" / variant
        assert_same_tree(payload, stage)


def verify_pkg(package: pathlib.Path, stage: pathlib.Path, variant: str) -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = pathlib.Path(temporary) / "root"
        subprocess.run(["pkgutil", "--expand-full", str(package), str(root)], check=True)
        candidates = [
            path
            for path in root.rglob(variant)
            if path.is_dir()
            and path.as_posix().endswith(f"usr/local/lib/deepseek-harness-rs/{variant}")
        ]
        if len(candidates) != 1:
            raise SystemExit(f"macOS package payload root is ambiguous: {candidates}")
        assert_same_tree(candidates[0], stage)


def verify_windows(package: pathlib.Path, stage: pathlib.Path) -> None:
    # Inno Setup emits a deterministic compiled-code manifest. `innounp` is
    # installed by the workflow and extracts without executing the installer.
    innounp = shutil.which("innounp") or shutil.which("innounp.exe")
    if innounp is None:
        raise SystemExit("innounp is required to verify the Windows installer")
    with tempfile.TemporaryDirectory() as temporary:
        root = pathlib.Path(temporary) / "root"
        root.mkdir()
        subprocess.run([innounp, "-x", f"-d{root}", str(package)], check=True)
        payload = root / "{app}"
        if not payload.is_dir():
            candidates = [path for path in root.rglob("{app}") if path.is_dir()]
            if len(candidates) != 1:
                raise SystemExit(f"Windows installer payload root is ambiguous: {candidates}")
            payload = candidates[0]
        assert_same_tree(payload, stage)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["windows", "linux", "macos"], required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--variant", choices=["core", "skin", "free"], required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    arch = safe("arch", args.arch)
    version = safe("version", args.version)
    stage = expected_stage(version, args.platform, arch, args.variant)
    if not stage.is_dir():
        raise SystemExit(f"missing verified release stage: {stage}")
    prefix = f"deepseek-harness-rs-v{version}-{args.platform}-{arch}-{args.variant}"
    if args.platform == "windows":
        package = ROOT / "dist" / f"{prefix}-setup.exe"
    elif args.platform == "linux":
        package = ROOT / "dist" / f"{prefix}.deb"
    else:
        package = ROOT / "dist" / f"{prefix}.pkg"
    if not package.is_file():
        raise SystemExit(f"missing installer package: {package}")
    if args.platform == "windows":
        verify_windows(package, stage)
    elif args.platform == "linux":
        verify_deb(package, stage, args.variant)
    else:
        verify_pkg(package, stage, args.variant)
    print(package)


if __name__ == "__main__":
    main()
