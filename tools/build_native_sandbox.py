"""Build a coherent native helper set and write its verified source identity."""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import time

from native_sandbox_identity import HELPERS, IDENTITY_FILE, checkout_identity, file_sha256, source_identity, verify_directory

ROOT = pathlib.Path(__file__).resolve().parents[1]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target-dir", type=pathlib.Path, default=ROOT / "target/native-windows-sandbox")
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--allow-dirty-development", action="store_true")
    parser.add_argument("--timeout-seconds", type=int, default=1800)
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("Windows sandbox binaries must be built on Windows")
    if args.timeout_seconds < 1:
        parser.error("timeout must be positive")
    source = source_identity(ROOT)
    revision, dirty, version = checkout_identity(ROOT)
    if dirty and not args.allow_dirty_development:
        parser.error("release source is dirty; development builds require an explicit flag")
    target = args.target_dir.resolve()
    env = dict(os.environ, DSH_BUILD_SOURCE_ID=source["sha256"])
    command = ["cargo", "build", "--locked", "--release", "--workspace", "--manifest-path",
               str(ROOT / "native/windows-sandbox/Cargo.toml"), "--target-dir", str(target)]
    if args.offline:
        command.append("--offline")
    started = time.monotonic()
    target.mkdir(parents=True, exist_ok=True)
    log_path = target / "native-build.log"
    with log_path.open("w", encoding="utf-8") as log:
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
                                   creationflags=subprocess.CREATE_NO_WINDOW)
        print(json.dumps({"pid": process.pid, "log": str(log_path)}), flush=True)
        while True:
            remaining = args.timeout_seconds - (time.monotonic() - started)
            try:
                code = process.wait(timeout=max(0, min(30, remaining)))
                break
            except subprocess.TimeoutExpired:
                if time.monotonic() - started < args.timeout_seconds:
                    print(json.dumps({"elapsedSeconds": round(time.monotonic() - started),
                                      "logBytes": log_path.stat().st_size}), flush=True)
                    continue
                # The still-live Popen object owns this PID; terminate only its build tree.
                if process.poll() is None:
                    subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], check=False,
                                   timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
                process.wait(timeout=30)
                raise SystemExit("native sandbox build exceeded its deadline; owned build tree terminated")
    if code:
        raise SystemExit(code)
    if source_identity(ROOT) != source or checkout_identity(ROOT) != (revision, dirty, version):
        raise SystemExit("native sandbox source changed during compilation; identity not published")
    directory = target / "release"
    info = json.loads(subprocess.check_output([str(directory / HELPERS[0]), "--build-info"], text=True, timeout=15))
    if info.get("sourceSha256") != source["sha256"] or info.get("revision") != revision or info.get("dirty") != dirty:
        raise SystemExit("native bridge reports a different source identity")
    record = {"schemaVersion": 1, "sourceSha256": source["sha256"], "sourceFiles": source["files"],
              "revision": revision, "dirty": dirty, "productVersion": version, "bridgeBuildInfo": info,
              "helpers": {name: file_sha256(directory / name) for name in HELPERS},
              "elapsedSeconds": round(time.monotonic() - started, 3)}
    temporary = directory / (IDENTITY_FILE + ".tmp")
    temporary.write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temporary.replace(directory / IDENTITY_FILE)
    verify_directory(ROOT, directory, require_clean=not args.allow_dirty_development)
    print(json.dumps({"directory": str(directory), "sourceSha256": source["sha256"],
                      "helpers": record["helpers"], "elapsedSeconds": record["elapsedSeconds"]}), flush=True)


if __name__ == "__main__":
    main()
