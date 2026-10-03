"""Publish only synthetic icon golden PNGs to a unique, parentless diagnostic branch."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import tempfile


FLUTTER_VERSION = "3.47.5"
FLUTTER_REVISION = "6a19cca56475dbfba1478ee68d7bd0c2ef891da1"
GOLDENS = tuple(f"icons_{theme}_{ratio}x.png" for theme in ("light", "dark") for ratio in ("1.0", "1.5"))
DIFF_TYPES = ("masterImage", "testImage", "isolatedDiff", "maskedDiff")
TARGETS = {("windows", "x86_64"), ("macos", "x86_64"), ("macos", "aarch64")}
MAX_PNG_BYTES = 8 * 1024 * 1024


def git(repo: Path, arguments: list[str], *, data: bytes | None = None, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(["git", *arguments], cwd=repo, input=data, capture_output=True, env=env)
    if result.returncode:
        # Never print authentication configuration, environment variables or remote credentials.
        raise RuntimeError(f"git {arguments[0]} failed with exit code {result.returncode}")
    return result.stdout.decode("utf-8", errors="replace").strip()


def read_png(repo: Path, source: Path) -> bytes:
    if source.is_symlink() or any(parent.is_symlink() for parent in source.parents if parent != repo):
        raise ValueError("golden evidence must not contain symbolic links")
    source.resolve(strict=True).relative_to(repo)
    if not source.is_file() or source.stat().st_size > MAX_PNG_BYTES:
        raise ValueError("golden evidence is not a bounded regular PNG file")
    data = source.read_bytes()
    if len(data) < 24 or data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise ValueError("golden evidence has an invalid PNG header")
    width, height = struct.unpack(">II", data[16:24])
    if not 0 < width <= 4096 or not 0 < height <= 4096:
        raise ValueError("golden evidence PNG dimensions exceed the diagnostic limit")
    return data


def baseline_root(repo: Path, platform: str, arch: str) -> Path:
    root = repo / "apps/desktop_flutter/test/goldens"
    arm = root / "macos-arm64"
    return arm if (platform, arch) == ("macos", "aarch64") and arm.is_dir() else root


def collect(repo: Path, platform: str, arch: str) -> dict[str, bytes]:
    test_root = repo / "apps/desktop_flutter/test"
    files: dict[str, bytes] = {}
    for name in GOLDENS:
        for kind in DIFF_TYPES:
            filename = f"{Path(name).stem}_{kind}.png"
            source = test_root / "failures" / filename
            if source.exists() or source.is_symlink():
                files[f"failures/{filename}"] = read_png(repo, source)
    if not files:
        return {}
    for name in GOLDENS:
        files[f"baselines/{name}"] = read_png(repo, baseline_root(repo, platform, arch) / name)
    return files


def create_commit(repo: Path, *, platform: str, arch: str, run_id: str, run_attempt: str, source: str) -> tuple[str, str | None]:
    repo = repo.resolve(strict=True)
    if (platform, arch) not in TARGETS:
        raise ValueError("unsupported golden diagnostic target")
    if not re.fullmatch(r"[0-9]{1,25}", run_id) or not re.fullmatch(r"[0-9]{1,5}", run_attempt):
        raise ValueError("invalid Actions run identity")
    if not re.fullmatch(r"[0-9a-f]{40}", source) or git(repo, ["rev-parse", "HEAD"]) != source:
        raise ValueError("golden evidence source does not match the checked-out commit")
    branch = f"diagnostics/flutter-goldens-{run_id}-{platform}-{arch}"
    files = collect(repo, platform, arch)
    if not files:
        return branch, None
    manifest = {
        "sourceRevision": source,
        "flutterVersion": FLUTTER_VERSION,
        "flutterRevision": FLUTTER_REVISION,
        "platform": platform,
        "arch": arch,
        "runId": run_id,
        "runAttempt": run_attempt,
        "test": "apps/desktop_flutter/test/design_icon_golden_test.dart",
        "baselineRoot": baseline_root(repo, platform, arch).relative_to(repo).as_posix(),
        "files": {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in sorted(files.items())},
    }
    files["manifest.json"] = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    with tempfile.TemporaryDirectory(prefix="dsh-golden-index-") as temporary:
        env = {
            **os.environ,
            "GIT_INDEX_FILE": str(Path(temporary) / "index"),
            "GIT_AUTHOR_NAME": "github-actions[bot]",
            "GIT_AUTHOR_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com",
            "GIT_COMMITTER_NAME": "github-actions[bot]",
            "GIT_COMMITTER_EMAIL": "41898282+github-actions[bot]@users.noreply.github.com",
        }
        git(repo, ["read-tree", "--empty"], env=env)
        for name, data in sorted(files.items()):
            blob = git(repo, ["hash-object", "-w", "--stdin"], data=data, env=env)
            git(repo, ["update-index", "--add", "--cacheinfo", f"100644,{blob},{name}"], env=env)
        tree = git(repo, ["write-tree"], env=env)
        # No -p argument: this commit has no source history and only the PNG whitelist + manifest.
        commit = git(repo, ["commit-tree", tree], data=f"Synthetic Flutter icon golden evidence ({platform}/{arch})\n".encode("utf-8"), env=env)
    return branch, commit


def publish(repo: Path, branch: str, commit: str) -> None:
    existing = subprocess.run(["git", "ls-remote", "--exit-code", "--heads", "origin", f"refs/heads/{branch}"], cwd=repo, capture_output=True)
    if existing.returncode == 0:
        raise ValueError("the unique golden diagnostic branch already exists; refusing to overwrite it")
    if existing.returncode != 2:
        raise RuntimeError(f"git remote branch check failed with exit code {existing.returncode}")
    git(repo, ["push", "origin", f"{commit}:refs/heads/{branch}"])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--run-attempt", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--publish", action="store_true", help="Push the unique diagnostic branch using existing checkout Git authentication")
    args = parser.parse_args()
    branch, commit = create_commit(args.repo, platform=args.platform, arch=args.arch, run_id=args.run_id, run_attempt=args.run_attempt, source=args.source)
    if commit is None:
        print("No synthetic icon golden failure PNGs were produced; no diagnostic branch created.")
    elif args.publish:
        publish(args.repo, branch, commit)
        print(f"::notice title=Flutter golden evidence::{branch} (commit {commit})", flush=True)
    else:
        print(f"Prepared parentless golden evidence commit {commit}; publication was not requested.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
