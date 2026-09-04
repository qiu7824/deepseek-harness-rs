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
import stat
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
        creationflags=(getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0) | getattr(subprocess, "CREATE_NO_WINDOW", 0)),
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


class RuntimeStatusError(AssertionError):
    def __init__(self, status: int, raw: bytes):
        self.status = status
        super().__init__(f"runtime endpoint: HTTP {status}: {raw[:500]!r}")


def runtime_request(port: int, payload: dict[str, object] | None = None) -> dict[str, object]:
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=15)
    try:
        connection.request(
            "GET" if payload is None else "POST", "/__dsh-runtime",
            body=None if payload is None else json.dumps(payload).encode("utf-8"),
            headers={"content-type": "application/json", "origin": f"http://127.0.0.1:{port}", "sec-fetch-site": "same-origin"},
        )
        response = connection.getresponse()
        raw = response.read()
        if response.status != 200:
            raise RuntimeStatusError(response.status, raw)
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise AssertionError("runtime endpoint returned no state object")
        return value
    finally:
        connection.close()


def assert_runtime_home(runtime: dict[str, object], home: pathlib.Path) -> None:
    if runtime.get("migrationError"):
        raise AssertionError(f"runtime data migration failed: {runtime['migrationError']}")
    paths = runtime.get("paths") or {}
    actual = paths.get("dataDirectory")
    if not isinstance(actual, str) or not pathlib.Path(actual).exists() or not os.path.samefile(actual, home):
        raise AssertionError(f"runtime is using the wrong data directory: {actual!r}, expected {str(home)!r}")


def managed_profile_links(home: pathlib.Path) -> list[str]:
    """Inspect only package-link slots; never recurse into their package targets."""
    modules = home / "profiles" / "node_modules"
    if not modules.is_dir():
        return []
    def linked(path: pathlib.Path) -> bool:
        info = path.lstat()
        return stat.S_ISLNK(info.st_mode) or bool(getattr(info, "st_file_attributes", 0) & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400))
    links = []
    for entry in modules.iterdir():
        if linked(entry):
            links.append(entry.relative_to(home).as_posix())
        elif entry.name.startswith("@") and entry.is_dir():
            for package in entry.iterdir():
                if linked(package):
                    links.append(package.relative_to(home).as_posix())
    return sorted(links)


def migrate_home(
    port: int, home: pathlib.Path, destination: pathlib.Path,
    fixtures: list[tuple[str, str]], sequence: int,
    sessions_before: dict[str, tuple[int, str]], storages_before: dict[str, tuple[int, str]],
) -> tuple[int, dict[str, object]]:
    initial = runtime_request(port)
    assert_runtime_home(initial, home)
    instance = initial.get("instanceId")
    if not isinstance(instance, str) or not instance:
        raise AssertionError("runtime has no instanceId to verify restart")
    environment = pathlib.Path(initial["paths"]["environmentDirectory"]).resolve()
    inside_home = False
    for parent in environment.parents:
        try:
            if os.path.samefile(parent, home):
                inside_home = True
                break
        except OSError:
            continue
    if not inside_home:
        raise AssertionError("migration fixture environment is outside the isolated data directory")
    environment.mkdir(parents=True, exist_ok=True)
    marker = home / "migration-marker.txt"
    environment_marker = environment / "migration-runtime-marker.txt"
    marker.write_bytes(b"persisted migration fixture\n")
    environment_marker.write_bytes(b"persisted runtime environment\n")
    links = managed_profile_links(home)
    if os.name == "nt" and not links:
        raise AssertionError("--migrate-home requires a packaged binary that creates installer-managed profiles/node_modules links")
    described = require_ok(rpc(port, "settings.describe", {}, sequence), "settings.describe")
    sequence += 1
    section = next((view for view in described.get("namespaces", []) if view.get("ns") == "storage-paths"), None)
    if section is None:
        raise AssertionError("storage-paths settings namespace is missing")
    require_ok(rpc(port, "settings.mutate", {"ns": "storage-paths", "expectedRevision": section["revision"], "ops": [
        {"op": "set", "path": ["dataDirectory"], "value": str(destination)}
    ]}, sequence), "settings.mutate storage-paths")
    sequence += 1
    response = runtime_request(port, {"action": "restart"})
    if response.get("restarting") is not True:
        raise AssertionError(f"runtime restart was not accepted: {response!r}")
    deadline = time.monotonic() + 75
    current = None
    while time.monotonic() < deadline:
        time.sleep(0.25)
        try:
            state = runtime_request(port)
        except RuntimeStatusError as error:
            if error.status in (502, 503, 504):
                continue
            raise
        except (OSError, ValueError, http.client.HTTPException):
            continue
        if state.get("migrationError"):
            raise AssertionError(f"runtime data migration failed: {state['migrationError']}")
        if state.get("instanceId") != instance:
            current = state
            break
    if current is None:
        raise AssertionError("runtime restart never produced a different instanceId")
    assert_runtime_home(current, destination)
    if not set(links).issubset(managed_profile_links(destination)):
        raise AssertionError("migration did not recreate installer-managed package links in the new home")
    if session_manifest(home / "sessions") != sessions_before or workspace_manifest(home) != storages_before:
        raise AssertionError("migration changed the original session/workspace files")
    if session_manifest(destination / "sessions") != sessions_before or workspace_manifest(destination) != storages_before:
        raise AssertionError("migrated session/workspace files do not match their original fingerprints")
    if marker.read_bytes() != (destination / marker.name).read_bytes():
        raise AssertionError("migration did not preserve the data marker at both locations")
    active_environment = pathlib.Path(current["paths"]["environmentDirectory"])
    if not os.path.samefile(active_environment, destination / "environments"):
        raise AssertionError("the default runtime environment did not move with the data directory")
    if environment_marker.read_bytes() != (active_environment / environment_marker.name).read_bytes():
        raise AssertionError("migration did not preserve the runtime environment marker")
    for workspace_id, session_id in fixtures:
        sequence = assert_visible(port, workspace_id, session_id, sequence)
    # A post-migration write must land only in the new home, then survive a
    # subsequent cold boot whose DSH_HOME still names the original directory.
    workspace_id = fixtures[0][0]
    created = require_ok(rpc(port, "session.create", {"workspaceId": workspace_id}, sequence), "session.create after migration")
    sequence += 1
    session_id = created.get("sessionId")
    if not isinstance(session_id, str) or not session_id:
        raise AssertionError("post-migration session.create returned no sessionId")
    fixtures.append((workspace_id, session_id))
    sequence = assert_visible(port, workspace_id, session_id, sequence)
    wait_for_session_manifest(destination / "sessions", len(fixtures))
    if session_manifest(home / "sessions") != sessions_before or workspace_manifest(home) != storages_before:
        raise AssertionError("post-migration writes still changed the original home")
    return sequence, {"runtime_instance_changed": True, "data_home_migrated": True,
                      "original_home_preserved": True, "runtime_environment_migrated": True,
                      "new_home_writes_verified": True, "managed_profile_link_count": len(links),
                      "managed_profile_link_paths": links}


def temporary_directory(stack: contextlib.ExitStack, prefix: str) -> pathlib.Path:
    parent = pathlib.Path(tempfile.gettempdir()).resolve()
    directory = pathlib.Path(stack.enter_context(tempfile.TemporaryDirectory(prefix=prefix, dir=parent))).resolve()
    if directory.parent != parent or not directory.name.startswith(prefix):
        raise AssertionError("temporary directory escaped its intended parent")
    return directory


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--repo", default=str(pathlib.Path(__file__).resolve().parents[1]))
    parser.add_argument("--relocate", action="store_true", help="Restart from a copied package against the same data directory")
    parser.add_argument("--migrate-home", action="store_true", help="Migrate data through the settings/restart API, then cold boot from the original DSH_HOME")
    args = parser.parse_args()
    binary = pathlib.Path(args.binary).resolve()
    repo = pathlib.Path(args.repo).resolve()
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}")
    if args.migrate_home and not (
        (binary.parent / "PACKAGE.json").is_file()
        and (binary.parent / "web" / "dist").is_dir()
        and (binary.parent / "config").is_dir()
    ):
        raise SystemExit("--migrate-home requires a real portable package containing PACKAGE.json, web/dist, and config")

    with contextlib.ExitStack() as stack:
        temporary = temporary_directory(stack, "dsh-settings-data-e2e-")
        home = pathlib.Path(temporary)
        active_home = home
        migration_evidence: dict[str, object] = {}
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

            if args.migrate_home:
                target_parent = temporary_directory(stack, "dsh-migrated-home-e2e-")
                destination = target_parent / "data"
                destination.mkdir()
                sequence, migration_evidence = migrate_home(port, home, destination, fixtures, sequence, sessions_before, storages_before)
                active_home = destination
                sessions_expected = wait_for_session_manifest(active_home / "sessions", len(fixtures))
                storages_expected = workspace_manifest(active_home)
            else:
                sessions_expected = sessions_before
                storages_expected = storages_before

        restart_binary, restart_repo = binary, repo
        if args.relocate:
            relocated = temporary_directory(stack, "dsh-relocated-package-") / "package"
            shutil.copytree(binary.parent, relocated)
            restart_binary, restart_repo = relocated / binary.name, relocated
        with running_host(restart_binary, restart_repo, home) as restarted_port:
            if args.migrate_home:
                assert_runtime_home(runtime_request(restarted_port), active_home)
                migration_evidence["cold_restart_from_original_home"] = True
                if args.relocate:
                    manifest_path = next((path for path in (restart_binary.parent / "package.json", restart_binary.parent / "PACKAGE.json") if path.is_file()), None)
                    manifest = json.loads(manifest_path.read_text(encoding="utf-8-sig")) if manifest_path else {}
                    package_name = manifest.get("name")
                    if not isinstance(package_name, str) or not re.fullmatch(r"(?:@[a-z0-9._-]+/)?[a-z0-9._-]+", package_name, re.IGNORECASE):
                        raise AssertionError("relocated package has no usable package name for its managed module link")
                    package_link = active_home / "profiles" / "node_modules" / package_name
                    if package_link.relative_to(active_home).as_posix() in migration_evidence["managed_profile_link_paths"]:
                        if not package_link.exists() or not os.path.samefile(package_link, restart_binary.parent):
                            raise AssertionError("cold restart did not retarget the migrated package link to the relocated installation")
                        migration_evidence["relocated_package_link_retargeted"] = True
            for workspace_id, session_id in fixtures:
                sequence = assert_visible(restarted_port, workspace_id, session_id, sequence)
            if session_manifest(active_home / "sessions") != sessions_expected:
                raise AssertionError("same-home restart changed durable session files")
            storages_after_restart = workspace_manifest(active_home)
            if storages_after_restart != storages_expected:
                changed = {
                    path: {"before": storages_expected.get(path), "after": storages_after_restart.get(path)}
                    for path in sorted(set(storages_expected) | set(storages_after_restart))
                    if storages_expected.get(path) != storages_after_restart.get(path)
                }
                raise AssertionError(
                    "same-home restart changed storage files: " + json.dumps(changed, sort_keys=True)
                )
            if args.migrate_home and (session_manifest(home / "sessions") != sessions_before or workspace_manifest(home) != storages_before):
                raise AssertionError("cold restart changed the preserved original home")

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
                    **migration_evidence,
                },
                sort_keys=True,
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
