"""Verify child task progress, terminal outcomes, parent result delivery, and skill cards."""
from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc

MODELS = [{"id": name, "contextWindow": 65536, "maxTokens": 2048} for name in ("parent", "child-ok", "child-error")]
LABELS = {"success": "独立模块检查", "failure": "失败分支检查", "parent-running": "运行中回流检查", "interrupt": "中止分支检查", "archive-delete": "归档清理检查"}


class Fixture(BaseHTTPRequestHandler):
    records = []
    phase = "success"
    started = threading.Event()
    release = threading.Event()
    parent_started = threading.Event()
    parent_release = threading.Event()
    issued = set()
    parent_waited = set()

    def log_message(self, *_args):
        pass

    def response(self, value, status=200):
        body = json.dumps(value).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.response({"data": MODELS})

    def event(self, delta, finish=None):
        data = {"id": "fixture-" + uuid.uuid4().hex, "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
        if finish:
            data["usage"] = {"prompt_tokens": 128, "completion_tokens": 64, "total_tokens": 192}
        self.wfile.write(("data: " + json.dumps(data, ensure_ascii=False) + "\n\n").encode("utf-8"))
        self.wfile.flush()

    def do_POST(self):
        try:
            self.model_response()
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            # A deliberate child interruption closes its streaming HTTP request.
            pass

    def model_response(self):
        request = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        Fixture.records.append(request)
        model = request.get("model")
        markers = re.findall(r"progress-e2e:([a-z-]+)", json.dumps(request.get("messages", [])))
        phase = markers[-1] if markers else Fixture.phase
        if model == "child-error":
            Fixture.started.set()
            Fixture.release.wait(20)
            self.response({"error": {"message": "isolated child rejection"}}, 401)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        if model == "child-ok":
            self.event({"role": "assistant", "content": "正在检查独立模块。"})
            Fixture.started.set()
            Fixture.release.wait(20)
            self.event({"content": "独立模块验证完成。"})
        elif model == "parent" and request.get("tools") and phase not in Fixture.issued:
            Fixture.issued.add(phase)
            if phase == "skill":
                name, arguments = "skill", {"name": "progress-fixture-skill"}
            else:
                name, arguments = "subagent", {"description": LABELS[phase], "prompt": "Inspect the isolated fixture.", "provider": "spawn", "model": "child-error" if phase == "failure" else "child-ok"}
            self.event({"role": "assistant", "tool_calls": [{"index": 0, "id": "call-" + uuid.uuid4().hex, "type": "function", "function": {"name": name, "arguments": json.dumps(arguments, ensure_ascii=False)}}]})
            self.event({}, "tool_calls")
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            return
        else:
            if model == "parent" and request.get("tools") and phase == "parent-running" and phase not in Fixture.parent_waited:
                Fixture.parent_waited.add(phase)
                self.event({"role": "assistant", "content": "父任务仍在执行。"})
                Fixture.parent_started.set()
                Fixture.parent_release.wait(20)
            self.event({"role": "assistant", "content": "主任务已收到执行结果。"})
        self.event({}, "stop")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


class Client:
    def __init__(self, port):
        self.port, self.sequence = port, 0

    def call(self, method, payload):
        self.sequence += 1
        return require_ok(rpc(self.port, method, payload, self.sequence), method)

    def until(self, read, predicate, label, timeout=25):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = read()
            if predicate(value):
                return value
            time.sleep(0.05)
        raise AssertionError(label + ": " + repr(value)[:800])


def turn_ends(history):
    return [item["event"] for item in history["events"] if item["event"]["type"] == "turn/end"]


def settled_notices(history, child_id, include_inbox=False):
    notices = []
    for item in history["events"]:
        event = item["event"]
        candidates = [event.get("data", {})] if event["type"] == "user/message" else []
        if include_inbox and event["type"] == "agent/inbox/spliced":
            candidates += event.get("data", {}).get("inserted", [])
        for message in candidates:
            source = message.get("source", {})
            if source.get("kind") == "subagent-settled" and source.get("senderSessionId") == child_id:
                notices.append(message)
    return notices


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    parser.add_argument("--node-modules", type=pathlib.Path)
    parser.add_argument("--observe-only", action="store_true", help="Record missing result delivery without reporting a passed Host check.")
    args = parser.parse_args()
    binary, workdir = args.binary.resolve(), args.workdir.resolve()
    workdir.mkdir(parents=True, exist_ok=True)
    home = workdir / "ui-home"
    if home.exists():
        raise ValueError("choose a fresh fixture directory")
    env = isolated_environment(workdir, home)
    workspace = workdir / "project"
    workspace.mkdir()
    server = ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {"llm-pi-ai": {"providers": {"progress-fixture": {"keyless": True, "api": "openai-completions", "baseURL": "http://127.0.0.1:" + str(server.server_port) + "/v1", "models": MODELS}}}, "agent-default-model": {"provider": "progress-fixture", "model": "parent"}}
    (home / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
    evidence = {"binary": str(binary), "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "phases": {}, "passed": False, "observeOnly": args.observe_only}
    try:
        with running_fixture_host(binary, workdir, env, args.node_modules, "subagent-progress") as port:
            client = Client(port)
            workspace_id = client.call("workspace.create", {"path": str(workspace)})["workspace"]["workspaceId"]
            client.call("capabilities.skillSave", {"name": "progress-fixture-skill", "content": "---\nname: progress-fixture-skill\ndescription: Inspect the progress fixture.\n---\n\n# Progress inspection\n\nCheck the isolated fixture output.\n"})
            for phase in ("success", "failure", "skill", "parent-running", "interrupt", "archive-delete"):
                Fixture.phase = phase
                Fixture.started.clear()
                Fixture.release.clear()
                Fixture.parent_started.clear()
                Fixture.parent_release.clear()
                attached_before = client.until(lambda: client.call("host.describe", {}), lambda value: value["attachedSessions"] == 0, "previous fixture agents must retire", timeout=5)["attachedSessions"]
                session = client.call("session.create", {"workspaceId": workspace_id, "agentPreset": "standard"})["sessionId"]
                client.call("session.prompt", {"sessionId": session, "content": [{"type": "text", "text": "progress-e2e:" + phase}], "mode": "queue"})
                row = {"sessionId": session, "attachedBefore": attached_before}
                evidence["phases"][phase] = row
                if phase != "skill":
                    assert Fixture.started.wait(20), "child model never started"
                    catalog = client.until(lambda: client.call("subagent.list", {"parentSessionId": session}), lambda value: any(item.get("activity") == "running" for item in value["entries"]), "running child catalog")
                    child = next(item for item in catalog["entries"] if item.get("activity") == "running")
                    address = {"parentSessionId": session, "childSessionId": child["id"], "mode": child["mode"]}
                    row.update({"address": address, "runningCatalog": catalog, "runningHistory": client.call("subagent.history", {**address, "maxMessages": 8})})
                    assert child["label"] == LABELS[phase]
                    if phase == "success":
                        row["runningHistory"] = client.until(lambda: client.call("subagent.history", {**address, "maxMessages": 8}), lambda value: "正在检查独立模块" in json.dumps(value, ensure_ascii=False), "visible streaming child update")
                    if phase == "parent-running":
                        assert Fixture.parent_started.wait(5), "the parent model must remain in a live request"
                        row["parentBeforeChildEnd"] = client.call("session.history", {"sessionId": session})
                        assert not turn_ends(row["parentBeforeChildEnd"]), "the parent must still be running when the child completes"
                        Fixture.release.set()
                        row["parentRunningDeliveryHistory"] = client.until(lambda: client.call("session.history", {"sessionId": session}), lambda value: bool(settled_notices(value, child["id"], True)), "delivery to a running parent's inbox", timeout=5)
                        assert not turn_ends(row["parentRunningDeliveryHistory"]), "parent completion cannot stand in for in-flight delivery"
                        assert "独立模块验证完成" in json.dumps(settled_notices(row["parentRunningDeliveryHistory"], child["id"], True), ensure_ascii=False)
                        Fixture.parent_release.set()
                    else:
                        row["parentBeforeChildEnd"] = client.until(lambda: client.call("session.history", {"sessionId": session}), lambda value: bool(turn_ends(value)), "parent must finish before releasing the child", timeout=5)
                        row["attachedDuringChild"] = client.call("host.describe", {})["attachedSessions"]
                        assert row["attachedDuringChild"] >= 2, "the idle parent must remain attached while its child runs"
                        if phase == "interrupt":
                            row["interruptReceipt"] = client.call("subagent.interrupt", address)
                            assert row["interruptReceipt"]["accepted"] is True
                            row["interruptedHistory"] = client.until(lambda: client.call("subagent.history", {**address, "maxMessages": 8}), lambda value: bool(turn_ends(value)), "interrupted child must settle promptly", timeout=5)
                            assert turn_ends(row["interruptedHistory"])[-1]["data"]["reason"]["kind"] == "aborted", "an interrupted child must never report completed"
                            Fixture.release.set()
                        elif phase == "archive-delete":
                            row["archiveReceipt"] = client.call("workspace.archiveSession", {"sessionId": session})
                            assert session in row["archiveReceipt"]["archivedSessionIds"]
                            row["deleteReceipt"] = client.call("workspace.deleteArchivedSession", {"sessionId": session})
                            assert row["deleteReceipt"]["deleted"] is True
                            row["hostAfterDelete"] = client.until(lambda: client.call("host.describe", {}), lambda value: value["attachedSessions"] <= attached_before, "permanent deletion must release the parent and child agents", timeout=5)
                            row["sessionsAfterDelete"] = client.call("session.list", {})
                            assert all(not item.get("running") for item in row["sessionsAfterDelete"]["items"] if item["sessionId"] in (session, child["id"])), "a deleted parent's child cannot remain running"
                            assert all(item["sessionId"] != session for item in row["sessionsAfterDelete"]["items"]), "the parent must be durably deleted"
                            requests_before_release = len(Fixture.records)
                            Fixture.release.set()
                            time.sleep(0.15)
                            row["requestsAfterCleanupRelease"] = len(Fixture.records) - requests_before_release
                            assert row["requestsAfterCleanupRelease"] == 0, "closing a deleted child stream must not restart the removed task"
                            row["descendantsDrained"] = True
                            continue
                        else:
                            Fixture.release.set()
                history = client.until(lambda: client.call("session.history", {"sessionId": session}), lambda value: any(item["event"]["type"] == "turn/end" for item in value["events"]), "parent completion")
                row["parentHistory"] = history
                results = [item["event"] for item in history["events"] if item["event"]["type"] == "tool/result"]
                assert len(results) == 1, results
                result = results[0]["data"]["message"]["content"][0]
                assert result["isError"] is False, result
                if phase != "skill":
                    row["settledCatalog"] = client.until(lambda: client.call("subagent.list", {"parentSessionId": session}), lambda value: value["entries"][0]["activity"] == "inactive", "child settlement")
                    row["settledHistory"] = client.call("subagent.history", {**row["address"], "maxMessages": 8})
                    assert row["settledCatalog"]["entries"][0]["activity"] == "inactive"
                    end = next(item["event"] for item in reversed(row["settledHistory"]["events"]) if item["event"]["type"] == "turn/end")
                    expected_reason = "error" if phase == "failure" else "aborted" if phase == "interrupt" else "completed"
                    assert end["data"]["reason"]["kind"] == expected_reason, end
                    if phase != "parent-running":
                        assert turn_ends(row["parentBeforeChildEnd"])[-1]["time"] <= end["time"], "the child must settle after the parent turn finished"
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline:
                        row["parentHistory"] = client.call("session.history", {"sessionId": session})
                        notices = settled_notices(row["parentHistory"], child["id"])
                        if notices:
                            break
                        time.sleep(0.05)
                    row["parentResultDelivered"] = bool(notices) and (phase in ("failure", "interrupt") or "独立模块验证完成" in json.dumps(notices, ensure_ascii=False))
                    row["settledNoticeCount"] = len(notices)
                    assert len(notices) <= 1, "a child activation must not duplicate its terminal notice"
                    assert row["address"]["childSessionId"] in json.dumps(result), "returned child id must prove the conversation link"
                    row["summary"] = client.call("session.list", {})
                else:
                    assert "<skill_content" in json.dumps(result), "skill call must retain its expandable body"
            evidence["passed"] = all(row.get("parentResultDelivered", True) for row in evidence["phases"].values())
    finally:
        Fixture.release.set()
        Fixture.parent_release.set()
        server.shutdown()
        server.server_close()
        (workdir / "subagent-progress-evidence.json").write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")
        (workdir / "subagent-model-requests.json").write_text(json.dumps(Fixture.records, ensure_ascii=False, indent=2), encoding="utf-8")
    if args.node_modules:
        harness = pathlib.Path(__file__).parent / "tests" / "subagent_progress_dom_harness.cjs"
        completed = subprocess.run(["node", str(harness), str(args.node_modules.resolve()), str(workdir)], capture_output=True, text=True, encoding="utf-8", errors="replace", env=env, timeout=60, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        (workdir / "subagent-progress-dom.log").write_text(completed.stdout + completed.stderr, encoding="utf-8")
        assert completed.returncode == 0, completed.stdout + completed.stderr
        print(completed.stdout.strip())
    if not evidence["passed"]:
        print("OBSERVED FAILURE: child result delivery is missing; running, completion, failure and skill evidence were captured")
        return 0 if args.observe_only else 1
    print("PASS real Host: running child update, completion, failure, parent result delivery, and skill disclosure evidence")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
