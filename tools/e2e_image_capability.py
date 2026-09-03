from __future__ import annotations

import argparse
import base64
import binascii
import contextlib
import http.client
import json
import os
import pathlib
import queue
import re
import subprocess
import struct
import tempfile
import threading
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def png_chunk(kind: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + kind
        + payload
        + struct.pack(">I", binascii.crc32(kind + payload) & 0xFFFFFFFF)
    )


PNG_1X1 = (
    b"\x89PNG\r\n\x1a\n"
    + png_chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0))
    + png_chunk(b"IDAT", zlib.compress(b"\x00\xff\x00\x00\xff"))
    + png_chunk(b"IEND", b"")
)


class CaptureHandler(BaseHTTPRequestHandler):
    server_version = "DSHImageFixture/1"

    def do_POST(self) -> None:  # noqa: N802 - stdlib handler contract
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        self.server.capture.put((self.path, body))
        chunks = (
            'data: {"type":"response.output_text.delta","delta":"ok"}\n\n'
            'data: {"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1}}}\n\n'
        ).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("content-length", str(len(chunks)))
        self.end_headers()
        self.wfile.write(chunks)
        self.wfile.flush()

    def log_message(self, _format: str, *_args: object) -> None:
        return


class CaptureServer(ThreadingHTTPServer):
    capture: queue.Queue[tuple[str, bytes]]


def read_lines(stream, output: queue.Queue[str]) -> None:
    for line in iter(stream.readline, ""):
        output.put(line.rstrip("\r\n"))


def rpc(port: int, method: str, payload: dict[str, object], sequence: int) -> dict[str, object]:
    rpc_id = f"image-e2e-{sequence}"
    body = json.dumps(
        {
            "type": "client-request",
            "rpcId": rpc_id,
            "method": method,
            "payload": payload,
        },
        separators=(",", ":"),
    ).encode("utf-8")
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=20)
    connection.request(
        "POST",
        f"/api/{method}",
        body=body,
        headers={
            "content-type": "application/json",
            "content-length": str(len(body)),
            "origin": f"http://127.0.0.1:{port}",
            "sec-fetch-site": "same-origin",
        },
    )
    response = connection.getresponse()
    raw = response.read()
    connection.close()
    if response.status != 200:
        raise AssertionError(f"{method}: HTTP {response.status}: {raw[:200]!r}")
    decoded = json.loads(raw)
    if decoded.get("rpcId") != rpc_id:
        raise AssertionError(f"{method}: wrong rpcId")
    return decoded


def require_ok(response: dict[str, object], method: str) -> dict[str, object]:
    result = response.get("result")
    if not isinstance(result, dict) or result.get("ok") is not True:
        raise AssertionError(f"{method}: {result!r}")
    value = result.get("value")
    return value if isinstance(value, dict) else {}


def contains_input_image(value: object) -> bool:
    if isinstance(value, dict):
        if value.get("type") == "input_image" and isinstance(value.get("image_url"), str):
            return value["image_url"].startswith("data:image/")
        return any(contains_input_image(child) for child in value.values())
    if isinstance(value, list):
        return any(contains_input_image(child) for child in value)
    return False


def wire_shape(value: object, key: str | None = None) -> object:
    if isinstance(value, dict):
        return {name: wire_shape(child, name) for name, child in value.items()}
    if isinstance(value, list):
        return [wire_shape(child) for child in value]
    if isinstance(value, str):
        if key in {"type", "role", "model"}:
            return value
        if value.startswith("data:"):
            return "<data-url>"
        return "<string>"
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--repo", default=str(pathlib.Path(__file__).resolve().parents[1]))
    args = parser.parse_args()
    binary = pathlib.Path(args.binary).resolve()
    repo = pathlib.Path(args.repo).resolve()
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}")

    capture: queue.Queue[tuple[str, bytes]] = queue.Queue()
    provider = CaptureServer(("127.0.0.1", 0), CaptureHandler)
    provider.capture = capture
    provider_thread = threading.Thread(target=provider.serve_forever, daemon=True)
    provider_thread.start()
    provider_port = provider.server_address[1]

    process: subprocess.Popen[str] | None = None
    with contextlib.ExitStack() as stack:
        home = stack.enter_context(tempfile.TemporaryDirectory(prefix="dsh-image-e2e-"))
        stack.callback(provider.server_close)
        stack.callback(provider.shutdown)

        def stop_process() -> None:
            if process is None or process.poll() is not None:
                return
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)

        stack.callback(stop_process)
        environment = os.environ.copy()
        environment["DSH_HOME"] = home
        process = subprocess.Popen(
            [str(binary), "web", "--port", "0"],
            cwd=repo,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        lines: queue.Queue[str] = queue.Queue()
        assert process.stdout is not None
        assert process.stderr is not None
        threading.Thread(target=read_lines, args=(process.stdout, lines), daemon=True).start()
        threading.Thread(target=read_lines, args=(process.stderr, lines), daemon=True).start()
        deadline = time.monotonic() + 40
        host_port: int | None = None
        startup_lines: list[str] = []
        while time.monotonic() < deadline:
            if process.poll() is not None:
                while True:
                    try:
                        startup_lines.append(lines.get_nowait())
                    except queue.Empty:
                        break
                raise AssertionError(
                    f"dsh exited before readiness: {process.returncode}: {startup_lines[-8:]!r}"
                )
            try:
                line = lines.get(timeout=0.25)
            except queue.Empty:
                continue
            startup_lines.append(line)
            match = re.fullmatch(r"dsh web: http://127\.0\.0\.1:(\d+)", line)
            if match:
                host_port = int(match.group(1))
                break
        if host_port is None:
            raise AssertionError("dsh did not report loopback readiness")

        sequence = 1
        provider_value = {
            "keyless": True,
            "api": "openai-responses",
            "baseURL": f"http://127.0.0.1:{provider_port}/v1",
            "models": [
                {
                    "id": "gpt-vision-fixture",
                    "input": ["text", "image"],
                }
            ],
        }
        require_ok(
            rpc(
                host_port,
                "settings.mutate",
                {
                    "ns": "llm-pi-ai",
                    "ops": [
                        {
                            "op": "set",
                            "path": ["providers", "vision-fixture"],
                            "value": provider_value,
                        }
                    ],
                },
                sequence,
            ),
            "settings.mutate",
        )
        sequence += 1

        created = require_ok(
            rpc(host_port, "session.create", {"cwd": str(repo)}, sequence),
            "session.create",
        )
        sequence += 1
        session_id = created.get("sessionId")
        if not isinstance(session_id, str) or not session_id:
            raise AssertionError("session.create returned no sessionId")

        deadline = time.monotonic() + 10
        last_selection: dict[str, object] | None = None
        while time.monotonic() < deadline:
            selection = rpc(
                host_port,
                "session.selectModel",
                {
                    "sessionId": session_id,
                    "provider": "vision-fixture",
                    "model": "gpt-vision-fixture",
                },
                sequence,
            )
            sequence += 1
            result = selection.get("result")
            if isinstance(result, dict) and result.get("ok") is True:
                last_selection = selection
                break
            time.sleep(0.05)
        if last_selection is None:
            raise AssertionError("configured vision model did not become selectable")

        prompt = require_ok(
            rpc(
                host_port,
                "session.prompt",
                {
                    "sessionId": session_id,
                    "mode": "queue",
                    "content": [
                        {
                            "type": "image",
                            "mediaType": "image/png",
                            "data": base64.b64encode(PNG_1X1).decode("ascii"),
                            "name": "fixture.png",
                        },
                        {"type": "text", "text": "describe this fixture"},
                    ],
                    "clientTimeZone": "UTC",
                },
                sequence,
            ),
            "session.prompt",
        )
        sequence += 1
        if prompt.get("accepted") is not True:
            raise AssertionError(f"session.prompt was not accepted: {prompt!r}")

        captured_requests = 0
        provider_request: dict[str, object] | None = None
        request_path = ""
        captured_shapes: list[object] = []
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            try:
                candidate_path, request_bytes = capture.get(timeout=0.25)
            except queue.Empty:
                continue
            captured_requests += 1
            candidate = json.loads(request_bytes)
            captured_shapes.append(wire_shape(candidate))
            if (
                candidate_path == "/v1/responses"
                and candidate.get("model") == "gpt-vision-fixture"
                and contains_input_image(candidate)
            ):
                request_path = candidate_path
                provider_request = candidate
                break
        if provider_request is None:
            raise AssertionError(
                "no provider request contained Responses input_image data: "
                + json.dumps(captured_shapes[-4:], sort_keys=True)
            )

        turn_completed = False
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            history = require_ok(
                rpc(
                    host_port,
                    "session.history",
                    {"sessionId": session_id, "maxMessages": 12},
                    sequence,
                ),
                "session.history",
            )
            sequence += 1
            events = history.get("events")
            if isinstance(events, list) and any(
                isinstance(entry, dict)
                and isinstance(entry.get("event"), dict)
                and entry["event"].get("type") == "turn/end"
                for entry in events
            ):
                turn_completed = True
                break
            time.sleep(0.05)
        if not turn_completed:
            raise AssertionError("image turn did not reach turn/end")

        print(
            json.dumps(
                {
                    "settings_live": True,
                    "model_selected": True,
                    "prompt_accepted": True,
                    "provider_path": request_path,
                    "captured_provider_requests": captured_requests,
                    "wire_input_image": True,
                    "turn_completed": True,
                },
                sort_keys=True,
            )
        )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
