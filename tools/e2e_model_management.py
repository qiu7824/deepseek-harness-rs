"""Verify packaged model settings through React DOM, live RPC, and a Host restart."""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import pathlib
import queue
import re
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from collections.abc import Iterator

sys.dont_write_bytecode = True
from e2e_settings_model_preserves_data import read_lines, require_ok, rpc

MODELS = [
    {"id": "fixture-one", "name": "Fixture One", "contextWindow": 32768, "maxTokens": 4096},
    {"id": "fixture-two", "name": "Fixture Two", "contextWindow": 65536, "maxTokens": 8192},
    {"id": "fixture-hidden", "name": "Fixture Hidden", "enabled": False, "contextWindow": 32768, "maxTokens": 4096},
]


class ModelFixture(BaseHTTPRequestHandler):
    def log_message(self, *_args: object) -> None:
        pass

    def do_GET(self) -> None:
        self.respond({"data": MODELS}, "application/json")

    def do_POST(self) -> None:
        request = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        if request.get("model") not in {row["id"] for row in MODELS}:
            self.send_error(400, "unknown fixture model")
            return
        text = "\n\n".join(f"记录段落 {index + 1}：隔离会话正文保持完整。" for index in range(30))
        events = [
            {"id": "fixture-" + uuid.uuid4().hex, "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": None}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 128, "completion_tokens": 512, "total_tokens": 640}},
        ]
        stream = "".join("data: " + json.dumps(event, ensure_ascii=False) + "\n\n" for event in events) + "data: [DONE]\n\n"
        self.respond(stream, "text/event-stream")

    def respond(self, value: object, content_type: str) -> None:
        text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
        body = text.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()


def isolated_environment(workdir: pathlib.Path, home: pathlib.Path) -> dict[str, str]:
    user = workdir / "ui-user"
    paths = {
        "DSH_HOME": home,
        "HOME": user,
        "USERPROFILE": user,
        "APPDATA": user / "AppData/Roaming",
        "LOCALAPPDATA": user / "AppData/Local",
        "CLAUDE_CONFIG_DIR": user / "claude",
        "TEMP": workdir / "ui-temp",
        "TMP": workdir / "ui-temp",
        "TMPDIR": workdir / "ui-temp",
        "XDG_CONFIG_HOME": user / ".config",
        "XDG_CACHE_HOME": user / ".cache",
        "XDG_DATA_HOME": user / ".local/share",
    }
    for path in paths.values():
        path.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    for key in list(env):
        if key in {"DSH_INSTALL_ANCHOR", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "DEEPSEEK_API_KEY"} or key.startswith("DSH_OAUTH_"):
            env.pop(key, None)
    env.update({key: str(path) for key, path in paths.items()})
    return env


@contextlib.contextmanager
def running_fixture_host(binary: pathlib.Path, workdir: pathlib.Path, env: dict[str, str], node_modules: pathlib.Path | None, phase: str) -> Iterator[int]:
    process = subprocess.Popen(
        [str(binary), "web", "--port", "0"], cwd=workdir, env=env,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace",
        creationflags=getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0) | getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    lines: queue.Queue[str] = queue.Queue()
    assert process.stdout is not None and process.stderr is not None
    threading.Thread(target=read_lines, args=(process.stdout, lines), daemon=True).start()
    threading.Thread(target=read_lines, args=(process.stderr, lines), daemon=True).start()
    observed: list[str] = []
    try:
        deadline = time.monotonic() + 45
        port = None
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise AssertionError(f"fixture Host exited with {process.returncode}: {observed[-8:]!r}")
            try:
                line = lines.get(timeout=0.2)
            except queue.Empty:
                continue
            observed.append(line)
            matched = re.fullmatch(r"dsh web: http://127\.0\.0\.1:(\d+)", line)
            if matched:
                port = int(matched.group(1))
                break
        if port is None:
            raise AssertionError(f"fixture Host did not become ready: {observed[-8:]!r}")
        owned = {"helperPid": os.getpid(), "hostPid": process.pid, "home": env["DSH_HOME"], "port": port, "binary": str(binary)}
        if node_modules is not None:
            owned["nodeModules"] = str(node_modules)
        (workdir / "ui-owned-process.json").write_text(json.dumps(owned, indent=2), encoding="utf-8")
        yield port
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
        for _ in range(10000):
            try:
                observed.append(lines.get_nowait())
            except queue.Empty:
                break
        (workdir / f"model-management-host-{phase}.log").write_text("\n".join(observed), encoding="utf-8")


def seed_sessions(port: int, workdir: pathlib.Path) -> None:
    sequence = 0
    def call(method: str, payload: dict[str, object]) -> dict[str, object]:
        nonlocal sequence
        sequence += 1
        return require_ok(rpc(port, method, payload, sequence), method)
    workspace_dir = workdir / "ui-workspace"
    workspace_dir.mkdir()
    workspace = call("workspace.create", {"path": str(workspace_dir)})["workspace"]
    sessions = []
    for index in range(3):
        session_id = call("session.create", {"workspaceId": workspace["workspaceId"]})["sessionId"]
        call("session.rename", {"sessionId": session_id, "title": f"Model continuity {index + 1}"})
        for turn in range(5 if index == 0 else 1):
            call("session.prompt", {"sessionId": session_id, "content": [{"type": "text", "text": f"隔离验证消息 {turn + 1}"}], "mode": "queue"})
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                history = call("session.history", {"sessionId": session_id})
                events = [entry["event"] for entry in history["events"]]
                if sum(event["type"] == "turn/end" for event in events) > turn:
                    assert any(event["type"] == "assistant/message" and "隔离会话正文" in json.dumps(event, ensure_ascii=False) for event in events), "fixture must produce real assistant history"
                    break
                time.sleep(0.05)
            else:
                raise AssertionError("fixture model turn did not finish")
        sessions.append(session_id)
    (workdir / "ui-fixtures.json").write_text(json.dumps({"workspaceId": workspace["workspaceId"], "sessionIds": sessions}, indent=2), encoding="utf-8")


def run_dom_phase(repo: pathlib.Path, workdir: pathlib.Path, env: dict[str, str], phase: str) -> None:
    completed = subprocess.run(
        ["node", str(repo / "tools/tests/model_management_rpc_dom_harness.cjs"), str(workdir), phase],
        cwd=repo, env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    (workdir / f"model-management-dom-{phase}.log").write_text(completed.stdout + completed.stderr, encoding="utf-8")
    if completed.returncode:
        raise AssertionError(f"{phase} React/RPC verification failed: {completed.stdout[-2000:]} {completed.stderr[-2000:]}")
    print(completed.stdout.strip(), flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--repo", type=pathlib.Path, required=True)
    parser.add_argument("--node-modules", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    args = parser.parse_args()
    binary, repo, node_modules, workdir = [path.resolve() for path in (args.binary, args.repo, args.node_modules, args.workdir)]
    if not binary.is_file() or not (repo / "tools/tests/model_management_rpc_dom_harness.cjs").is_file():
        raise ValueError("binary or repository test harness is missing")
    if not all((node_modules / package / "package.json").is_file() for package in ("react", "react-dom", "jsdom")):
        raise ValueError("node-modules must contain React, React DOM, and jsdom")
    workdir.mkdir(parents=True, exist_ok=True)
    if (workdir / "ui-home").exists() or (workdir / "ui-fixtures.json").exists():
        raise ValueError("workdir already contains a fixture; choose a fresh directory")
    home = workdir / "ui-home"
    env = isolated_environment(workdir, home)
    server = ThreadingHTTPServer(("127.0.0.1", 0), ModelFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {"llm-pi-ai": {"providers": {"ui-fixture": {"displayName": "UI Fixture", "keyless": True, "api": "openai-completions", "baseURL": f"http://127.0.0.1:{server.server_port}/v1", "models": MODELS}}}, "agent-default-model": {"provider": "ui-fixture", "model": "fixture-one"}}
    (home / "settings.json").write_text(json.dumps(settings, ensure_ascii=False), encoding="utf-8")
    try:
        with running_fixture_host(binary, workdir, env, node_modules, "save") as port:
            seed_sessions(port, workdir)
            run_dom_phase(repo, workdir, env, "save")
        with running_fixture_host(binary, workdir, env, node_modules, "reload"):
            run_dom_phase(repo, workdir, env, "reload")
        evidence_path = workdir / "model-management-rpc-evidence.json"
        evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
        evidence["binarySha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        evidence["binary"] = str(binary)
        evidence_path.write_text(json.dumps(evidence, indent=2), encoding="utf-8")
        print(f"PASS packaged model management: 3 sessions, saved visibility, directory refresh, hidden current model, restart, unchanged durable history. Evidence: {evidence_path}")
        return 0
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    raise SystemExit(main())
