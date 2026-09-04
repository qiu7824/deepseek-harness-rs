from __future__ import annotations

import argparse
import contextlib
import hashlib
import http.client
import json
import os
import pathlib
import queue
import re
import shutil
import subprocess
import tempfile
import threading
import time
from collections.abc import Iterator


def read_lines(stream, output: queue.Queue[str]) -> None:
    for line in iter(stream.readline, ""):
        output.put(line.rstrip("\r\n"))


@contextlib.contextmanager
def running_host(binary: pathlib.Path, repo: pathlib.Path, home: pathlib.Path) -> Iterator[int]:
    process = subprocess.Popen(
        [str(binary), "web", "--port", "0"],
        cwd=repo,
        env={**os.environ, "DSH_HOME": str(home)},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        creationflags=getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0),
    )
    lines: queue.Queue[str] = queue.Queue()
    assert process.stdout is not None
    assert process.stderr is not None
    threading.Thread(target=read_lines, args=(process.stdout, lines), daemon=True).start()
    threading.Thread(target=read_lines, args=(process.stderr, lines), daemon=True).start()
    observed: list[str] = []
    try:
        deadline = time.monotonic() + 40
        port: int | None = None
        while time.monotonic() < deadline:
            if process.poll() is not None:
                while True:
                    try:
                        observed.append(lines.get_nowait())
                    except queue.Empty:
                        break
                raise AssertionError(f"dsh exited before readiness: {process.returncode}: {observed[-10:]!r}")
            try:
                line = lines.get(timeout=0.25)
            except queue.Empty:
                continue
            observed.append(line)
            match = re.fullmatch(r"dsh web: http://127\.0\.0\.1:(\d+)", line)
            if match:
                port = int(match.group(1))
                break
        if port is None:
            raise AssertionError(f"dsh did not report readiness: {observed[-10:]!r}")
        yield port
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)


def rpc(port: int, method: str, payload: dict[str, object], sequence: int) -> dict[str, object]:
    rpc_id = f"settings-data-e2e-{sequence}"
    body = json.dumps(
        {"type": "client-request", "rpcId": rpc_id, "method": method, "payload": payload},
        separators=(",", ":"),
    ).encode("utf-8")
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=20)
    connection.request(
        "POST",
        f"/api/{method}",
        body=body,
        headers={
            "content-type": "application/json",
            "content-length": str(len(body)),
            "origin": f"http://127.0.0.1:{port}",
            "sec-fetch-site": "same-origin",
        },
    )
    response = connection.getresponse()
    raw = response.read()
    connection.close()
    if response.status != 200:
        raise AssertionError(f"{method}: HTTP {response.status}: {raw[:300]!r}")
    decoded = json.loads(raw)
    if decoded.get("rpcId") != rpc_id:
        raise AssertionError(f"{method}: wrong rpcId")
    return decoded


def require_ok(response: dict[str, object], method: str) -> dict[str, object]:
    result = response.get("result")
    if not isinstance(result, dict) or result.get("ok") is not True:
        raise AssertionError(f"{method}: {result!r}")
    value = result.get("value")
    return value if isinstance(value, dict) else {}


def file_manifest(root: pathlib.Path) -> dict[str, tuple[int, str]]:
    if not root.exists():
        return {}
    manifest: dict[str, tuple[int, str]] = {}
    for path in sorted(candidate for candidate in root.rglob("*") if candidate.is_file()):
        data = path.read_bytes()
        manifest[path.relative_to(root).as_posix()] = (len(data), hashlib.sha256(data).hexdigest())
    return manifest


def session_manifest(root: pathlib.Path) -> dict[str, tuple[int, str]]:
    """Fingerprint only published session logs, never atomic-write temporaries."""
    return {
        path: fingerprint
        for path, fingerprint in file_manifest(root).items()
        if path.endswith("/session.jsonl") or path.endswith("/session.jsonl.zstd")
    }


def wait_for_session_manifest(
    root: pathlib.Path, expected_count: int = 1
) -> dict[str, tuple[int, str]]:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        manifest = session_manifest(root)
        if len(manifest) >= expected_count:
            return manifest
        time.sleep(0.05)
    raise AssertionError(
        f"session.create published {len(session_manifest(root))}/{expected_count} durable session logs"
    )


def workspace_manifest(home: pathlib.Path) -> dict[str, tuple[int, str]]:
    return {
        path: fingerprint
        for path, fingerprint in file_manifest(home / "storages").items()
        if path.startswith("workspace")
    }


def assert_visible(port: int, workspace_id: str, session_id: str, sequence: int) -> int:
    workspaces = require_ok(rpc(port, "workspace.list", {}, sequence), "workspace.list")
    sequence += 1
    sessions = require_ok(rpc(port, "session.list", {}, sequence), "session.list")
    sequence += 1
    workspace_items = workspaces.get("items")
    session_items = sessions.get("items")
    if not isinstance(workspace_items, list) or not any(
        isinstance(item, dict)
        and item.get("workspaceId") == workspace_id
        and session_id in item.get("sessionIds", [])
        for item in workspace_items
    ):
        raise AssertionError(f"workspace/session link disappeared: {workspaces!r}")
    if not isinstance(session_items, list) or not any(
        isinstance(item, dict) and item.get("sessionId") == session_id for item in session_items
    ):
        raise AssertionError(f"session disappeared: {sessions!r}")
    require_ok(
        rpc(port, "session.history", {"sessionId": session_id, "maxMessages": 8}, sequence),
        "session.history",
    )
    sequence += 1
    return sequence


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--repo", default=str(pathlib.Path(__file__).resolve().parents[1]))
    parser.add_argument("--relocate", action="store_true", help="Restart from a copied package against the same data directory")
    args = parser.parse_args()
    binary = pathlib.Path(args.binary).resolve()
    repo = pathlib.Path(args.repo).resolve()
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}")

    with contextlib.ExitStack() as stack:
        temporary = stack.enter_context(tempfile.TemporaryDirectory(prefix="dsh-settings-data-e2e-"))
        home = pathlib.Path(temporary)
        sequence = 1
        fixtures: list[tuple[str, str]] = []
        with running_host(binary, repo, home) as port:
            workspace_root = home / "workspace-fixtures"
            for workspace_number in range(2):
                cwd = workspace_root / f"workspace-{workspace_number + 1}"
                cwd.mkdir(parents=True)
                workspace = require_ok(
                    rpc(port, "workspace.create", {"path": str(cwd)}, sequence),
                    "workspace.create",
                ).get("workspace")
                sequence += 1
                if not isinstance(workspace, dict) or not isinstance(
                    workspace.get("workspaceId"), str
                ):
                    raise AssertionError(f"workspace.create returned no workspace: {workspace!r}")
                workspace_id = workspace["workspaceId"]
                for _ in range(2):
                    created = require_ok(
                        rpc(
                            port,
                            "session.create",
                            {"workspaceId": workspace_id, "cwd": str(cwd)},
                            sequence,
                        ),
                        "session.create",
                    )
                    sequence += 1
                    session_id = created.get("sessionId")
                    if not isinstance(session_id, str) or not session_id:
                        raise AssertionError(f"session.create returned no sessionId: {created!r}")
                    fixtures.append((workspace_id, session_id))

            for workspace_id, session_id in fixtures:
                sequence = assert_visible(port, workspace_id, session_id, sequence)
            sessions_before = wait_for_session_manifest(home / "sessions", len(fixtures))
            storages_before = workspace_manifest(home)
            if len(sessions_before) != len(fixtures) or not storages_before:
                raise AssertionError(
                    f"fixture did not create durable data: sessions={sessions_before}, storages={storages_before}"
                )

            provider_value = {
                "keyless": True,
                "api": "openai-responses",
                "baseURL": "http://127.0.0.1:9/v1",
                "models": [{"id": "gpt-settings-data-fixture", "input": ["text"]}],
            }
            require_ok(
                rpc(
                    port,
                    "settings.mutate",
                    {
                        "ns": "llm-pi-ai",
                        "ops": [
                            {
                                "op": "set",
                                "path": ["providers", "settings-data-fixture"],
                                "value": provider_value,
                            }
                        ],
                    },
                    sequence,
                ),
                "settings.mutate",
            )
            sequence += 1
            for workspace_id, session_id in fixtures:
                sequence = assert_visible(port, workspace_id, session_id, sequence)
            sessions_after_mutate = session_manifest(home / "sessions")
            if sessions_after_mutate != sessions_before:
                changed = {
                    path: {"before": sessions_before.get(path), "after": sessions_after_mutate.get(path)}
                    for path in sorted(set(sessions_before) | set(sessions_after_mutate))
                    if sessions_before.get(path) != sessions_after_mutate.get(path)
                }
                raise AssertionError(
                    "settings.mutate changed durable session files: " + json.dumps(changed, sort_keys=True)
                )
            if workspace_manifest(home) != storages_before:
                raise AssertionError("settings.mutate changed durable workspace files")

            # A fresh set of list/history requests models a browser reload against the same Host.
            for workspace_id, session_id in fixtures:
                sequence = assert_visible(port, workspace_id, session_id, sequence)

        restart_binary, restart_repo = binary, repo
        if args.relocate:
            relocated = pathlib.Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="dsh-relocated-package-"))) / "package"
            shutil.copytree(binary.parent, relocated)
            restart_binary, restart_repo = relocated / binary.name, relocated
        with running_host(restart_binary, restart_repo, home) as restarted_port:
            for workspace_id, session_id in fixtures:
                sequence = assert_visible(restarted_port, workspace_id, session_id, sequence)
            if session_manifest(home / "sessions") != sessions_before:
                raise AssertionError("same-home restart changed durable session files")
            storages_after_restart = workspace_manifest(home)
            if storages_after_restart != storages_before:
                changed = {
                    path: {"before": storages_before.get(path), "after": storages_after_restart.get(path)}
                    for path in sorted(set(storages_before) | set(storages_after_restart))
                    if storages_before.get(path) != storages_after_restart.get(path)
                }
                raise AssertionError(
                    "same-home restart changed storage files: " + json.dumps(changed, sort_keys=True)
                )

        print(
            json.dumps(
                {
                    "settings_mutated": True,
                    "immediate_visible": True,
                    "fresh_list_visible": True,
                    "same_home_restart_visible": True,
                    "sessions_verified": len(fixtures),
                    "workspaces_verified": len({workspace_id for workspace_id, _ in fixtures}),
                    "session_files_preserved": True,
                    "workspace_files_preserved": True,
                    "relocated_package_restart": args.relocate,
                },
                sort_keys=True,
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
