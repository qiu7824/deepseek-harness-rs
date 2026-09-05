"""Exercise native approval waits with harmless files and the shipped React card."""
from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc


class ApprovalFixture(BaseHTTPRequestHandler):
    root: pathlib.Path
    records: list[dict] = []

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
        self.send({"data": [{"id": "approval-fixture", "contextWindow": 32768, "maxTokens": 4096}]})

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        messages = request.get("messages", [])
        choice = None
        for index, message in enumerate(messages):
            if message.get("role") != "user":
                continue
            content = message.get("content", "")
            if isinstance(content, list):
                content = "\n".join(part.get("text", "") for part in content)
            if "approval-e2e:" in content:
                choice = (index, json.JSONDecoder().raw_decode(content.split("approval-e2e:", 1)[1])[0])
        delta = {"role": "assistant", "content": "Approval fixture complete."}
        finish = "stop"
        if choice is not None:
            index, spec = choice
            target = (self.root / spec["path"]).resolve()
            if self.root.resolve() not in target.parents:
                raise ValueError("approval fixture target must remain under its isolated root")
            results = [row for row in messages[index + 1:] if row.get("role") == "tool"]
            self.records.append({"spec": spec, "results": results})
            required = 2 if spec["tool"] == "write" else 1
            if len(results) < required:
                # Observe before writing; only synthetic files can be addressed.
                tool = "read" if not results else "write"
                arguments = {"file_path": str(target)}
                if tool == "write":
                    arguments["content"] = "APPROVAL_FIXTURE_WRITTEN:" + spec["marker"]
                delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": "approval-call-" + uuid.uuid4().hex, "type": "function", "function": {"name": tool, "arguments": json.dumps(arguments)}}]}
                finish = "tool_calls"
        events = [{"id": "fixture-" + uuid.uuid4().hex, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]}, {"choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": {"prompt_tokens": 128, "completion_tokens": 64, "total_tokens": 192}}]
        self.send("".join("data: " + json.dumps(event) + "\n\n" for event in events) + "data: [DONE]\n\n", "text/event-stream")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--repo", type=pathlib.Path, required=True)
    parser.add_argument("--node-modules", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    parser.add_argument("--live", action="store_true", help="Keep only this isolated fixture running until approval-stop.request is created")
    args = parser.parse_args()
    binary, repo, modules, work = (value.resolve() for value in (args.binary, args.repo, args.node_modules, args.workdir))
    work.mkdir(parents=True, exist_ok=True)
    home = work / "ui-home"
    if home.exists():
        raise ValueError("choose a fresh fixture directory")
    env = isolated_environment(work, home)
    project = work / "project"
    project.mkdir()
    (project / ".env").write_text("APPROVAL_FIXTURE_ONLY=not-a-real-secret", encoding="utf-8")
    (work / "outside-one").mkdir()
    (work / "outside-two").mkdir()
    (work / "outside-one/中文审批验证.txt").write_text("APPROVAL_FIXTURE_BEFORE", encoding="utf-8")
    ApprovalFixture.root = work
    server = ThreadingHTTPServer(("127.0.0.1", 0), ApprovalFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {"llm-pi-ai": {"providers": {"approval-fixture": {"keyless": True, "api": "openai-completions", "baseURL": f"http://127.0.0.1:{server.server_port}/v1", "models": [{"id": "approval-fixture", "contextWindow": 32768, "maxTokens": 4096}]}}}, "agent-default-model": {"provider": "approval-fixture", "model": "approval-fixture"}}
    if args.live:
        settings["security"] = {"approvalTimeoutSeconds": 300}
    (home / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
    try:
        with running_fixture_host(binary, work, env, modules, "approvals") as port:
            workspace = require_ok(rpc(port, "workspace.create", {"path": str(project)}, 1), "workspace.create")["workspace"]
            sessions = []
            for index in range(2):
                session = require_ok(rpc(port, "session.create", {"workspaceId": workspace["workspaceId"], "agentPreset": "standard"}, 2 + index), "session.create")["sessionId"]
                sessions.append(session)
                require_ok(rpc(port, "session.rename", {"sessionId": session, "title": f"审批隔离验证 {index + 1}"}, 20 + index), "session.rename")
            fixture = {"workspaceId": workspace["workspaceId"], "sessionIds": sessions, "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
            (work / "approval-fixtures.json").write_text(json.dumps(fixture), encoding="utf-8")
            if args.live:
                payload = {"sessionId": sessions[0], "content": [{"type": "text", "text": "approval-e2e:" + json.dumps({"tool": "write", "path": "outside-one/中文审批验证.txt", "marker": "live-safe"})}], "mode": "queue"}
                require_ok(rpc(port, "session.prompt", payload, 30), "session.prompt")
                print(f"SAFE LIVE APPROVAL: http://127.0.0.1:{port}; session {sessions[0]}; root {work}", flush=True)
                while not (work / "approval-stop.request").exists():
                    time.sleep(0.2)
            else:
                completed = subprocess.run(["node", str(repo / "tools/tests/approval_rpc_dom_harness.cjs"), str(work)], cwd=repo, env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=180, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
                (work / "approval-dom.log").write_text(completed.stdout + completed.stderr, encoding="utf-8")
                if completed.returncode:
                    raise AssertionError(completed.stdout + completed.stderr)
                print(completed.stdout.strip(), flush=True)
    finally:
        (work / "approval-model-requests.json").write_text(json.dumps(ApprovalFixture.records, indent=2, ensure_ascii=False), encoding="utf-8")
        server.shutdown()
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
