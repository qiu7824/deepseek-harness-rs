from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import re
import signal
import sys
import subprocess
import tempfile
import threading
import time
import urllib.error
from pathlib import Path
import urllib.request
from datetime import datetime, timezone
from html.parser import HTMLParser
sys.path.insert(0, str(Path(__file__).resolve().parent))
from free_model_evidence import provider_for


DEFAULT_CATALOG_URL = "https://opencode.ai/zen/v1/models"
DEFAULT_MODEL_ID = "ling-3.0-flash-fin-free"
PRICING_URL = "https://opencode.ai/docs/zen/"


def open_with_retry(request, timeout):
    for attempt in range(3):
        try:
            return urllib.request.urlopen(request, timeout=timeout)
        except urllib.error.HTTPError as error:
            if error.code not in (429, 502, 503, 504) or attempt == 2:
                raise
            retry_after = error.headers.get("Retry-After", "")
            delay = min(15, max(2, int(retry_after))) if retry_after.isdigit() else 2 ** (attempt + 1)
        except (urllib.error.URLError, TimeoutError):
            if attempt == 2:
                raise
            delay = 2 ** (attempt + 1)
        time.sleep(delay)


class PricingTables(HTMLParser):
    def __init__(self):
        super().__init__()
        self.rows = []
        self.row = None
        self.cell = None

    def handle_starttag(self, tag, attrs):
        if tag == "tr":
            self.row = []
        elif tag in ("td", "th") and self.row is not None:
            self.cell = []

    def handle_data(self, data):
        if self.cell is not None:
            self.cell.append(data)

    def handle_endtag(self, tag):
        if tag in ("td", "th") and self.cell is not None:
            self.row.append(" ".join("".join(self.cell).split()))
            self.cell = None
        elif tag == "tr" and self.row is not None:
            self.rows.append(self.row)
            self.row = None


def pricing_catalog() -> dict:
    request = urllib.request.Request(PRICING_URL, headers={"User-Agent": "deepseek-harness-rs-release-verifier"})
    with open_with_retry(request, timeout=25) as response:
        html = response.read(4 * 1024 * 1024).decode("utf-8")
    parser = PricingTables()
    parser.feed(html)
    return pricing_catalog_from_rows(parser.rows)


def pricing_catalog_from_rows(rows: list) -> dict:
    prices = {row[0]: row for row in rows if len(row) >= 4 and all(value.lower() == "free" for value in row[1:4])}
    result = {}
    for row in rows:
        if len(row) < 3 or row[0] not in prices:
            continue
        api = {"https://opencode.ai/zen/v1/chat/completions": "openai-completions", "https://opencode.ai/zen/v1/responses": "openai-responses"}.get(row[2])
        if api is None:
            continue
        result[row[1]] = {"name": row[0], "api": api, "provider": provider_for(api), "freePricingVerified": True,
                          "pricingSource": PRICING_URL, "pricingLabel": row[0],
                          "pricingEvidence": {"modelId": row[1], "label": row[0], "prices": ["Free", "Free", "Free"], "endpoint": row[2]}}
    return result


def verify_free_pricing(model_id: str) -> dict:
    row = pricing_catalog().get(model_id)
    if row is None:
        raise ValueError("the official pricing table does not currently confirm this exact model is free")
    return row


def fetch_model_ids(url: str, timeout: float = 20.0) -> set[str]:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/json",
            "User-Agent": "deepseek-harness-rs-release-verifier",
        },
    )
    with open_with_retry(request, timeout=timeout) as response:
        payload = json.load(response)
    rows = payload.get("data")
    if not isinstance(rows, list):
        raise ValueError("model catalog response has no data list")
    ids = {
        row.get("id")
        for row in rows
        if isinstance(row, dict) and isinstance(row.get("id"), str)
    }
    if not ids:
        raise ValueError("model catalog response has no model ids")
    return ids


def streamed_completion(endpoint: str, body: dict, timeout: float) -> dict:
    request = urllib.request.Request(endpoint, data=json.dumps(body).encode("utf-8"), headers={
        "Accept": "text/event-stream", "Content-Type": "application/json",
        "User-Agent": "deepseek-harness-rs-release-verifier",
    })
    with open_with_retry(request, timeout=timeout) as response:
        message = {"role": "assistant", "content": ""}
        calls = {}
        finished = False
        received = 0
        for raw in response:
            received += len(raw)
            if received > 8 * 1024 * 1024:
                raise ValueError("free inference stream exceeded 8 MiB")
            line = raw.decode("utf-8").strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                finished = True
                break
            payload = json.loads(data)
            if payload.get("error"):
                failure = payload["error"]
                detail = str(failure.get("code") or failure.get("type") or failure.get("message") or "provider error") if isinstance(failure, dict) else str(failure)
                raise ValueError("free inference stream error: " + detail[:300])
            for choice in payload.get("choices") or []:
                delta = choice.get("delta") or {}
                if delta.get("content"):
                    message["content"] += delta["content"]
                for call in delta.get("tool_calls") or []:
                    target = calls.setdefault(call.get("index", 0), {"id": "", "type": "function", "function": {"name": "", "arguments": ""}})
                    if call.get("id"):
                        target["id"] = call["id"]
                    for key in ("name", "arguments"):
                        part = (call.get("function") or {}).get(key)
                        if isinstance(part, str):
                            target["function"][key] += part
                if choice.get("finish_reason"):
                    finished = True
        if not finished:
            raise ValueError("free inference stream ended without completion")
        if calls:
            message["tool_calls"] = [calls[index] for index in sorted(calls)]
        return message


def inference_probe(model_id: str, url: str, timeout: float = 90.0, api: str = "openai-completions") -> dict:
    if api == "openai-responses":
        return responses_probe(model_id, url, timeout)
    endpoint = url.rsplit("/", 1)[0] + "/chat/completions"
    body = {
        "model": model_id,
        "messages": [{"role": "user", "content": "Call the connectivity_check tool with status set to ok."}],
        "tools": [{"type": "function", "function": {
            "name": "connectivity_check", "description": "Confirm connection",
            "parameters": {"type": "object", "properties": {"status": {"type": "string", "enum": ["ok"]}}, "required": ["status"]}
        }}],
        "tool_choice": "auto", "max_tokens": 1024, "stream": True,
    }
    started = time.monotonic()
    message = streamed_completion(endpoint, body, timeout)
    calls = message.get("tool_calls") or []
    if not any(call.get("function", {}).get("name") == "connectivity_check" and
               json.loads(call["function"].get("arguments", "{}")) == {"status": "ok"}
               for call in calls):
        raise ValueError("free model did not return a valid tool call")
    body["messages"] += [message] + [{"role": "tool", "tool_call_id": call["id"], "content": "ok"} for call in calls]
    body["messages"].append({"role": "user", "content": "Reply with the single word OK."})
    body.pop("tools")
    body.pop("tool_choice")
    content = streamed_completion(endpoint, body, timeout).get("content")
    if not isinstance(content, str) or not content.strip() or "OK" not in content.upper():
        raise ValueError("free model did not complete the tool-result conversation")
    return {"inference": True, "streaming": True, "toolCall": True, "toolResult": True, "anonymous": True,
            "latencyMs": round((time.monotonic() - started) * 1000)}


def verify(model_id: str, url: str = DEFAULT_CATALOG_URL) -> dict:
    if url != DEFAULT_CATALOG_URL:
        raise ValueError("free verification is restricted to the official anonymous endpoint")
    ids = fetch_model_ids(url)
    if model_id not in ids:
        raise ValueError(f"free model {model_id!r} is absent from {url}")
    pricing = verify_free_pricing(model_id)
    return {"url": url, "model": model_id, "available": True,
            "verifiedAt": datetime.now(timezone.utc).isoformat(), **pricing, **inference_probe(model_id, url, api=pricing["api"])}


def responses_completion(endpoint: str, body: dict, timeout: float) -> dict:
    request = urllib.request.Request(endpoint, data=json.dumps(body).encode("utf-8"), headers={
        "Accept": "text/event-stream", "Content-Type": "application/json", "User-Agent": "deepseek-harness-rs-release-verifier"})
    text, calls, finished, received = "", {}, False, 0
    with open_with_retry(request, timeout) as response:
        for raw in response:
            received += len(raw)
            if received > 8 * 1024 * 1024:
                raise ValueError("free Responses stream exceeded 8 MiB")
            line = raw.decode("utf-8").strip()
            if not line.startswith("data:"):
                continue
            value = line[5:].strip()
            if value == "[DONE]":
                continue
            event = json.loads(value)
            kind = event.get("type")
            if kind in ("error", "response.failed", "response.incomplete"):
                raise ValueError("free Responses stream returned a provider failure")
            if kind == "response.output_text.delta":
                text += event.get("delta", "")
            elif kind == "response.output_item.added" and event.get("item", {}).get("type") == "function_call":
                item = event["item"]
                calls[item["id"]] = {"type": "function_call", "call_id": item["call_id"], "name": item["name"], "arguments": item.get("arguments", "")}
            elif kind == "response.function_call_arguments.delta" and event.get("item_id") in calls:
                calls[event["item_id"]]["arguments"] += event.get("delta", "")
            elif kind == "response.completed":
                for item in event.get("response", {}).get("output", []):
                    if item.get("type") == "function_call":
                        calls[item.get("id", item["call_id"])] = {key: item[key] for key in ("type", "call_id", "name", "arguments")}
                    elif item.get("type") == "message" and not text:
                        text += "".join(part.get("text", "") for part in item.get("content", []))
                finished = True
    if not finished:
        raise ValueError("free Responses stream ended before response.completed")
    return {"text": text, "calls": list(calls.values())}


def responses_probe(model_id: str, url: str, timeout: float) -> dict:
    endpoint = url.rsplit("/", 1)[0] + "/responses"
    body = {"model": model_id, "instructions": "Follow the user's connectivity check exactly.",
            "input": [{"role": "user", "content": "Call connectivity_check with status set to ok."}],
            "tools": [{"type": "function", "name": "connectivity_check", "description": "Confirm connection", "parameters": {"type": "object", "properties": {"status": {"type": "string", "enum": ["ok"]}}, "required": ["status"]}}],
            "stream": True, "max_output_tokens": 1024}
    started = time.monotonic()
    first = responses_completion(endpoint, body, timeout)
    calls = first["calls"]
    if not calls or any(call["name"] != "connectivity_check" or json.loads(call["arguments"]) != {"status": "ok"} for call in calls):
        raise ValueError("free Responses model did not return the requested tool call")
    body["input"] += calls + [{"type": "function_call_output", "call_id": call["call_id"], "output": "ok"} for call in calls]
    body["input"].append({"role": "user", "content": "Reply with the single word OK."})
    body.pop("tools")
    if "OK" not in responses_completion(endpoint, body, timeout)["text"].upper():
        raise ValueError("free Responses tool-result conversation did not complete")
    return {"inference": True, "streaming": True, "toolCall": True, "toolResult": True, "anonymous": True,
            "latencyMs": round((time.monotonic() - started) * 1000)}


def binary_sha256(binary: Path) -> str:
    digest = hashlib.sha256()
    with binary.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def harness_rpc(base: str, method: str, payload: dict, timeout: float = 15.0) -> dict:
    request = urllib.request.Request(base + "/api/" + method, data=json.dumps({
        "type": "client-request", "rpcId": "free-release-verification", "method": method, "payload": payload,
    }).encode("utf-8"), headers={"Content-Type": "application/json", "Origin": base, "Sec-Fetch-Site": "same-origin"})
    # Local mutations must never be automatically retried: a lost response must
    # not create a second session or dispatch the same prompt twice.
    with urllib.request.urlopen(request, timeout=timeout) as response:
        raw = response.read(8 * 1024 * 1024 + 1)
    if len(raw) > 8 * 1024 * 1024:
        raise ValueError("Harness RPC response exceeded 8 MiB")
    result = json.loads(raw).get("result") or {}
    if result.get("ok") is not True:
        error = result.get("error") or {}
        raise ValueError(f"Harness {method} failed: {error.get('message', 'unknown error')}")
    return result.get("value") or {}


def stop_verification_host(process: subprocess.Popen) -> None:
    if process.poll() is None:
        if os.name == "nt":
            # This PID was created below and is still owned by this Popen.
            try:
                subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15,
                               creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0), check=False)
            except (OSError, subprocess.TimeoutExpired):
                process.kill()
        else:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            if os.name != "nt":
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            else:
                process.kill()
            process.wait(timeout=10)
    for stream in (process.stdout, process.stderr):
        if stream:
            stream.close()


def verify_harness(binary: Path, model_id: str, url: str, workdir: Path | None, timeout: float, api: str = "openai-completions") -> dict:
    binary = binary.resolve(strict=True)
    if not binary.is_file():
        raise ValueError("--binary must name the actual release executable")
    if url != DEFAULT_CATALOG_URL:
        raise ValueError("binary verification requires the official anonymous OpenCode route")
    route = provider_for(api)
    digest = binary_sha256(binary)
    if workdir is not None:
        workdir = workdir.resolve()
        workdir.mkdir(parents=True, exist_ok=True)
    parent = workdir or Path(tempfile.gettempdir()).resolve()
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="dsh-free-release-", dir=str(parent)) as temporary:
        root = Path(temporary).resolve()
        if root.parent != parent or not root.name.startswith("dsh-free-release-"):
            raise ValueError("verification directory escaped its temporary parent")
        home, workspace, user_home, temp = (root / name for name in ("dsh-home", "workspace", "user-home", "tmp"))
        for directory in (home, workspace, user_home, temp):
            directory.mkdir()
        marker = workspace / "connectivity-check.txt"
        marker.write_text("DSH connectivity verification\n", encoding="utf-8")
        model_config = {"id": model_id, "maxTokens": 16384}
        if model_id == DEFAULT_MODEL_ID:
            model_config.update(contextWindow=262144, reasoningEfforts=False)
        profile = {"displayName": "OpenCode Free", "keyless": True, "api": api,
                   "baseURL": "https://opencode.ai/zen/v1", "models": [model_config]}
        (home / "settings.json").write_text(json.dumps({
            "llm-pi-ai": {"providers": {route: profile}},
            "agent-default-model": {"provider": route, "model": model_id},
        }), encoding="utf-8")
        # Whitelist only process/runtime variables. API keys, release tokens,
        # proxy credentials, and the user's account stores are not inherited.
        allowed = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "LANG", "LC_ALL", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"}
        env = {key: value for key, value in os.environ.items() if key.upper() in allowed}
        env.update(DSH_HOME=str(home), HOME=str(user_home), USERPROFILE=str(user_home),
                   APPDATA=str(user_home / "AppData" / "Roaming"), LOCALAPPDATA=str(user_home / "AppData" / "Local"),
                   TEMP=str(temp), TMP=str(temp), TMPDIR=str(temp))
        process = subprocess.Popen([str(binary), "web", "--port", "0"], cwd=str(workspace), env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace",
            creationflags=(getattr(subprocess, "CREATE_NO_WINDOW", 0) | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)),
            start_new_session=os.name != "nt")
        lines = queue.Queue(maxsize=1024)
        def drain(stream):
            for line in iter(stream.readline, ""):
                try:
                    lines.put_nowait(line.rstrip("\r\n"))
                except queue.Full:
                    pass
        readers = [threading.Thread(target=drain, args=(stream,), daemon=True) for stream in (process.stdout, process.stderr)]
        for reader in readers:
            reader.start()
        base = None
        session_id = None
        complete = False
        try:
            deadline = started + timeout
            readiness_deadline = min(deadline, time.monotonic() + 45)
            recent = []
            while time.monotonic() < readiness_deadline:
                if process.poll() is not None:
                    raise ValueError(f"verification binary exited before readiness (code {process.returncode})")
                try:
                    line = lines.get(timeout=0.25)
                except queue.Empty:
                    continue
                recent = (recent + [line])[-8:]
                match = re.fullmatch(r"dsh web: http://127\.0\.0\.1:(\d+)", line)
                if match:
                    base = "http://127.0.0.1:" + match.group(1)
                    break
            if base is None:
                raise ValueError("verification binary did not report readiness: " + " | ".join(recent)[-1000:])
            def rpc(method, payload):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("Harness verification timed out")
                return harness_rpc(base, method, payload, min(15, remaining))
            workspace_id = rpc("workspace.create", {"path": str(workspace)})["workspace"]["workspaceId"]
            session_id = rpc("session.create", {"workspaceId": workspace_id})["sessionId"]
            rpc("session.selectModel", {"sessionId": session_id, "provider": route, "model": model_id})
            rpc("session.prompt", {"sessionId": session_id, "mode": "queue", "content": [{"type": "text", "text":
                "只使用 glob 工具列出当前工作目录的 connectivity-check.txt，得到工具结果后严格只回复检测口令 OK。不要使用其它工具，不要创建或修改文件。"}]})
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise ValueError("verification binary exited during the model request")
                history = rpc("session.history", {"sessionId": session_id, "maxMessages": 200})
                events = [row["event"] for row in history.get("events", []) if isinstance(row.get("event"), dict)]
                calls = [event for event in events if event.get("type") == "tool/call"]
                if any(event.get("data", {}).get("name") != "glob" for event in calls):
                    names = [event.get("data", {}).get("name") for event in calls if event.get("data", {}).get("name") != "glob"]
                    raise ValueError("free model selected unexpected tools during read-only verification: " + json.dumps(names[:5]))
                ended = [event for event in events if event.get("type") == "turn/end"]
                if ended:
                    reason = ended[-1].get("data", {}).get("reason", {})
                    if reason.get("kind") != "completed":
                        raise ValueError("Harness turn did not complete: " + json.dumps(reason, ensure_ascii=False)[:700])
                    results = [event for event in events if event.get("type") == "tool/result"]
                    call_ids = {event["data"]["callId"] for event in calls}
                    matched = [event for event in results if any(
                        block.get("type") == "tool-result" and block.get("toolCallId") in call_ids and block.get("isError") is False
                        and any("connectivity-check.txt" in part.get("text", "") for part in block.get("content", []))
                        for block in event.get("data", {}).get("message", {}).get("content", []))]
                    messages = [event for event in events if event.get("type") == "assistant/message"]
                    final_text = "".join(block.get("text", "") for block in messages[-1].get("data", {}).get("message", {}).get("content", [])) if messages else ""
                    if not calls or not matched or not re.search(r"(?<![A-Za-z])OK(?![A-Za-z])", final_text, re.IGNORECASE):
                        raise ValueError(f"Harness did not complete the glob/tool-result/OK round trip (calls={len(calls)}, successful results={len(matched)}, reply={final_text[:100]!r})")
                    configs = [event.get("data", {}).get("header", {}).get("config", {}) for event in events if event.get("type") == "request/header"]
                    contexts = [event.get("data", {}) for event in events if event.get("type") == "request/context"]
                    if not any(config.get("model") == model_id and config.get("maxTokens") == 16384 for config in configs):
                        raise ValueError("Harness used the wrong model or output budget")
                    if model_id == DEFAULT_MODEL_ID and not any(context.get("contextWindow") == 262144 and context.get("contextWindowEstimated") is not True for context in contexts):
                        raise ValueError("Harness did not use the declared model context capacity")
                    if sorted(path.name for path in workspace.iterdir()) != [marker.name] or marker.read_text(encoding="utf-8") != "DSH connectivity verification\n":
                        raise ValueError("read-only verification modified its workspace")
                    complete = True
                    break
                time.sleep(min(0.75, max(0, deadline - time.monotonic())))
            if not complete:
                raise TimeoutError("Harness did not finish its free-model turn within the verification timeout")
        finally:
            if not complete and base and session_id and process.poll() is None:
                try:
                    harness_rpc(base, "session.cancel", {"sessionId": session_id}, timeout=3)
                except Exception:
                    pass
            stop_verification_host(process)
            for reader in readers:
                reader.join(timeout=2)
        if binary_sha256(binary) != digest:
            raise ValueError("the executable changed during verification")
    return {"harnessVerified": True, "binarySha256": digest, "harnessModel": model_id,
            "harnessTool": "glob", "harnessToolResult": True, "harnessCompleted": True,
            **({"contextWindow": 262144, "reasoningEfforts": False} if model_id == DEFAULT_MODEL_ID else {}), "maxTokens": 16384,
            "harnessLatencyMs": round((time.monotonic() - started) * 1000)}


def save_report(path: Path | None, report: dict) -> None:
    if path is not None:
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(path.name + ".tmp")
        temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        temporary.replace(path)


def verify_many(url: str = DEFAULT_CATALOG_URL, binary: Path | None = None, workdir: Path | None = None,
                timeout: float = 180, report_path: Path | None = None, preferred: str = DEFAULT_MODEL_ID) -> dict:
    if url != DEFAULT_CATALOG_URL:
        raise ValueError("free verification is restricted to the official anonymous endpoint")
    catalog = fetch_model_ids(url)
    prices = pricing_catalog()
    ids = sorted(set(prices) | {model for model in catalog if model.endswith("-free")}, key=lambda model: (model != preferred, model))
    report = {"schemaVersion": 2, "url": url, "pricingSource": PRICING_URL,
              "verifiedAt": datetime.now(timezone.utc).isoformat(), "binarySha256": binary_sha256(binary) if binary else None,
              "models": [], "includedModels": [], "defaultModel": None}
    for model in ids:
        row = {"model": model, "name": model, "status": "pending-verification", "available": False,
               "verifiedAt": datetime.now(timezone.utc).isoformat(), "catalogAvailable": model in catalog,
               "freePricingVerified": False, **prices.get(model, {})}
        report["models"].append(row)
    save_report(report_path, report)
    for row in report["models"]:
        model = row["model"]
        print(json.dumps({"model": model, "phase": "checking"}), flush=True)
        if not row["catalogAvailable"]:
            row.update(status="retired", reason="当前官方模型目录已移除此模型")
        elif not row["freePricingVerified"]:
            row.update(reason="当前官方价格表未确认此精确模型免费；未发送推理请求")
        else:
            try:
                row.update(inference_probe(model, url, min(timeout, 90), row["api"]))
                if binary:
                    row.update(verify_harness(binary, model, url, workdir, timeout, row["api"]))
                    row.update(status="available", available=True)
                    report["includedModels"].append({"provider": row["provider"], "model": model})
                    report["defaultModel"] = report["defaultModel"] or report["includedModels"][-1]
                else:
                    row.update(reason="匿名流式及工具往返已通过，等待正式二进制验证")
            except urllib.error.HTTPError as error:
                row.update(status="rate-limited" if error.code == 429 else "unavailable",
                           httpStatus=error.code, reason=f"匿名端点返回 HTTP {error.code}")
            except Exception as error:
                message = str(error)
                limited = "429" in message or "rate_limit" in message.lower() or "rate limit" in message.lower()
                row.update(status="rate-limited" if limited else "unavailable", reason=message[:700])
        row["verifiedAt"] = datetime.now(timezone.utc).isoformat()
        save_report(report_path, report)
        print(json.dumps({"model": model, "status": row["status"], "reason": row.get("reason")}, ensure_ascii=True), flush=True)
    report["verifiedAt"] = datetime.now(timezone.utc).isoformat()
    save_report(report_path, report)
    return report


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=DEFAULT_MODEL_ID)
    parser.add_argument("--all", action="store_true", help="Check every live officially priced free candidate independently")
    parser.add_argument("--prefer", default=DEFAULT_MODEL_ID, help="Preferred default if this model passes all checks")
    parser.add_argument("--url", default=DEFAULT_CATALOG_URL)
    parser.add_argument("--report", help="Write the successful release-gate evidence as JSON")
    parser.add_argument("--binary", type=Path, help="Verify a real release executable using an isolated Harness session")
    parser.add_argument("--workdir", type=Path, help="Parent directory for temporary Harness verification data")
    parser.add_argument("--timeout", type=float, default=180, help="Maximum seconds for the isolated Harness verification")
    args = parser.parse_args()
    if not 30 <= args.timeout <= 600:
        parser.error("--timeout must be between 30 and 600 seconds")
    if args.binary and not args.binary.is_file():
        parser.error("--binary must name an existing release executable")
    if args.report:
        # A failed rerun must not leave an older successful attestation behind.
        Path(args.report).unlink(missing_ok=True)
    if args.all:
        evidence = verify_many(args.url, args.binary, args.workdir, args.timeout, Path(args.report) if args.report else None, args.prefer)
        passed = evidence["includedModels"] if args.binary else [row for row in evidence["models"] if row.get("inference") is True]
        if not passed:
            raise SystemExit("no candidate passed this verification stage; per-model evidence was retained")
        print(json.dumps({"includedModels": evidence["includedModels"], "defaultModel": evidence["defaultModel"]}))
        return
    evidence = verify(args.model, args.url)
    if args.binary:
        evidence.update(verify_harness(args.binary, args.model, args.url, args.workdir, args.timeout, evidence["api"]))
    if args.report:
        Path(args.report).parent.mkdir(parents=True, exist_ok=True)
        with open(args.report, "w", encoding="utf-8") as stream:
            json.dump(evidence, stream, ensure_ascii=False, indent=2)
    print(json.dumps(evidence))


if __name__ == "__main__":
    main()
