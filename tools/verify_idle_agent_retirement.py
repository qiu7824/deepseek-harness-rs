"""One-shot production E2E for exact idle Agent retirement.

Creates and deletes only its own synthetic session. It never reads response text.
"""
from __future__ import annotations

import argparse
import json
import time
import urllib.request
import uuid

from memory_probe import collect_snapshot


def rpc(base: str, method: str, payload: dict) -> dict:
    request = {
        "type": "client-request",
        "rpcId": str(uuid.uuid4()),
        "method": method,
        "payload": payload,
    }
    raw = urllib.request.urlopen(
        urllib.request.Request(
            f"{base}/api/{method}",
            data=json.dumps(request).encode(),
            headers={"Content-Type": "application/json"},
        ),
        timeout=30,
    ).read()
    response = json.loads(raw)
    result = response.get("result")
    if not isinstance(result, dict) or result.get("ok") is not True:
        raise RuntimeError(f"{method} failed")
    return result["value"]


def wait_running(base: str, session_id: str, expected: bool, timeout: float = 180.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        items = rpc(base, "session.list", {}).get("items", [])
        current = next((item for item in items if item.get("sessionId") == session_id), None)
        if current is not None and bool(current.get("running")) is expected:
            return
        time.sleep(0.25)
    raise TimeoutError(f"session running did not become {expected}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="http://127.0.0.1:58080")
    parser.add_argument("--cwd", required=True)
    args = parser.parse_args()
    session_id = f"agent-session-memory-retire-e2e-{uuid.uuid4()}"
    observations: dict = {"session_id_sha256": __import__("hashlib").sha256(session_id.encode()).hexdigest()}
    try:
        created = rpc(args.base, "session.create", {"sessionId": session_id, "cwd": args.cwd})
        if created.get("sessionId") != session_id:
            raise RuntimeError("session.create returned wrong identity")
        for turn in range(2):
            accepted = rpc(
                args.base,
                "session.prompt",
                {
                    "sessionId": session_id,
                    "mode": "queue",
                    "content": [{"type": "text", "text": "只回复 OK"}],
                    "clientTimeZone": "Asia/Shanghai",
                },
            )
            if accepted.get("accepted") is not True:
                raise RuntimeError("prompt was not accepted")
            wait_running(args.base, session_id, True)
            wait_running(args.base, session_id, False)
            time.sleep(5)
            observations[f"after_turn_{turn + 1}"] = collect_snapshot(port=58080)
        rpc(args.base, "workspace.archiveSession", {"sessionId": session_id})
        rpc(args.base, "workspace.deleteArchivedSession", {"sessionId": session_id})
        items = rpc(args.base, "session.list", {}).get("items", [])
        if any(item.get("sessionId") == session_id for item in items):
            raise RuntimeError("temporary session survived deletion")
        observations["deleted"] = True
        print(json.dumps(observations, separators=(",", ":")))
        return 0
    except Exception:
        try:
            rpc(args.base, "workspace.archiveSession", {"sessionId": session_id})
            rpc(args.base, "workspace.deleteArchivedSession", {"sessionId": session_id})
        except Exception:
            pass
        raise


if __name__ == "__main__":
    raise SystemExit(main())
