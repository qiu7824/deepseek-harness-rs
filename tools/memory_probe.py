"""Read-only memory probes for the formal Windows DSH host."""

from __future__ import annotations

import hashlib
import ntpath
import re
import subprocess
from collections.abc import Callable


_LISTENER_ROW = re.compile(
    r"^\s*TCP\s+127\.0\.0\.1:(?P<port>\d+)\s+\S+\s+LISTENING\s+(?P<pid>\d+)\s*$",
    re.IGNORECASE,
)

_PROCESS_FIELDS = {
    "WorkingSetSize": "working_set_bytes",
    "PrivatePageCount": "private_bytes",
    "ThreadCount": "threads",
    "HandleCount": "handles",
}


def _parse_wmic_blocks(output: str) -> list[dict[str, str]]:
    blocks: list[dict[str, str]] = []
    current: dict[str, str] = {}
    for raw in output.replace("\r", "").splitlines():
        line = raw.strip()
        if not line or "=" not in line:
            continue
        key, value = line.split("=", 1)
        if key in current:
            blocks.append(current)
            current = {}
        current[key] = value
    if current:
        blocks.append(current)
    return blocks


def parse_process_snapshot(
    host_output: str, children_output: str, *, expected_pid: int
) -> dict[str, object]:
    host_blocks = _parse_wmic_blocks(host_output)
    host = next(
        (block for block in host_blocks if block.get("ProcessId") == str(expected_pid)),
        None,
    )
    if host is None:
        raise RuntimeError(f"process metrics did not contain expected PID {expected_pid}")

    snapshot: dict[str, object] = {"pid": expected_pid}
    missing: list[str] = []
    for source, target in _PROCESS_FIELDS.items():
        value = host.get(source, "")
        if not value.isdigit():
            missing.append(target)
        else:
            snapshot[target] = int(value)
    if missing:
        raise RuntimeError(f"missing process metrics: {', '.join(sorted(missing))}")

    children: dict[str, int] = {}
    for block in _parse_wmic_blocks(children_output):
        if block.get("ParentProcessId") != str(expected_pid):
            continue
        name = block.get("Name", "").strip().lower()
        if name:
            children[name] = children.get(name, 0) + 1
    snapshot["children"] = children
    return snapshot


_REQUIRED_SNAPSHOT_FIELDS = {
    "pid",
    "working_set_bytes",
    "private_bytes",
    "threads",
    "handles",
    "children",
}


def build_record(
    *,
    label: str,
    snapshot: dict[str, object],
    binary_bytes: bytes,
    home_path: str,
    timestamp: str,
) -> dict[str, object]:
    missing = sorted(_REQUIRED_SNAPSHOT_FIELDS.difference(snapshot))
    if missing:
        raise RuntimeError(f"missing snapshot fields: {', '.join(missing)}")
    return {
        "label": label,
        "timestamp": timestamp,
        **snapshot,
        "binary_sha256": hashlib.sha256(binary_bytes).hexdigest(),
        "home_path_sha256": hashlib.sha256(home_path.encode("utf-8")).hexdigest(),
    }


def parse_executable_path(output: str, *, expected_pid: int) -> str:
    blocks = _parse_wmic_blocks(output)
    block = next(
        (entry for entry in blocks if entry.get("ProcessId") == str(expected_pid)),
        None,
    )
    if block is None:
        raise RuntimeError(f"executable identity did not contain expected PID {expected_pid}")
    path = block.get("ExecutablePath", "").strip()
    if not path:
        raise RuntimeError(f"executable path is unavailable for PID {expected_pid}")
    return path


def assert_running_binary(running_path: str, expected_path: str) -> None:
    running = ntpath.normcase(ntpath.normpath(running_path))
    expected = ntpath.normcase(ntpath.normpath(expected_path))
    if running != expected:
        raise RuntimeError("running listener binary does not match explicit --binary")


def collect_executable_path(
    *, pid: int, runner: Callable[[list[str]], str] = None
) -> str:
    run = runner or _run_text
    output = run(
        [
            "wmic",
            "process",
            "where",
            f"ProcessId={pid}",
            "get",
            "ProcessId,ExecutablePath",
            "/format:list",
        ]
    )
    return parse_executable_path(output, expected_pid=pid)


def _run_text(argv: list[str]) -> str:
    return subprocess.run(
        argv,
        check=True,
        capture_output=True,
        text=True,
        errors="replace",
    ).stdout


def collect_snapshot(
    *, port: int, runner: Callable[[list[str]], str] = _run_text
) -> dict[str, object]:
    netstat = runner(["netstat", "-ano", "-p", "tcp"])
    pid = parse_listener_pid(netstat, port)
    host = runner(
        [
            "wmic",
            "process",
            "where",
            f"ProcessId={pid}",
            "get",
            "ProcessId,WorkingSetSize,PrivatePageCount,ThreadCount,HandleCount",
            "/format:list",
        ]
    )
    children = runner(
        [
            "wmic",
            "process",
            "where",
            f"ParentProcessId={pid}",
            "get",
            "ProcessId,ParentProcessId,Name",
            "/format:list",
        ]
    )
    return parse_process_snapshot(host, children, expected_pid=pid)


def parse_listener_pid(netstat_output: str, port: int) -> int:
    """Return the one PID listening on the exact loopback TCP port."""
    pids = {
        int(match.group("pid"))
        for line in netstat_output.splitlines()
        if (match := _LISTENER_ROW.match(line)) and int(match.group("port")) == port
    }
    if not pids:
        raise RuntimeError(f"no exact loopback listener on 127.0.0.1:{port}")
    if len(pids) != 1:
        rendered = ", ".join(str(pid) for pid in sorted(pids))
        raise RuntimeError(f"multiple listener PIDs on 127.0.0.1:{port}: {rendered}")
    return next(iter(pids))
