"""Verify Host-owned scheduled tasks: management, chat creation, delivery into a session, restart persistence."""
from __future__ import annotations

import argparse
import datetime
import hashlib
import http.client
import json
import pathlib
import shutil
import sys
import tempfile
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc
from e2e_http import ThreadingHTTPServer

CHAT_REQUEST = "schedule-e2e: 每天早上 8:15 整理今天的待办并生成清单"


class ModelFixture(BaseHTTPRequestHandler):
    """OpenAI-compatible fixture: answers a chat scheduling request with scheduled_task_create."""

    requests: list[dict[str, object]] = []

    def log_message(self, *_args):
        pass

    def send(self, value, content_type="application/json"):
        body = (value if isinstance(value, str) else json.dumps(value)).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.send({"data": [{"id": "schedule-fixture", "contextWindow": 32768, "maxTokens": 4096}]})

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        messages = request.get("messages", [])
        tools = [tool.get("function", {}).get("name") for tool in request.get("tools", [])]
        ModelFixture.requests.append({"tools": tools})
        asked = next((index for index, message in enumerate(messages) if message.get("role") == "user" and CHAT_REQUEST in json.dumps(message, ensure_ascii=False)), None)
        answered = asked is not None and any(message.get("role") == "tool" for message in messages[asked + 1:])
        if asked is not None and not answered and "scheduled_task_create" in tools:
            arguments = {"title": "对话创建的待办任务", "prompt": "整理今天的待办并生成清单", "daily": {"time": "08:15", "time_zone": "Asia/Shanghai"}}
            delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": "call-" + uuid.uuid4().hex, "type": "function", "function": {"name": "scheduled_task_create", "arguments": json.dumps(arguments, ensure_ascii=False)}}]}
            finish = "tool_calls"
        else:
            delta = {"role": "assistant", "content": "已安排。"}
            finish = "stop"
        events = [{"id": "fixture-" + uuid.uuid4().hex, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]}, {"choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": {"prompt_tokens": 64, "completion_tokens": 16, "total_tokens": 80}}]
        self.send("".join("data: " + json.dumps(event, ensure_ascii=False) + "\n\n" for event in events) + "data: [DONE]\n\n", "text/event-stream")


def schedule(port: int, operation: str, payload: dict[str, object], expect: int = 200) -> dict[str, object]:
    body = json.dumps(payload).encode("utf-8")
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=40)
    try:
        connection.request(
            "POST",
            f"/__dsh-schedule/{operation}",
            body=body,
            headers={
                "content-type": "application/json",
                "origin": f"http://127.0.0.1:{port}",
                "sec-fetch-site": "same-origin",
            },
        )
        response = connection.getresponse()
        value = json.loads(response.read().decode("utf-8"))
    finally:
        connection.close()
    if response.status != expect:
        raise AssertionError(f"{operation}: HTTP {response.status} {value!r}")
    return value


def iso(delta_seconds: float) -> str:
    moment = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(seconds=delta_seconds)
    return moment.isoformat(timespec="milliseconds").replace("+00:00", "Z")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=pathlib.Path)
    parser.add_argument("--workdir", type=pathlib.Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    work = args.workdir or pathlib.Path(tempfile.mkdtemp(prefix="dsh-schedule-e2e-"))
    if work.exists():
        shutil.rmtree(work)
    home, project = work / "home", work / "project"
    project.mkdir(parents=True)
    env = isolated_environment(work, home)
    server = ThreadingHTTPServer(("127.0.0.1", 0), ModelFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {"llm-pi-ai": {"providers": {"schedule-fixture": {"keyless": True, "api": "openai-completions", "baseURL": f"http://127.0.0.1:{server.server_port}/v1", "models": [{"id": "schedule-fixture", "contextWindow": 32768, "maxTokens": 4096}]}}}, "agent-default-model": {"provider": "schedule-fixture", "model": "schedule-fixture"}}
    (home / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
    counter = 0
    evidence: dict[str, object] = {"binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "passed": False}
    state: dict[str, str] = {}
    try:
        for phase in ("deliver", "restart"):
            with running_fixture_host(binary, work, env, None, "schedule-" + phase) as port:
                def call(method: str, payload: dict[str, object]) -> dict[str, object]:
                    nonlocal counter
                    counter += 1
                    return require_ok(rpc(port, method, payload, counter), method)

                if phase == "deliver":
                    session = call("session.create", {"cwd": str(project), "agentPreset": "standard"})["sessionId"]
                    state["session"] = session
                    schedule(port, "create", {"sessionId": session, "prompt": "x", "rule": {"kind": "every", "everySeconds": 30}}, 400)
                    schedule(port, "create", {"sessionId": session, "prompt": "x", "rule": {"kind": "at", "at": iso(-60)}}, 400)
                    once = schedule(port, "create", {"sessionId": session, "title": "E2E 单次", "prompt": "定时任务端到端：请回复 OK", "rule": {"kind": "at", "at": iso(4)}})["task"]
                    daily = schedule(port, "create", {"sessionId": session, "prompt": "每天早上汇总", "rule": {"kind": "daily", "time": "09:00", "timeZone": "Asia/Shanghai"}})["task"]
                    state["once"], state["daily"] = once["id"], daily["id"]
                    catalog = schedule(port, "catalog", {})
                    assert catalog["error"] is None and len(catalog["tasks"]) == 2, catalog
                    assert isinstance(catalog["hostTimeZone"], str) and catalog["hostTimeZone"], catalog
                    revision = catalog["revision"]
                    deadline = time.monotonic() + 30
                    delivered = None
                    while time.monotonic() < deadline:
                        revision = schedule(port, "wait", {"revision": revision})["revision"]
                        rows = {task["id"]: task for task in schedule(port, "catalog", {"sessionId": session})["tasks"]}
                        if rows[once["id"]].get("lastDelivery"):
                            delivered = rows[once["id"]]
                            break
                    assert delivered is not None, "one-shot task was not delivered"
                    receipt = delivered["lastDelivery"]
                    assert receipt["outcome"] == "delivered" and receipt["messageId"], receipt
                    assert delivered["status"] == "inactive" and "nextRunAt" not in delivered, delivered
                    history = json.dumps(call("session.history", {"sessionId": session}), ensure_ascii=False)
                    assert "[SCHEDULED TASK]" in history and "定时任务端到端" in history, "delivered message missing from session history"
                    page = schedule(port, "history", {"id": once["id"], "sessionId": session})
                    assert page["total"] == 1 and page["records"][0]["prompt"] == "定时任务端到端：请回复 OK", page
                    updated = schedule(port, "update", {"id": daily["id"], "sessionId": session, "expectedUpdatedAt": daily["updatedAt"], "title": "晨报", "rule": {"kind": "weekly", "time": "08:30", "weekdays": [1, 3, 5], "timeZone": "Asia/Shanghai"}})["task"]
                    assert updated["title"] == "晨报" and updated["rule"]["kind"] == "weekly", updated
                    schedule(port, "update", {"id": daily["id"], "sessionId": session, "expectedUpdatedAt": daily["updatedAt"], "title": "stale"}, 409)
                    schedule(port, "update", {"id": daily["id"], "sessionId": "other-session", "title": "x"}, 404)
                    paused = schedule(port, "setActive", {"id": daily["id"], "sessionId": session, "active": False})["task"]
                    assert paused["status"] == "inactive", paused
                    # Chat: the user asks in the conversation and the agent calls scheduled_task_create.
                    chat = call("session.create", {"cwd": str(project), "agentPreset": "standard"})["sessionId"]
                    state["chat"] = chat
                    call("session.prompt", {"sessionId": chat, "mode": "queue", "requestId": "schedule-chat-1", "content": [{"type": "text", "text": CHAT_REQUEST}]})
                    deadline = time.monotonic() + 30
                    created = []
                    while time.monotonic() < deadline and not created:
                        time.sleep(0.3)
                        created = [task for task in schedule(port, "catalog", {"sessionId": chat})["tasks"] if task["origin"] == "agent"]
                    assert created, "chat did not create a task; model requests: " + json.dumps([request["tools"] for request in ModelFixture.requests], ensure_ascii=False)[:3000]
                    offered = set(next(request["tools"] for request in ModelFixture.requests if "scheduled_task_create" in request["tools"]))
                    assert {"scheduled_task_create", "scheduled_task_list", "scheduled_task_delete"} <= offered, offered
                    task = created[0]
                    assert task["title"] == "对话创建的待办任务" and task["rule"] == {"kind": "daily", "time": "08:15", "timeZone": "Asia/Shanghai"}, task
                    state["chatTask"] = task["id"]
                    evidence["chat"] = {"taskId": task["id"], "toolsOffered": sorted(name for name in offered if name.startswith("scheduled_task_"))}
                    evidence["delivery"] = {"messageId": receipt["messageId"], "occurrenceAt": receipt["occurrenceAt"]}
                else:
                    stored = json.loads((home / "schedule.json").read_text(encoding="utf-8"))
                    assert stored["version"] == 1 and len(stored["tasks"]) == 3, [task["id"] for task in stored["tasks"]]
                    rows = {task["id"]: task for task in schedule(port, "catalog", {})["tasks"]}
                    assert rows[state["chatTask"]]["origin"] == "agent" and rows[state["chatTask"]]["sessionId"] == state["chat"], rows
                    assert rows[state["daily"]]["status"] == "inactive" and rows[state["daily"]]["title"] == "晨报", rows
                    assert rows[state["once"]]["lastDelivery"]["outcome"] == "delivered", rows
                    resumed = schedule(port, "setActive", {"id": state["daily"], "sessionId": state["session"], "active": True})["task"]
                    assert resumed["status"] == "active" and resumed["nextRunAt"], resumed
                    schedule(port, "delete", {"id": state["daily"], "sessionId": state["session"]})
                    schedule(port, "history", {"id": state["daily"], "sessionId": state["session"]}, 404)
                    assert sorted(task["id"] for task in schedule(port, "catalog", {})["tasks"]) == sorted([state["once"], state["chatTask"]])
        evidence["passed"] = True
        print(json.dumps(evidence))
        return 0
    finally:
        if args.workdir is None:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
