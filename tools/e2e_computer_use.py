"""Exercise the shared native browser through the real Host GUI route."""
from __future__ import annotations

import argparse
import base64
import json
import os
import pathlib
import signal
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc


class PageFixture(BaseHTTPRequestHandler):
    def log_message(self, *_args: object) -> None:
        pass

    def do_GET(self) -> None:
        title = "second" if self.path.startswith("/second") else "ready"
        body = f"""<!doctype html><html><head><meta charset=utf-8><title>{title}</title>
        <style>html,body{{margin:0}}body{{height:3000px}}button{{position:absolute;left:20px;top:20px;width:180px;height:50px}}input{{position:absolute;left:20px;top:100px;width:300px;height:40px}}</style></head>
        <body><button onclick=\"document.title='clicked'\">Click</button>
        <input id=entry oninput=\"document.title='typed:'+this.value\"><input id=secret type=password style=\"top:160px\"><div id=dragger style=\"position:absolute;left:20px;top:230px;width:180px;height:50px;background:blue\" onmousedown=\"window.dragging=true\">Drag</div><script>document.addEventListener('mouseup',()=>{{if(window.dragging){{document.title='dragged';window.dragging=false}}}})</script><div style=\"position:absolute;top:2600px\">bottom</div></body></html>""".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def browser_request(port: int, operation: str, payload: dict[str, object], *, expected_status: int = 200) -> dict[str, object]:
    raw = json.dumps(payload, separators=(",", ":")).encode()
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/__dsh-computer-use/{operation}",
        data=raw,
        headers={
            "Content-Type": "application/json",
            "Origin": f"http://127.0.0.1:{port}",
            "Sec-Fetch-Site": "same-origin",
        },
    )
    try:
        response = urllib.request.urlopen(request, timeout=40)
    except urllib.error.HTTPError as failure:
        if failure.code != expected_status:
            raise AssertionError(f"{operation}: HTTP {failure.code}: {failure.read(8192).decode('utf-8', 'replace')}") from failure
        return json.loads(failure.read())
    with response:
        if response.status != expected_status:
            raise AssertionError(f"{operation}: expected HTTP {expected_status}, got {response.status}")
        return json.loads(response.read())


def terminate_browser_profile(profile: pathlib.Path) -> list[int]:
    needle = str(profile.resolve())
    if os.name == "nt":
        env = os.environ.copy()
        env["DSH_CDP_PROFILE_UNDER_TEST"] = needle
        script = r"""
$needle = $env:DSH_CDP_PROFILE_UNDER_TEST
$ids = @(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -and $_.CommandLine.Contains($needle) } | ForEach-Object { [int]$_.ProcessId })
if ($ids.Count -eq 0) { Write-Error "controlled browser process not found"; exit 2 }
$ids | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }
$ids -join ','
"""
        completed = subprocess.run(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
            check=True,
            capture_output=True,
            text=True,
            env=env,
            timeout=20,
        )
        return [int(value) for value in completed.stdout.strip().split(",") if value]
    killed: list[int] = []
    for command_line in pathlib.Path("/proc").glob("[0-9]*/cmdline"):
        try:
            if needle.encode() not in command_line.read_bytes():
                continue
            pid = int(command_line.parent.name)
            os.kill(pid, signal.SIGKILL)
            killed.append(pid)
        except (FileNotFoundError, PermissionError, ProcessLookupError, ValueError):
            continue
    if not killed:
        raise AssertionError("controlled browser process not found")
    return killed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    parser.add_argument("--browser", type=pathlib.Path)
    args = parser.parse_args()
    run = args.workdir.resolve() / f"run-{int(time.time() * 1000)}"
    home = run / "home"
    workspace = run / "workspace"
    workspace.mkdir(parents=True)
    env = isolated_environment(run, home)
    if os.name == "nt":
        env["__COMPAT_LAYER"] = "DetectorsAppHealth"
    computer_use: dict[str, object] = {
        "enabled": True,
        "adapter": "native-browser",
        "browserHeadless": True,
        "maxBrowserSessions": 3,
        "timeoutSeconds": 20,
    }
    if args.browser:
        computer_use["browserExecutable"] = str(args.browser.resolve())
    home.mkdir(parents=True, exist_ok=True)
    (home / "settings.json").write_text(
        json.dumps({"computer-use": computer_use}, ensure_ascii=False), encoding="utf-8"
    )
    fixture = ThreadingHTTPServer(("127.0.0.1", 0), PageFixture)
    threading.Thread(target=fixture.serve_forever, daemon=True).start()
    evidence: dict[str, object] = {}
    try:
        with running_fixture_host(args.binary.resolve(), run, env, None, "computer-use") as port:
            sequence = 0

            def call(method: str, payload: dict[str, object]) -> dict[str, object]:
                nonlocal sequence
                sequence += 1
                return require_ok(rpc(port, method, payload, sequence), method)

            created = call("workspace.create", {"path": str(workspace)})["workspace"]
            first_owner = call("session.create", {"workspaceId": created["workspaceId"]})["sessionId"]
            second_owner = call("session.create", {"workspaceId": created["workspaceId"]})["sessionId"]
            meta = browser_request(port, "meta", {"ownerSessionId": first_owner})
            assert meta["enabled"] is True and meta["adapter"] == "native-browser", meta
            assert meta["defaultBrowserSessionId"] == "default", meta

            base = f"http://127.0.0.1:{fixture.server_port}"
            navigated = browser_request(port, "action", {
                "ownerSessionId": first_owner,
                "action": "navigate",
                "url": f"{base}/first",
                "waitMs": 300,
            })
            assert navigated["state"]["title"] == "ready", navigated
            screenshot = base64.b64decode(navigated["screenshot"]["base64"])
            assert screenshot.startswith(b"\x89PNG\r\n\x1a\n") and len(screenshot) > 1000

            clicked = browser_request(port, "action", {
                "ownerSessionId": first_owner, "browserSessionId": "default",
                "action": "click", "x": 60, "y": 45, "waitMs": 100,
            })
            assert clicked["state"]["title"] == "clicked", clicked
            typed = browser_request(port, "action", {
                "ownerSessionId": first_owner, "browserSessionId": "default",
                "action": "type", "x": 60, "y": 120, "text": "hello", "waitMs": 100,
            })
            assert typed["state"]["activeElement"]["value"] == "hello", typed
            assert typed["control"]["mode"] == "manual", typed
            keyboard = browser_request(port, "action", {"ownerSessionId": first_owner,"action":"key","keys":["Control","a"]})
            keyboard = browser_request(port, "action", {"ownerSessionId": first_owner,"action":"key","keys":["Backspace"]})
            assert keyboard["state"]["activeElement"]["value"] == "", keyboard
            secret = browser_request(port, "action", {"ownerSessionId":first_owner,"action":"type","x":60,"y":180,"text":"private-input-check"})
            assert secret["state"]["activeElement"]["value"] is None, secret["state"]
            assert "private-input-check" not in json.dumps(secret["state"])
            dragged = browser_request(port,"action",{"ownerSessionId":first_owner,"action":"drag","x":60,"y":250,"endX":280,"endY":300})
            assert dragged["state"]["title"] == "dragged", dragged["state"]
            resumed = browser_request(port,"action",{"ownerSessionId":first_owner,"action":"resume_agent"})
            assert resumed["control"]["mode"] == "agent", resumed
            manual = browser_request(port,"action",{"ownerSessionId":first_owner,"action":"takeover"})
            assert manual["control"]["mode"] == "manual", manual
            scrolled = browser_request(port, "action", {
                "ownerSessionId": first_owner, "browserSessionId": "default",
                "action": "scroll", "x": 100, "y": 300, "deltaY": 700, "waitMs": 150,
            })
            assert scrolled["state"]["scrollY"] > 0, scrolled

            other_sessions = browser_request(port, "action", {
                "ownerSessionId": second_owner, "action": "list_sessions",
            })
            assert other_sessions["sessions"] == [], other_sessions
            blocked = browser_request(port, "action", {
                "ownerSessionId": second_owner, "browserSessionId": "default", "action": "status",
            }, expected_status=404)
            assert blocked["error"] == "COMPUTER_USE_SESSION_NOT_FOUND", blocked

            second_page = browser_request(port, "action", {
                "ownerSessionId": second_owner, "browserSessionId": "default",
                "action": "navigate", "url": f"{base}/second", "waitMs": 300,
            })
            assert second_page["state"]["title"] == "second", second_page
            profile_root = home / "cache" / "computer-use"
            assert len(list(profile_root.glob("session-*"))) == 2

            crash_owner = call("session.create", {"workspaceId": created["workspaceId"]})["sessionId"]
            profiles_before_crash_session = set(profile_root.glob("session-*"))
            crash_page = browser_request(port, "action", {
                "ownerSessionId": crash_owner,
                "action": "navigate",
                "url": f"{base}/first",
                "waitMs": 300,
            })
            assert crash_page["state"]["title"] == "ready", crash_page
            crash_profiles = set(profile_root.glob("session-*")) - profiles_before_crash_session
            assert len(crash_profiles) == 1, crash_profiles
            crash_profile = crash_profiles.pop()
            killed_browser_pids = terminate_browser_profile(crash_profile)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and crash_profile.exists():
                time.sleep(0.05)
            assert not crash_profile.exists(), list(profile_root.glob("session-*"))
            cold_meta=browser_request(port,"meta",{"ownerSessionId":crash_owner})
            assert cold_meta["enabled"] and cold_meta["available"], cold_meta
            call("workspace.archiveSession", {"sessionId": crash_owner})
            call("workspace.deleteArchivedSession", {"sessionId": crash_owner})

            call("workspace.archiveSession", {"sessionId": first_owner})
            call("workspace.deleteArchivedSession", {"sessionId": first_owner})
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and len(list(profile_root.glob("session-*"))) != 1:
                time.sleep(0.05)
            assert len(list(profile_root.glob("session-*"))) == 1, list(profile_root.glob("*"))
            surviving = browser_request(port, "action", {
                "ownerSessionId": second_owner, "browserSessionId": "default", "action": "status",
            })
            assert surviving["state"]["title"] == "second", surviving
            retired = browser_request(port, "meta", {"ownerSessionId": first_owner}, expected_status=403)
            assert retired["error"] == "session-not-found", retired

            closed = browser_request(port, "action", {
                "ownerSessionId": second_owner, "browserSessionId": "default", "action": "close",
            })
            assert closed["closed"] is True and not list(profile_root.glob("session-*")), closed
            evidence = {
                "status": "passed",
                "adapter": meta["adapter"],
                "actions": ["navigate", "click", "type", "key", "drag", "takeover", "resume_agent", "scroll", "status", "close"],
                "passwordStateRedacted": True,
                "coldSessionControl": True,
                "screenshotBytes": len(screenshot),
                "ownerIsolation": True,
                "sharedDefaultSession": meta["defaultBrowserSessionId"],
                "ownerRetirementCleanup": True,
                "unexpectedExitCleanup": True,
                "unexpectedExitOwnerIdle": True,
                "killedBrowserProcessCount": len(killed_browser_pids),
                "shutdownCleanup": "covered by native adapter test",
                "nodeRequired": False,
            }
    finally:
        fixture.shutdown()
        fixture.server_close()

    unavailable_run = run / "unavailable"
    unavailable_home = unavailable_run / "home"
    unavailable_workspace = unavailable_run / "workspace"
    unavailable_workspace.mkdir(parents=True)
    unavailable_home.mkdir(parents=True)
    missing_browser = unavailable_run / "missing-browser.exe"
    (unavailable_home / "settings.json").write_text(
        json.dumps(
            {
                "computer-use": {
                    "enabled": True,
                    "adapter": "native-browser",
                    "browserExecutable": str(missing_browser),
                    "browserHeadless": True,
                    "maxBrowserSessions": 1,
                    "timeoutSeconds": 5,
                }
            },
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )
    unavailable_env = isolated_environment(unavailable_run, unavailable_home)
    with running_fixture_host(
        args.binary.resolve(), unavailable_run, unavailable_env, None, "computer-use-unavailable"
    ) as port:
        unavailable_sequence = 0

        def unavailable_call(method: str, payload: dict[str, object]) -> dict[str, object]:
            nonlocal unavailable_sequence
            unavailable_sequence += 1
            return require_ok(rpc(port, method, payload, unavailable_sequence), method)

        workspace_row = unavailable_call(
            "workspace.create", {"path": str(unavailable_workspace)}
        )["workspace"]
        owner = unavailable_call(
            "session.create", {"workspaceId": workspace_row["workspaceId"]}
        )["sessionId"]
        meta = browser_request(port, "meta", {"ownerSessionId": owner})
        assert meta["enabled"] is True and meta["available"] is False, meta
        assert meta["error"]["code"] == "COMPUTER_USE_BROWSER_NOT_FOUND", meta
        unavailable = browser_request(
            port,
            "action",
            {"ownerSessionId": owner, "action": "start"},
            expected_status=400,
        )
        assert unavailable["error"] == "COMPUTER_USE_BROWSER_NOT_FOUND", unavailable
    evidence["unavailableBrowserColdStart"] = True
    evidence["unavailableBrowserError"] = "COMPUTER_USE_BROWSER_NOT_FOUND"
    proof = run / "computer-use-evidence.json"
    proof.write_text(json.dumps(evidence, indent=2, ensure_ascii=False), encoding="utf-8")
    print(json.dumps({**evidence, "evidence": str(proof)}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
