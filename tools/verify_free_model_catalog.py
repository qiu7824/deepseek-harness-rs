from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import re
import signal
import subprocess
import tempfile
import threading
import time
import urllib.error
from pathlib import Path
import urllib.request
from datetime import datetime, timezone
from html.parser import HTMLParser


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


def verify_free_pricing(model_id: str) -> dict:
    request = urllib.request.Request(PRICING_URL, headers={"User-Agent": "deepseek-harness-rs-release-verifier"})
    with open_with_retry(request, timeout=25) as response:
        html = response.read(4 * 1024 * 1024).decode("utf-8")
    parser = PricingTables()
    parser.feed(html)
    label = next((row[0] for row in parser.rows if len(row) >= 2 and row[1] == model_id), None)
    prices = next((row for row in parser.rows if len(row) >= 4 and row[0] == label and
                   all(value.lower() == "free" for value in row[1:4])), None)
    if not prices:
        raise ValueError("the official pricing table does not currently confirm this model is free")
    return {"freePricingVerified": True, "pricingSource": PRICING_URL, "pricingLabel": label}


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
                raise ValueError("free inference stream returned an error")
            for choice in payload.get("choices", []):
                delta = choice.get("delta") or {}
                if delta.get("content"):
                    message["content"] += delta["content"]
                for call in delta.get("tool_calls", []):
                    target = calls.setdefault(call.get("index", 0), {"id": "", "type": "function", "function": {"name": "", "arguments": ""}})
                    if call.get("id"):
                        target["id"] = call["id"]
                    for key in ("name", "arguments"):
                        target["function"][key] += call.get("function", {}).get(key, "")
                if choice.get("finish_reason"):
                    finished = True
        if not finished:
            raise ValueError("free inference stream ended without completion")
        if calls:
            message["tool_calls"] = [calls[index] for index in sorted(calls)]
        return message


def inference_probe(model_id: str, url: str, timeout: float = 90.0) -> dict:
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
    ids = fetch_model_ids(url)
    if model_id not in ids:
        raise ValueError(f"free model {model_id!r} is absent from {url}")
    pricing = verify_free_pricing(model_id)
    return {"url": url, "model": model_id, "available": True,
            "verifiedAt": datetime.now(timezone.utc).isoformat(), **pricing, **inference_probe(model_id, url)}


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


def verify_harness(binary: Path, model_id: str, url: str, workdir: Path | None, timeout: float) -> dict:
    binary = binary.resolve(strict=True)
    if not binary.is_file():
        raise ValueError("--binary must name the actual release executable")
    if model_id != "ling-3.0-flash-fin-free" or url != DEFAULT_CATALOG_URL:
        raise ValueError("binary verification currently requires the official Ling free route and its verified capacities")
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
        profile = {"displayName": "OpenCode Free", "keyless": True, "api": "openai-completions",
                   "baseURL": "https://opencode.ai/zen/v1", "models": [{"id": model_id,
                   "contextWindow": 262144, "maxTokens": 16384, "reasoningEfforts": False}]}
        (home / "settings.json").write_text(json.dumps({
            "llm-pi-ai": {"providers": {"opencode-free": profile}},
            "agent-default-model": {"provider": "opencode-free", "model": model_id},
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
            rpc("session.selectModel", {"sessionId": session_id, "provider": "opencode-free", "model": model_id})
            rpc("session.prompt", {"sessionId": session_id, "mode": "queue", "content": [{"type": "text", "text":
                "只使用 glob 工具列出当前工作目录的 connectivity-check.txt，得到工具结果后严格只回复检测口令 OK。不要使用其它工具，不要创建或修改文件。"}]})
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise ValueError("verification binary exited during the model request")
                history = rpc("session.history", {"sessionId": session_id, "maxMessages": 200})
                events = [row["event"] for row in history.get("events", []) if isinstance(row.get("event"), dict)]
                calls = [event for event in events if event.get("type") == "tool/call"]
                if any(event.get("data", {}).get("name") != "glob" for event in calls):
                    raise ValueError("free model selected an unexpected tool during read-only verification")
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
                    if not calls or not matched or final_text.strip().strip("`").strip() != "OK":
                        raise ValueError(f"Harness did not complete the glob/tool-result/OK round trip (calls={len(calls)}, successful results={len(matched)}, reply={final_text[:100]!r})")
                    configs = [event.get("data", {}).get("header", {}).get("config", {}) for event in events if event.get("type") == "request/header"]
                    contexts = [event.get("data", {}) for event in events if event.get("type") == "request/context"]
                    if not any(config.get("model") == model_id and config.get("maxTokens") == 16384 for config in configs):
                        raise ValueError("Harness used the wrong model or output budget")
                    if not any(context.get("contextWindow") == 262144 and context.get("contextWindowEstimated") is not True for context in contexts):
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
            "contextWindow": 262144, "maxTokens": 16384,
            "harnessLatencyMs": round((time.monotonic() - started) * 1000)}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=DEFAULT_MODEL_ID)
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
    evidence = verify(args.model, args.url)
    if args.binary:
        evidence.update(verify_harness(args.binary, args.model, args.url, args.workdir, args.timeout))
    if args.report:
        Path(args.report).parent.mkdir(parents=True, exist_ok=True)
        with open(args.report, "w", encoding="utf-8") as stream:
            json.dump(evidence, stream, ensure_ascii=False, indent=2)
    print(json.dumps(evidence))


if __name__ == "__main__":
    main()
