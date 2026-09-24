"""Bind the three Windows sandbox helpers to the exact local source bundle."""
from __future__ import annotations

import hashlib
import json
import pathlib
import subprocess
import tomllib
from collections.abc import Callable

HELPERS = ("dsh-windows-native.exe", "dsh-command-runner.exe", "dsh-windows-sandbox-setup.exe")
IDENTITY_FILE = "BUILD_IDENTITY.json"


def file_sha256(path: pathlib.Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def source_identity(root: pathlib.Path) -> dict:
    native = root / "native/windows-sandbox"
    paths = [path for path in native.rglob("*") if path.is_file()
             and not {"target", ".git", "__pycache__"}.intersection(path.relative_to(native).parts)]
    paths += [root / "Cargo.toml", root / "crates/host/dsh-cli/build_identity.rs"]
    rows = []
    for path in sorted(paths, key=lambda item: item.relative_to(root).as_posix()):
        if path.is_symlink() or not path.resolve().is_relative_to(root.resolve()):
            raise ValueError(f"native source leaves checkout: {path}")
        rows.append({"path": path.relative_to(root).as_posix(), "sha256": file_sha256(path)})
    encoded = json.dumps(rows, ensure_ascii=False, separators=(",", ":")).encode()
    return {"sha256": hashlib.sha256(encoded).hexdigest(), "files": rows}


def checkout_identity(root: pathlib.Path) -> tuple[str, bool, str]:
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=root, text=True).strip())
    version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    return revision, dirty, version


def verify_record(record: dict, source_sha: str, revision: str, version: str,
                  binary_hash: Callable[[str], str], *, require_clean: bool = True) -> None:
    info = record.get("bridgeBuildInfo", {})
    if (record.get("schemaVersion") != 1 or record.get("sourceSha256") != source_sha
            or record.get("revision") != revision or record.get("productVersion") != version
            or info.get("sourceSha256") != source_sha or info.get("revision") != revision):
        raise ValueError("native sandbox source/build identity is stale; rebuild all helpers")
    if require_clean and (record.get("dirty") is not False or info.get("dirty") is not False):
        raise ValueError("native sandbox release must come from clean source")
    hashes = record.get("helpers", {})
    if set(hashes) != set(HELPERS):
        raise ValueError("native sandbox identity must cover all three helpers")
    for name in HELPERS:
        if hashes[name] != binary_hash(name):
            raise ValueError(f"native sandbox helper checksum mismatch: {name}")


def verify_directory(root: pathlib.Path, directory: pathlib.Path, *, require_clean: bool = True) -> dict:
    record = json.loads((directory / IDENTITY_FILE).read_text(encoding="utf-8"))
    revision, dirty, version = checkout_identity(root)
    if require_clean and dirty:
        raise ValueError("native sandbox release checkout is dirty")
    verify_record(record, source_identity(root)["sha256"], revision, version,
                  lambda name: file_sha256(directory / name), require_clean=require_clean)
    return record
