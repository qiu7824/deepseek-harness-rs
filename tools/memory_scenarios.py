"""Read-only production memory workload scenarios."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys
import time
import urllib.request
from collections.abc import Callable

try:
    from tools.memory_probe import (
        assert_running_binary,
        build_record,
        collect_executable_path,
        collect_snapshot,
    )
except ModuleNotFoundError:
    from memory_probe import (
        assert_running_binary,
        build_record,
        collect_executable_path,
        collect_snapshot,
    )


READ_ONLY_RPC_METHODS = frozenset(
    {
        "session.list",
        "session.history",
        "subagent.list",
        "subagent.history",
        "settings.describe",
        "session.models",
    }
)

Transport = Callable[[str, dict[str, object]], tuple[bytes, float]]


def _http_rpc(url: str, body: dict[str, object]) -> tuple[bytes, float]:
    encoded = json.dumps(body, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=encoded,
        headers={"Content-Type": "application/json"},
    )
    started = time.perf_counter()
    with urllib.request.urlopen(request, timeout=120) as response:
        payload = response.read()
    return payload, time.perf_counter() - started


def _decode_rpc_response(method: str, expected_rpc_id: str, response: bytes) -> object:
    try:
        envelope = json.loads(response)
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise RuntimeError(f"{method} returned invalid JSON") from error
    result = envelope.get("result")
    if isinstance(result, dict) and result.get("ok") is False:
        message = result.get("error", {}).get("message", "unknown RPC error")
        raise RuntimeError(f"{method} failed: {message}")
    if (
        envelope.get("type") != "server-response"
        or envelope.get("rpcId") != expected_rpc_id
        or not isinstance(result, dict)
        or result.get("ok") is not True
        or "value" not in result
    ):
        raise RuntimeError(f"{method} returned invalid RPC response")
    return result["value"]


def _request_body(method: str, payload: dict[str, object], index: int) -> dict[str, object]:
    return {
        "type": "client-request",
        "rpcId": f"memory-probe-{index}",
        "method": method,
        "payload": payload,
    }


def run_rpc_scenario(
    *,
    base_url: str,
    method: str,
    payload: dict[str, object],
    repetitions: int,
    transport: Transport = _http_rpc,
) -> dict[str, object]:
    if method not in READ_ONLY_RPC_METHODS:
        raise ValueError(f"method is not in the read-only RPC allowlist: {method}")
    if repetitions <= 0:
        raise ValueError("repetitions must be positive")

    response_bytes = 0
    elapsed_seconds = 0.0
    url = f"{base_url.rstrip('/')}/api/{method}"
    for index in range(repetitions):
        body = _request_body(method, payload, index)
        response, elapsed = transport(url, body)
        response_bytes += len(response)
        elapsed_seconds += elapsed
        _decode_rpc_response(method, str(body["rpcId"]), response)

    return {
        "method": method,
        "requests": repetitions,
        "response_bytes": response_bytes,
        "elapsed_seconds": elapsed_seconds,
    }


def preflight_rpc(
    *,
    base_url: str,
    method: str,
    payload: dict[str, object],
    transport: Transport = _http_rpc,
) -> object:
    if method not in READ_ONLY_RPC_METHODS:
        raise ValueError(f"method is not in the read-only RPC allowlist: {method}")
    body = _request_body(method, payload, 0)
    url = f"{base_url.rstrip('/')}/api/{method}"
    response, _elapsed = transport(url, body)
    return _decode_rpc_response(method, str(body["rpcId"]), response)


def run_default_matrix(
    *,
    base_url: str,
    history_session_id: str,
    snapshotter: Callable[[str], dict[str, object]],
    transport: Transport = _http_rpc,
) -> dict[str, object]:
    preflight_rpc(
        base_url=base_url,
        method="session.list",
        payload={},
        transport=transport,
    )
    history_preflight = preflight_rpc(
        base_url=base_url,
        method="session.history",
        payload={"sessionId": history_session_id, "afterSeq": 30, "maxMessages": 8},
        transport=transport,
    )
    if not isinstance(history_preflight, dict) or history_preflight.get("hasMore") is not True:
        raise RuntimeError("history preflight requires hasMore=true")
    snapshots = [snapshotter("baseline")]
    workloads: list[dict[str, object]] = []
    matrix = [
        ("list_20", "session.list", {}, 20, 20),
        ("list_100", "session.list", {}, 80, 100),
        ("list_second_100", "session.list", {}, 100, 200),
        (
            "history_20",
            "session.history",
            {"sessionId": history_session_id, "afterSeq": 30, "maxMessages": 8},
            20,
            20,
        ),
        (
            "history_100",
            "session.history",
            {"sessionId": history_session_id, "afterSeq": 30, "maxMessages": 8},
            80,
            100,
        ),
        (
            "history_second_100",
            "session.history",
            {"sessionId": history_session_id, "afterSeq": 30, "maxMessages": 8},
            100,
            200,
        ),
    ]
    for label, method, payload, repetitions, cumulative in matrix:
        workloads.append(
            {
                "label": label,
                "cumulative_requests": cumulative,
                **run_rpc_scenario(
                    base_url=base_url,
                    method=method,
                    payload=payload,
                    repetitions=repetitions,
                    transport=transport,
                ),
            }
        )
        snapshots.append(snapshotter(label))
    return {"snapshots": snapshots, "workloads": workloads}


def render_jsonl_report(
    *, snapshots: list[dict[str, object]], workloads: list[dict[str, object]]
) -> str:
    records = [
        *(({"schema_version": 1, "type": "snapshot", **snapshot}) for snapshot in snapshots),
        *(({"schema_version": 1, "type": "workload", **workload}) for workload in workloads),
    ]
    return "\n".join(
        json.dumps(record, ensure_ascii=False, separators=(",", ":")) for record in records
    ) + "\n"


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run read-only DSH memory scenarios")
    parser.add_argument("--binary", required=True)
    parser.add_argument("--home", required=True)
    parser.add_argument("--history-session", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--port", type=int, default=58080)
    args = parser.parse_args(argv)
    if args.port != 58080:
        raise ValueError("formal production port is fixed at 58080")
    return args


def render_completion(
    output: pathlib.Path, *, report_bytes: bytes, snapshots: int
) -> str:
    return json.dumps(
        {
            "output_name_sha256": hashlib.sha256(output.name.encode("utf-8")).hexdigest(),
            "report_sha256": hashlib.sha256(report_bytes).hexdigest(),
            "snapshots": snapshots,
        },
        separators=(",", ":"),
    )


def make_snapshotter(
    *,
    port: int,
    binary_bytes: bytes,
    home_path: str,
    expected_pid: int,
    collect: Callable[[int], dict[str, object]],
    timestamp: Callable[[], str],
) -> Callable[[str], dict[str, object]]:
    def snapshotter(label: str) -> dict[str, object]:
        snapshot = collect(port)
        if snapshot.get("pid") != expected_pid:
            raise RuntimeError(
                f"listener PID changed during scenario: expected {expected_pid}, got {snapshot.get('pid')}"
            )
        return build_record(
            label=label,
            snapshot=snapshot,
            binary_bytes=binary_bytes,
            home_path=home_path,
            timestamp=timestamp(),
        )

    return snapshotter


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    binary = pathlib.Path(args.binary).resolve()
    output = pathlib.Path(args.output).resolve()
    binary_bytes = binary.read_bytes()
    first_snapshot = collect_snapshot(port=args.port)
    expected_pid = int(first_snapshot["pid"])
    running_binary = collect_executable_path(pid=expected_pid)
    assert_running_binary(running_binary, str(binary))
    snapshotter = make_snapshotter(
        port=args.port,
        binary_bytes=binary_bytes,
        home_path=args.home,
        expected_pid=expected_pid,
        collect=lambda port: collect_snapshot(port=port),
        timestamp=lambda: time.strftime("%Y-%m-%dT%H:%M:%S%z"),
    )

    result = run_default_matrix(
        base_url=f"http://127.0.0.1:{args.port}",
        history_session_id=args.history_session,
        snapshotter=snapshotter,
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    report_bytes = render_jsonl_report(**result).encode("utf-8")
    output.write_bytes(report_bytes)
    print(
        render_completion(
            output,
            report_bytes=report_bytes,
            snapshots=len(result["snapshots"]),
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
