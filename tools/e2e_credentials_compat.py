from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
import pathlib
import queue
import re
import subprocess
import tempfile
import threading
import time


def run_once(binary: pathlib.Path, home: pathlib.Path) -> dict[str, object]:
    process = subprocess.Popen(
        [str(binary), "web", "--port", "0"],
        cwd=binary.parent,
        env={**os.environ, "DSH_HOME": str(home)},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        creationflags=getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0),
    )
    lines: queue.Queue[str] = queue.Queue()
    threads = []
    for stream in (process.stdout, process.stderr):
        thread = threading.Thread(target=lambda s=stream: [lines.put(line.rstrip()) for line in iter(s.readline, "")], daemon=True)
        thread.start()
        threads.append(thread)
    try:
        deadline = time.monotonic() + 30
        port = None
        observed = []
        while time.monotonic() < deadline and port is None:
            if process.poll() is not None and lines.empty():
                break
            try:
                line = lines.get(timeout=0.2)
            except queue.Empty:
                continue
            observed.append(line)
            match = re.search(r"dsh web: http://127\.0\.0\.1:(\d+)", line)
            if match:
                port = int(match.group(1))
        if port is None:
            raise RuntimeError(f"dsh did not start: {observed[-8:]}")
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        connection.request("GET", "/")
        response = connection.getresponse()
        response.read()
        connection.close()
        if response.status != 200:
            raise RuntimeError(f"unexpected root status {response.status}")
        return {"port": port, "root_status": response.status}
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    args = parser.parse_args()
    binary = pathlib.Path(args.binary).resolve()
    with tempfile.TemporaryDirectory(prefix="dsh-credentials-e2e-") as temporary:
        home = pathlib.Path(temporary)
        document = (
            "version: 1\n"
            "refs:\n"
            "  DSH_COMPAT_KEY: fixture-ref\n"
            "records:\n"
            "  llm-pi-ai/fixture:\n"
            "    kind: grant\n"
            "    payload:\n"
            "      access: fixture-value\n"
        )
        path = home / ".credentials.yaml"
        path.write_text(document, encoding="utf-8")
        before = hashlib.sha256(path.read_bytes()).hexdigest()
        first = run_once(binary, home)
        middle = hashlib.sha256(path.read_bytes()).hexdigest()
        second = run_once(binary, home)
        after = hashlib.sha256(path.read_bytes()).hexdigest()
        if before != middle or middle != after:
            raise AssertionError("versioned credentials document changed during read-only startup")
        print(json.dumps({
            "document_preserved": True,
            "first_root_status": first["root_status"],
            "restart_root_status": second["root_status"],
        }, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
