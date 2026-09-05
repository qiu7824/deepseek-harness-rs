"""Verify automatic failure capture, recovery, cross-model reuse, and current guards."""
from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc

MODELS = [{"id": name, "contextWindow": 32768, "maxTokens": 4096} for name in ("model-a", "model-b")]


def session_manifest(home):
    result = {}
    for path in sorted((home / "sessions").rglob("*")):
        if not path.is_file() or path.name not in ("session.jsonl", "session.jsonl.zstd"):
            continue
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(block)
        result[path.relative_to(home).as_posix()] = digest.hexdigest()
    return result


class LearningFixture(BaseHTTPRequestHandler):
    records: list[dict] = []
    request_count: int = 0
    workspace: str = ""

    def log_message(self, *_args):
        pass

    def send_json(self, value, content_type="application/json", status=200):
        body = (value if isinstance(value, str) else json.dumps(value)).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.send_json({"data": MODELS})

    def do_POST(self):
        LearningFixture.request_count += 1
        request = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        messages = request.get("messages", [])
        serialized = json.dumps(messages, ensure_ascii=False)
        phase = next((name for name in ("learn", "reuse", "repeat", "disabled", "other", "masteroff", "providererror", "reload") if f"learning-e2e:{name}" in serialized), "auxiliary")
        exposed = {tool.get("function", {}).get("name") for tool in request.get("tools", [])}
        results = [message for message in messages if message.get("role") == "tool"]
        delta = {"role": "assistant", "content": "Learning verification complete."}
        finish = "stop"
        if "glob" in exposed:
            self.records.append({"phase": phase, "model": request.get("model"), "context": serialized, "toolResults": results})
            if phase == "providererror":
                self.send_json({"error": {"message": "fixture account rejection"}}, status=401)
                return
            if (phase == "learn" and len(results) < 2) or (phase == "repeat" and not results):
                arguments = {"pattern": "*.txt", "path": self.workspace}
                if not results:
                    arguments["unexpected_argument"] = "must be rejected before the tool body"
                delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": "call-" + uuid.uuid4().hex, "type": "function", "function": {"name": "glob", "arguments": json.dumps(arguments)}}]}
                finish = "tool_calls"
        events = [
            {"id": "fixture-" + uuid.uuid4().hex, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": {"prompt_tokens": 128, "completion_tokens": 64, "total_tokens": 192}},
        ]
        self.send_json("".join("data: " + json.dumps(event) + "\n\n" for event in events) + "data: [DONE]\n\n", "text/event-stream")


class Client:
    def __init__(self, port):
        self.port, self.sequence = port, 0

    def call(self, method, payload):
        self.sequence += 1
        return require_ok(rpc(self.port, method, payload, self.sequence), method)

    def wait_ledger(self, predicate, label):
        deadline = time.monotonic() + 15
        last = None
        while time.monotonic() < deadline:
            last = self.call("memory.learningList", {"limit": 200})
            if predicate(last):
                return last
            time.sleep(0.05)
        raise AssertionError(f"{label}: {last!r}")

    def turn(self, workspace_id, model, phase, expected_reason="completed"):
        session = self.call("session.create", {"workspaceId": workspace_id, "agentPreset": "standard"})["sessionId"]
        self.call("session.selectModel", {"sessionId": session, "provider": "learning-fixture", "model": model})
        self.call("session.prompt", {"sessionId": session, "content": [{"type": "text", "text": "learning-e2e:" + phase}], "mode": "queue"})
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            history = self.call("session.history", {"sessionId": session})
            events = [item["event"] for item in history["events"]]
            ends = [event for event in events if event["type"] == "turn/end"]
            if ends:
                assert ends[-1]["data"]["reason"]["kind"] == expected_reason, ends[-1]
                return session, events
            time.sleep(0.05)
        raise AssertionError(f"{phase}: model turn did not finish")


def model_context(phase):
    records = [row for row in LearningFixture.records if row["phase"] == phase]
    assert records, f"no actual model request for {phase}"
    return records[0]["context"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    args = parser.parse_args()
    binary, workdir = args.binary.resolve(), args.workdir.resolve()
    workdir.mkdir(parents=True, exist_ok=True)
    home = workdir / "ui-home"
    if home.exists():
        raise ValueError("choose a fresh isolated workdir")
    env = isolated_environment(workdir, home)
    workspace = workdir / "project"
    workspace.mkdir()
    (workspace / "evidence.txt").write_text("fixture evidence", encoding="utf-8")
    LearningFixture.workspace = str(workspace)
    server = ThreadingHTTPServer(("127.0.0.1", 0), LearningFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {"llm-pi-ai": {"providers": {"learning-fixture": {"keyless": True, "api": "openai-completions", "baseURL": f"http://127.0.0.1:{server.server_port}/v1", "models": MODELS}}}, "agent-default-model": {"provider": "learning-fixture", "model": "model-a"}}
    (home / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
    report = {}
    try:
        with running_fixture_host(binary, workdir, env, None, "learning") as port:
            client = Client(port)
            workspace_id = client.call("workspace.create", {"path": str(workspace)})["workspace"]["workspaceId"]
            first_session, first_events = client.turn(workspace_id, "model-a", "learn")
            ledger = client.wait_ledger(lambda value: any(row["status"] == "verified" for row in value["items"]), "automatic recovery")
            lesson = next(row for row in ledger["items"] if row["status"] == "verified")
            assert lesson["code"] == "TOOL_INPUT_INVALID" and lesson["verification"] == "recovered", lesson
            assert lesson["models"][0]["model"] == "model-a", lesson
            assert lesson["id"] not in model_context("learn"), "a failure cannot be learned before it occurs"
            client.turn(workspace_id, "model-b", "reuse")
            assert lesson["id"] in model_context("reuse"), "new model must receive the verified lesson"
            _, repeat_events = client.turn(workspace_id, "model-b", "repeat")
            assert any(event["type"] == "tool/result" and "TOOL_INPUT_INVALID" in json.dumps(event) for event in repeat_events), "current guard must still block a repeated invalid call"
            ledger = client.wait_ledger(lambda value: any(row["id"] == lesson["id"] and row["occurrences"] == 2 for row in value["items"]), "repeat deduplication")
            lesson = next(row for row in ledger["items"] if row["id"] == lesson["id"])
            assert {row["model"] for row in lesson["models"]} == {"model-a", "model-b"}, lesson
            assert lesson["applicationCount"] >= 1 and lesson["lastApplicationOutcome"] == "preflight_blocked", lesson
            client.call("memory.learningToggle", {"id": lesson["id"], "enabled": False, "expectedRevision": lesson["revision"]})
            client.turn(workspace_id, "model-b", "disabled")
            assert lesson["id"] not in model_context("disabled")
            lesson = next(row for row in client.call("memory.learningList", {})["items"] if row["id"] == lesson["id"])
            client.call("memory.learningToggle", {"id": lesson["id"], "enabled": True, "expectedRevision": lesson["revision"]})
            other = workdir / "other-project"
            other.mkdir()
            other_id = client.call("workspace.create", {"path": str(other)})["workspace"]["workspaceId"]
            client.turn(other_id, "model-b", "other")
            assert lesson["id"] not in model_context("other")
            memory = next(row for row in client.call("settings.describe", {})["namespaces"] if row["ns"] == "memory")
            client.call("settings.mutate", {"ns": "memory", "ops": [{"op": "set", "path": ["enabled"], "value": False}], "expectedRevision": memory["revision"]})
            client.wait_ledger(lambda value: value["memoryEnabled"] is False and value["effectiveEnabled"] is False, "master memory disabled")
            client.turn(workspace_id, "model-b", "masteroff")
            assert lesson["id"] not in model_context("masteroff")
            memory = next(row for row in client.call("settings.describe", {})["namespaces"] if row["ns"] == "memory")
            client.call("settings.mutate", {"ns": "memory", "ops": [{"op": "set", "path": ["enabled"], "value": True}], "expectedRevision": memory["revision"]})
            client.wait_ledger(lambda value: value["effectiveEnabled"] is True, "master memory restored")
            client.turn(workspace_id, "model-b", "providererror", expected_reason="error")
            ledger = client.wait_ledger(lambda value: any(row["source"] == "provider" for row in value["items"]), "provider failure attribution")
            provider_failure = next(row for row in ledger["items"] if row["source"] == "provider")
            assert provider_failure["model"] == "model-b" and provider_failure["code"] == "AUTH", provider_failure
            answer = next(event["data"]["message"]["id"] for event in reversed(first_events) if event["type"] == "assistant/message")
            feedback = client.call("messageFeedback.put", {"sessionId": first_session, "messageId": answer, "rating": "negative", "note": "private feedback text", "ifVersion": None})
            assert feedback["ok"] is True, feedback
            ledger = client.wait_ledger(lambda value: any(row["source"] == "feedback" for row in value["items"]), "negative feedback observation")
            candidate = next(row for row in ledger["items"] if row["source"] == "feedback")
            assert candidate["status"] == "pending" and candidate["suggestion"] == "", candidate
            assert "private feedback text" not in json.dumps(ledger)
            report.update({"lesson": lesson, "providerFailure": provider_failure, "feedbackCandidate": candidate, "crossModelReuse": True, "repeatedCallBlocked": True, "disabledExcluded": True, "masterSwitchRespected": True, "workspaceIsolated": True})
        with running_fixture_host(binary, workdir, env, None, "learning-reload") as port:
            client = Client(port)
            # A history preview must work before any session is resumed or any
            # model request is made after this cold start.
            files_before = session_manifest(home)
            assert files_before, "the cold-preview fixture must contain persisted sessions"
            requests_before = LearningFixture.request_count
            ledger_before = client.call("memory.learningList", {})
            applications_before = {row["id"]: row["applicationCount"] for row in ledger_before["items"]}
            preview = client.call("memory.learningPreview", {"sessionId": first_session})
            assert preview["sessionSource"] == "persisted", preview
            assert preview["toolSource"] == "last-request" and preview["modelSource"] in ("stored-selection", "last-request"), preview
            assert preview["mode"] == "historical-context-preview" and preview["notice"], preview
            assert any(row["id"] == lesson["id"] for row in preview["items"]), preview
            assert preview["model"] == "model-a", "preview must use this historical session's own selection"
            again = client.call("memory.learningPreview", {"sessionId": first_session})
            assert again["items"] == preview["items"], "revision-cached polling must preserve the same evidence"
            assert LearningFixture.request_count == requests_before, "read-only preview must not call a model"
            assert session_manifest(home) == files_before, "read-only preview must not append/recover session logs"
            applications_after = {row["id"]: row["applicationCount"] for row in client.call("memory.learningList", {})["items"]}
            assert applications_after == applications_before, "preview is not an application of a lesson"
            report["coldStartHistoricalPreview"] = True
            report["coldPreviewReadOnly"] = True
            client.turn(workspace_id, "model-b", "reload")
            assert lesson["id"] in model_context("reload"), "verified lessons must survive a cold start"
            report["coldStartReuse"] = True
        (workdir / "learning-loop-evidence.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print("automatic learning, cross-model reuse, live guard, feedback and cold-start checks passed")
        return 0
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    raise SystemExit(main())
