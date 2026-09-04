"""Verify repeated cold resumes deliver every accepted user message."""

from __future__ import annotations

import argparse
import json
import pathlib
import queue
import tempfile
import threading
import time

from e2e_image_capability import CaptureHandler, CaptureServer
from e2e_settings_model_preserves_data import require_ok, rpc, running_host


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--repo", default=str(pathlib.Path(__file__).resolve().parents[1]))
    parser.add_argument("--turns", type=int, default=8)
    args = parser.parse_args()
    if not 3 <= args.turns <= 100:
        parser.error("--turns must be between 3 and 100")
    binary = pathlib.Path(args.binary).resolve()
    repo = pathlib.Path(args.repo).resolve()
    provider = CaptureServer(("127.0.0.1", 0), CaptureHandler)
    provider.capture = queue.Queue()
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    sequence = 0
    try:
        with tempfile.TemporaryDirectory(prefix="dsh-resume-e2e-") as temporary:
            home = pathlib.Path(temporary)
            with running_host(binary, repo, home) as port:
                def call(method: str, payload: dict[str, object]) -> dict[str, object]:
                    nonlocal sequence
                    sequence += 1
                    return require_ok(rpc(port, method, payload, sequence), method)

                call("settings.mutate", {"ns": "llm-pi-ai", "ops": [{
                    "op": "set", "path": ["providers", "resume-fixture"], "value": {
                        "keyless": True, "api": "openai-responses",
                        "baseURL": f"http://127.0.0.1:{provider.server_port}/v1",
                        "models": [{"id": "resume-model", "input": ["text"]}],
                    },
                }]})
                session_id = call("session.create", {"cwd": str(home)})["sessionId"]
                call("session.selectModel", {"sessionId": session_id, "provider": "resume-fixture", "model": "resume-model"})
                last_end = -1
                for turn in range(args.turns):
                    accepted = call("session.prompt", {
                        "sessionId": session_id, "mode": "queue", "clientTimeZone": "UTC",
                        "content": [{"type": "text", "text": f"Resume turn {turn + 1}."}],
                    })
                    if accepted.get("accepted") is not True:
                        raise AssertionError("prompt was not accepted")
                    deadline = time.monotonic() + 15
                    while time.monotonic() < deadline:
                        history = call("session.history", {"sessionId": session_id, "afterSeq": last_end + 1, "maxMessages": 64})
                        ended = [entry["event"]["seq"] for entry in history.get("events", []) if entry.get("event", {}).get("type") == "turn/end"]
                        if ended and max(ended) > last_end:
                            last_end = max(ended)
                            break
                        time.sleep(0.025)
                    else:
                        raise AssertionError(f"accepted turn {turn + 1} did not finish after idle retirement")
                    # Allow the owner to complete idle disposal before the
                    # next request, exercising repeated cold resume cycles.
                    time.sleep(0.05)
            with running_host(binary, repo, home) as port:
                sequence += 1
                history = require_ok(rpc(port, "session.history", {"sessionId": session_id, "maxMessages": 1000}, sequence), "session.history")
                events = [entry["event"] for entry in history.get("events", [])]
                completed = sum(event.get("type") == "turn/end" for event in events)
                serialized = json.dumps(events)
                if completed != args.turns:
                    raise AssertionError(f"durable completed turns: {completed}, expected {args.turns}")
                for turn in range(args.turns):
                    if f"Resume turn {turn + 1}." not in serialized:
                        raise AssertionError(f"accepted message {turn + 1} was not persisted")
            print(json.dumps({"accepted_turns": args.turns, "durable_completed_turns": completed, "same_home_restart": True}, sort_keys=True))
    finally:
        provider.shutdown()
        provider.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
