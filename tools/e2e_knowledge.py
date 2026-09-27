"""Verify local knowledge bases: management route, document extraction, search, the chat tool and restart persistence."""
from __future__ import annotations

import argparse
import base64
import hashlib
import http.client
import io
import json
import pathlib
import shutil
import sys
import tempfile
import threading
import time
import uuid
import zipfile
from http.server import BaseHTTPRequestHandler

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc
from e2e_http import ThreadingHTTPServer

CHAT_REQUEST = "knowledge-e2e: 服务出问题了怎么回滚？"
CONTEXT_NOTE = "local knowledge bases enabled for search"


class ModelFixture(BaseHTTPRequestHandler):
    """OpenAI-compatible fixture: answers the chat question with knowledge_search."""

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
        self.send({"data": [{"id": "knowledge-fixture", "contextWindow": 32768, "maxTokens": 4096}]})

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        messages = request.get("messages", [])
        tools = [tool.get("function", {}).get("name") for tool in request.get("tools", [])]
        text = json.dumps(messages, ensure_ascii=False)
        asked = next((index for index, message in enumerate(messages) if message.get("role") == "user" and CHAT_REQUEST in json.dumps(message, ensure_ascii=False)), None)
        results = [message for message in messages[(asked or 0) + 1:] if message.get("role") == "tool"] if asked is not None else []
        ModelFixture.requests.append({"tools": tools, "note": CONTEXT_NOTE in text, "toolResults": [json.dumps(message.get("content"), ensure_ascii=False) for message in results]})
        if asked is not None and not results and "knowledge_search" in tools:
            arguments = {"query": "回滚步骤"}
            delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": "call-" + uuid.uuid4().hex, "type": "function", "function": {"name": "knowledge_search", "arguments": json.dumps(arguments, ensure_ascii=False)}}]}
            finish = "tool_calls"
        else:
            delta = {"role": "assistant", "content": "根据《运维手册》，先停止服务再恢复上一版本。"}
            finish = "stop"
        events = [{"id": "fixture-" + uuid.uuid4().hex, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]}, {"choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": {"prompt_tokens": 64, "completion_tokens": 16, "total_tokens": 80}}]
        self.send("".join("data: " + json.dumps(event, ensure_ascii=False) + "\n\n" for event in events) + "data: [DONE]\n\n", "text/event-stream")


def knowledge(port: int, operation: str, payload: dict[str, object], expect: int = 200) -> dict[str, object]:
    body = json.dumps(payload).encode("utf-8")
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
    try:
        connection.request(
            "POST",
            f"/__dsh-knowledge/{operation}",
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


def docx(paragraphs: list[str]) -> bytes:
    body = "".join(f"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>" for text in paragraphs)
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("[Content_Types].xml", "<Types/>")
        archive.writestr("word/document.xml", f'<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>')
    return buffer.getvalue()


def encoded(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=pathlib.Path)
    parser.add_argument("--workdir", type=pathlib.Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    work = args.workdir or pathlib.Path(tempfile.mkdtemp(prefix="dsh-knowledge-e2e-"))
    if work.exists():
        shutil.rmtree(work)
    home, project, folder = work / "home", work / "project", work / "import"
    project.mkdir(parents=True)
    (folder / "notes").mkdir(parents=True)
    (folder / "notes" / "值班.txt").write_text("值班表：周一张三，周二李四。", encoding="utf-8")
    (folder / "logo.png").write_bytes(b"\x89PNG")
    env = isolated_environment(work, home)
    server = ThreadingHTTPServer(("127.0.0.1", 0), ModelFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {"llm-pi-ai": {"providers": {"knowledge-fixture": {"keyless": True, "api": "openai-completions", "baseURL": f"http://127.0.0.1:{server.server_port}/v1", "models": [{"id": "knowledge-fixture", "contextWindow": 32768, "maxTokens": 4096}]}}}, "agent-default-model": {"provider": "knowledge-fixture", "model": "knowledge-fixture"}}
    (home / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
    counter = 0
    evidence: dict[str, object] = {"binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "passed": False}
    state: dict[str, str] = {}
    try:
        for phase in ("manage", "restart"):
            with running_fixture_host(binary, work, env, None, "knowledge-" + phase) as port:
                def call(method: str, payload: dict[str, object]) -> dict[str, object]:
                    nonlocal counter
                    counter += 1
                    return require_ok(rpc(port, method, payload, counter), method)

                if phase == "manage":
                    catalog = knowledge(port, "catalog", {})
                    assert catalog["bases"] == [] and "pdf" in catalog["extensions"] and "docx" in catalog["extensions"], catalog
                    knowledge(port, "create", {"name": "  "}, 400)
                    base = knowledge(port, "create", {"name": "运维手册", "description": "部署与回滚"})["base"]
                    state["base"] = base["id"]
                    knowledge(port, "create", {"name": "运维手册"}, 409)
                    markdown = "# 回滚\n\n回滚步骤：先停止服务，再恢复上一版本，最后检查健康状态。".encode("utf-8")
                    md = knowledge(port, "upload", {"id": base["id"], "name": "回滚.md", "data": encoded(markdown)})["document"]
                    assert md["chunkCount"] == 1 and md["bytes"] == len(markdown), md
                    knowledge(port, "upload", {"id": base["id"], "name": "副本.md", "data": encoded(markdown)}, 409)
                    knowledge(port, "upload", {"id": base["id"], "name": "图片.png", "data": encoded(b"\x89PNG")}, 400)
                    word = knowledge(port, "upload", {"id": base["id"], "name": "发布.docx", "data": encoded(docx(["发布流程：先构建安装包。", "然后灰度 10% 用户。"]))})["document"]
                    state["word"] = word["id"]
                    knowledge(port, "importPath", {"id": base["id"], "path": "relative/path"}, 400)
                    report = knowledge(port, "importPath", {"id": base["id"], "path": str(folder)})["report"]
                    assert [doc["name"] for doc in report["added"]] == ["值班.txt"] and report["skipped"] == [], report
                    documents = knowledge(port, "documents", {"id": base["id"]})["documents"]
                    assert sorted(doc["name"] for doc in documents) == ["值班.txt", "发布.docx", "回滚.md"], documents
                    hits = knowledge(port, "search", {"query": "灰度发布"})["results"]
                    assert hits and hits[0]["documentName"] == "发布.docx" and "灰度" in hits[0]["text"], hits
                    assert knowledge(port, "search", {"query": "李四"})["results"][0]["documentName"] == "值班.txt"
                    # Chat: the runtime context names the base and the model searches it.
                    chat = call("session.create", {"cwd": str(project), "agentPreset": "standard"})["sessionId"]
                    call("session.prompt", {"sessionId": chat, "mode": "queue", "requestId": "knowledge-chat-1", "content": [{"type": "text", "text": CHAT_REQUEST}]})
                    deadline = time.monotonic() + 30
                    answered = None
                    while time.monotonic() < deadline and answered is None:
                        time.sleep(0.3)
                        answered = next((request for request in ModelFixture.requests if request["toolResults"]), None)
                    assert answered is not None, "chat did not search; model requests: " + json.dumps(ModelFixture.requests, ensure_ascii=False)[:3000]
                    assert "knowledge_search" in answered["tools"] and answered["note"], answered
                    assert "回滚.md" in answered["toolResults"][0] and "先停止服务" in answered["toolResults"][0], answered
                    evidence["chat"] = {"contextNote": answered["note"], "toolResult": answered["toolResults"][0][:200]}
                    disabled = knowledge(port, "update", {"id": base["id"], "enabled": False})["base"]
                    assert disabled["enabled"] is False and disabled["documentCount"] == 3, disabled
                    assert knowledge(port, "search", {"query": "回滚"})["results"] == []
                    knowledge(port, "update", {"id": base["id"], "enabled": True, "name": "运维与发布"})
                    knowledge(port, "deleteDocument", {"id": word["id"]})
                    knowledge(port, "deleteDocument", {"id": word["id"]}, 404)
                    evidence["documents"] = sorted(doc["name"] for doc in documents)
                else:
                    assert (home / "knowledge" / "knowledge.sqlite").is_file()
                    bases = knowledge(port, "catalog", {})["bases"]
                    assert [(item["name"], item["enabled"], item["documentCount"]) for item in bases] == [("运维与发布", True, 2)], bases
                    assert knowledge(port, "search", {"query": "回滚步骤"})["results"][0]["documentName"] == "回滚.md"
                    assert knowledge(port, "search", {"query": "灰度"})["results"] == [], "deleted document stays out of the index"
                    knowledge(port, "delete", {"id": state["base"]})
                    knowledge(port, "documents", {"id": state["base"]}, 404)
                    assert knowledge(port, "catalog", {})["bases"] == []
        evidence["passed"] = True
        print(json.dumps(evidence))
        return 0
    finally:
        if args.workdir is None:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
