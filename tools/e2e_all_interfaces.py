from __future__ import annotations

import argparse
import http.client
import json
import os
import pathlib
import queue
import re
import socket
import subprocess
import tempfile
import threading
import time


def non_loopback_ipv4() -> str:
    addresses = []
    for info in socket.getaddrinfo(socket.gethostname(), None, socket.AF_INET):
        address = info[4][0]
        if not address.startswith("127.") and address != "0.0.0.0":
            addresses.append(address)
    if not addresses:
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            probe.connect(("192.0.2.1", 9))
            addresses.append(probe.getsockname()[0])
        finally:
            probe.close()
    if not addresses:
        raise RuntimeError("no non-loopback IPv4 address")
    return sorted(set(addresses))[0]


def api_call(host: str, port: int, origin: str) -> tuple[int, dict[str, object]]:
    body = json.dumps({
        "type": "client-request",
        "rpcId": "bind-e2e",
        "method": "llm.providers",
        "payload": {},
    })
    connection = http.client.HTTPConnection(host, port, timeout=10)
    connection.request(
        "POST",
        "/api/llm.providers",
        body=body,
        headers={
            "Content-Type": "application/json",
            "Host": f"{host}:{port}",
            "Origin": origin,
            "Sec-Fetch-Site": "same-origin" if origin.startswith(f"http://{host}:{port}") else "cross-site",
        },
    )
    response = connection.getresponse()
    raw = response.read()
    connection.close()
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        payload = {}
    return response.status, payload


def launch(binary: pathlib.Path, home: pathlib.Path, host: str | None):
    args = [str(binary), "web", "--port", "0"]
    if host is not None:
        args.extend(["--host", host])
    process = subprocess.Popen(
        args,
        cwd=binary.parent,
        env={**os.environ, "DSH_HOME": str(home)},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        creationflags=getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0),
    )
    lines: queue.Queue[str] = queue.Queue()
    observed: list[str] = []
    for stream in (process.stdout, process.stderr):
        threading.Thread(
            target=lambda s=stream: [lines.put(line.rstrip()) for line in iter(s.readline, "")],
            daemon=True,
        ).start()
    deadline = time.monotonic() + 30
    port = None
    advertised = None
    # stdout readiness and stderr warnings arrive on independent reader
    # threads; neither pipe establishes ordering across the other one.
    while time.monotonic() < deadline and (
        port is None
        or (host == "0.0.0.0" and not any("WARNING:" in line for line in observed))
    ):
        try:
            line = lines.get(timeout=0.2)
        except queue.Empty:
            if process.poll() is not None:
                break
            continue
        observed.append(line)
        match = re.search(r"dsh web: http://([^:]+):(\d+)", line)
        if match:
            advertised, port = match.group(1), int(match.group(2))
    if port is None:
        process.terminate()
        raise RuntimeError(f"dsh did not start: {observed[-8:]}")
    return process, advertised, port, observed


def stop(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=10)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    args = parser.parse_args()
    binary = pathlib.Path(args.binary).resolve()
    lan = non_loopback_ipv4()
    with tempfile.TemporaryDirectory(prefix="dsh-bind-e2e-") as temporary:
        root = pathlib.Path(temporary)
        exposed, advertised, port, output = launch(binary, root / "all", "0.0.0.0")
        try:
            connection = http.client.HTTPConnection(lan, port, timeout=10)
            connection.request("GET", "/", headers={"Host": f"{lan}:{port}"})
            root_status = connection.getresponse().status
            connection.close()
            api_status, api = api_call(lan, port, f"http://{lan}:{port}")
            cross_status, _ = api_call(lan, port, "http://attacker.invalid")
            if advertised != "0.0.0.0" or root_status != 200 or api_status != 200:
                raise AssertionError("all-interface route was not usable")
            if api.get("type") != "server-response" or cross_status != 403:
                raise AssertionError("all-interface API origin policy failed")
            if not any("WARNING:" in line for line in output):
                raise AssertionError("all-interface launch omitted its exposure warning")
        finally:
            stop(exposed)

        local, default_advertised, default_port, _ = launch(binary, root / "default", None)
        try:
            blocked = False
            try:
                connection = http.client.HTTPConnection(lan, default_port, timeout=2)
                connection.request("GET", "/")
                connection.getresponse().read()
                connection.close()
            except OSError:
                blocked = True
            if default_advertised != "127.0.0.1" or not blocked:
                raise AssertionError("default listener escaped loopback")
        finally:
            stop(local)

    print(json.dumps({
        "all_interface_api_status": api_status,
        "cross_site_status": cross_status,
        "default_loopback_only": True,
        "root_status": root_status,
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
