"""Read-only verification of bidirectional session.history cursors."""

from __future__ import annotations

import argparse
import json
import urllib.request


def rpc(base_url: str, method: str, payload: dict[str, object], rpc_id: str) -> dict[str, object]:
    body = {"type": "client-request", "rpcId": rpc_id, "method": method, "payload": payload}
    request = urllib.request.Request(
        f"{base_url.rstrip('/')}/api/{method}",
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=120) as response:
        envelope = json.load(response)
    if envelope.get("type") != "server-response" or envelope.get("rpcId") != rpc_id:
        raise RuntimeError("invalid RPC response envelope")
    result = envelope.get("result")
    if not isinstance(result, dict) or result.get("ok") is not True:
        raise RuntimeError(f"{method} failed: {result}")
    value = result.get("value")
    if not isinstance(value, dict):
        raise RuntimeError("history value is not an object")
    return value


def verify(base_url: str, session_id: str, start_seq: int, pages: int) -> dict[str, object]:
    cursor = start_seq
    windows: list[dict[str, object]] = []
    seen: set[int] = set()
    for index in range(pages):
        value = rpc(
            base_url,
            "session.history",
            {"sessionId": session_id, "afterSeq": cursor, "maxMessages": 8},
            f"history-page-{index}",
        )
        for required in ("hasMoreBefore", "hasMoreAfter", "firstSeq", "lastSeq"):
            if required not in value:
                raise RuntimeError(f"history response is missing {required}")
        entries = value.get("events")
        if not isinstance(entries, list) or not entries:
            raise RuntimeError("history page is empty")
        seqs = [entry["event"]["seq"] for entry in entries]
        if seqs != sorted(seqs) or len(seqs) != len(set(seqs)):
            raise RuntimeError("history page is not strictly ordered and unique")
        if seen.intersection(seqs):
            raise RuntimeError("history pages overlap")
        last_event = entries[-1]["event"]
        covered_last = last_event.get("data", {}).get("__historyEndSeq", last_event["seq"])
        if value["firstSeq"] != seqs[0] or value["firstSeq"] != cursor:
            raise RuntimeError("history first cursor disagrees with events or request")
        if value["lastSeq"] != covered_last:
            raise RuntimeError("history last cursor disagrees with event coverage")
        seen.update(seqs)
        windows.append(
            {
                "firstSeq": value["firstSeq"],
                "lastSeq": value["lastSeq"],
                "hasMoreBefore": value["hasMoreBefore"],
                "hasMoreAfter": value["hasMoreAfter"],
                "events": len(seqs),
            }
        )
        if value["hasMoreAfter"] is not True:
            break
        cursor = int(value["lastSeq"]) + 1
    return {"windows": windows, "uniqueEvents": len(seen)}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="http://127.0.0.1:58080")
    parser.add_argument("--session", required=True)
    parser.add_argument("--start-seq", type=int, required=True)
    parser.add_argument("--pages", type=int, default=6)
    args = parser.parse_args()
    print(json.dumps(verify(args.base_url, args.session, args.start_seq, args.pages), separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
