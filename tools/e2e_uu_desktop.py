"""Verify the installed UU SDK through the real Host without sending desktop input."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import sys
import subprocess
import time

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc
from e2e_computer_use import browser_request


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--workdir", type=Path, required=True)
    parser.add_argument("--device", required=True)
    parser.add_argument("--video-node", type=Path, help="Node.js runtime for the H.264 WebSocket integration probe")
    args = parser.parse_args()
    user_id = None
    general = False
    info = Path(os.environ["ProgramData"]) / "Netease/GameViewer/user_info.ini"
    for line in info.read_text(encoding="utf-8-sig").splitlines():
        line = line.strip()
        if line.startswith("["):
            general = line == "[General]"
        elif general and "=" in line:
            key, value = line.split("=", 1)
            if key == "userId":
                user_id = value.strip().strip('"')
    if not user_id:
        raise RuntimeError("UU account is not signed in")
    run = args.workdir.resolve() / f"run-{int(time.time() * 1000)}"
    home, workspace = run / "home", run / "workspace"
    workspace.mkdir(parents=True)
    home.mkdir()
    env = isolated_environment(run, home)
    (home / "settings.json").write_text(json.dumps({
        "computer-use": {"enabled": True, "adapter": "uu-desktop", "timeoutSeconds": 45},
        "uu-remote": {"deviceId": args.device, "account": hashlib.sha256(user_id.encode()).hexdigest()},
    }), encoding="utf-8")
    sequence = 0
    with running_fixture_host(args.binary.resolve(), run, env, None, "uu-desktop") as port:
        def call(method, payload):
            nonlocal sequence
            sequence += 1
            return require_ok(rpc(port, method, payload, sequence), method)
        workspace_id = call("workspace.create", {"path": str(workspace)})["workspace"]["workspaceId"]
        owner = call("session.create", {"workspaceId": workspace_id, "agentPreset": "minimal"})["sessionId"]
        other = call("session.create", {"workspaceId": workspace_id, "agentPreset": "minimal"})["sessionId"]
        payload = {"ownerSessionId": owner, "browserSessionId": "default"}
        def act(action, **extra):
            return browser_request(port, "action", {**payload, "action": action, **extra})
        meta = browser_request(port, "meta", {"ownerSessionId": owner})
        assert meta["adapter"] == "uu-desktop" and meta["available"], meta
        try:
            started = act("start", includeScreenshot=False)
            assert started["state"]["connected"] and started["control"]["mode"] == "manual", started
            if args.video_node:
                probe = subprocess.run([str(args.video_node), str(Path(__file__).parent / "tests/uu_video_stream.cjs"), str(port), owner, str(run / "video")], timeout=100,
                                       capture_output=True, text=True, encoding="utf-8", creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
                print(probe.stdout, flush=True)
                assert probe.returncode == 0, probe.stderr
            result = act("capture")
            assert result["state"]["interactive"] and result["screenshot"]["mediaType"] == "image/jpeg"
            pixels = base64.b64decode(result["screenshot"]["base64"], validate=True)
            (run / "desktop.jpg").write_bytes(pixels)
            denied = browser_request(port, "action", {"ownerSessionId": other, "action": "start"}, expected_status=409)
            assert denied["error"] == "COMPUTER_USE_DEVICE_BUSY", denied
            held = act("takeover", includeScreenshot=False)
            assert held["control"]["mode"] == "manual"
            status = act("status", includeScreenshot=False)
            assert status["state"]["connected"]
        finally:
            act("close", includeScreenshot=False)
        # A failed/closed environment must not retain the physical device lease.
        payload["ownerSessionId"] = other
        try:
            reopened = act("start", includeScreenshot=False)
            assert reopened["state"]["connected"]
            image = act("capture")
            assert image["state"]["interactive"]
        finally:
            act("close", includeScreenshot=False)
        evidence = {"status": "passed", "adapter": "uu-desktop", "realScreenshotBytes": len(pixels),
                    "ownerIsolation": True, "closeReleasesDevice": True, "reconnect": True,
                    "desktopInputSent": False, "image": str(run / "desktop.jpg")}
        (run / "evidence.json").write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(evidence, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
