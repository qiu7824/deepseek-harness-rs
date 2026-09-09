"""Inspect UU capture transport and image decoding without desktop input.

Requires Pillow. Remote-screen correspondence needs an independent reference
and is not established by this probe.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import subprocess
import time
import warnings

try:
    from PIL import Image, ImageStat
except ImportError as error:
    raise RuntimeError("UU image verification requires Pillow (python -m pip install Pillow)") from error

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc
from e2e_computer_use import browser_request


MAX_JPEG_BYTES = 8 * 1024 * 1024
MAX_IMAGE_DIMENSION = 16_384
MAX_IMAGE_PIXELS = 16_000_000
CAPTURE_SAMPLES = 3


def valid_dimensions(width, height) -> bool:
    return (
        type(width) is int and type(height) is int
        and 0 < width <= MAX_IMAGE_DIMENSION
        and 0 < height <= MAX_IMAGE_DIMENSION
        and width * height <= MAX_IMAGE_PIXELS
    )


def inspect_capture(result: dict) -> tuple[bytes, dict]:
    """Decode a bounded JPEG; color statistics never determine validity."""
    state = result.get("state", {})
    screenshot = result.get("screenshot", {})
    if state.get("interactive") is not True or screenshot.get("mediaType") != "image/jpeg":
        raise ValueError("Capture must contain an interactive state and an image/jpeg screenshot")
    viewport = state.get("viewport", {})
    if not valid_dimensions(viewport.get("width"), viewport.get("height")):
        raise ValueError("Capture viewport dimensions are missing or exceed the image limits")
    encoded = screenshot.get("base64")
    if not isinstance(encoded, str) or not encoded or len(encoded) > 4 * ((MAX_JPEG_BYTES + 2) // 3):
        raise ValueError("Capture JPEG payload is empty or exceeds the byte limit")
    try:
        data = base64.b64decode(encoded, validate=True)
    except (ValueError, UnicodeError) as error:
        raise ValueError("Capture JPEG payload is not valid Base64") from error
    if not data or len(data) > MAX_JPEG_BYTES:
        raise ValueError("Capture JPEG payload is empty or exceeds the byte limit")
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("error", Image.DecompressionBombWarning)
            with Image.open(io.BytesIO(data)) as image:
                if image.format != "JPEG":
                    raise ValueError("Capture payload is not a JPEG image")
                width, height = image.size
                if not valid_dimensions(width, height):
                    raise ValueError("Decoded JPEG dimensions exceed the image limits")
                if (width, height) != (viewport["width"], viewport["height"]):
                    raise ValueError("Decoded JPEG dimensions do not match the viewport")
                for key, actual in (("width", width), ("height", height)):
                    if key in screenshot and (type(screenshot[key]) is not int or screenshot[key] != actual):
                        raise ValueError("Screenshot dimensions do not match the decoded JPEG")
                image.load()
                rgb = image.convert("RGB")
                extrema = rgb.getextrema()
                statistics = ImageStat.Stat(rgb)
                metadata = {
                    "format": "JPEG", "width": width, "height": height,
                    "encodedBytes": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                    "rgbSha256": hashlib.sha256(rgb.tobytes()).hexdigest(),
                    "uniformity": {
                        "uniformColor": all(low == high for low, high in extrema),
                        "allBlack": all(low == high == 0 for low, high in extrema),
                        "rgbExtrema": [list(bounds) for bounds in extrema],
                        "meanRgb": statistics.mean,
                        "stddevRgb": statistics.stddev,
                    },
                }
    except (OSError, Image.DecompressionBombError, Image.DecompressionBombWarning) as error:
        raise ValueError("Capture JPEG could not be completely decoded within the image limits") from error
    return data, metadata


def capture_samples(act, directory: Path, phase: str, *, sleep=time.sleep) -> list[dict]:
    samples = []
    started = time.monotonic()
    for index in range(1, CAPTURE_SAMPLES + 1):
        requested = time.monotonic()
        data, metadata = inspect_capture(act("capture"))
        prefix = "desktop" if phase == "initial" else "reconnect"
        name = f"{prefix}.jpg" if index == 1 else f"{prefix}-{index:02d}.jpg"
        path = directory / name
        path.write_bytes(data)
        samples.append({
            **metadata, "phase": phase, "sample": index, "image": str(path),
            "elapsedMs": round((time.monotonic() - started) * 1000),
            "captureDurationMs": round((time.monotonic() - requested) * 1000),
        })
        if index < CAPTURE_SAMPLES:
            sleep(0.5)
    return samples


def capture_evidence(initial: list[dict], reconnected: list[dict], video: dict) -> dict:
    if len(initial) < CAPTURE_SAMPLES or len(reconnected) < CAPTURE_SAMPLES:
        raise ValueError("Initial connection and reconnect each require at least three decoded captures")
    return {
        "status": "partial", "adapter": "uu-desktop", "desktopInputSent": False,
        "transport": {
            "status": "passed", "scope": "Host GUI capture and device lease lifecycle",
            "ownerIsolation": True, "closeReleasesDevice": True, "reconnect": True,
        },
        "imageDecode": {
            "status": "passed", "initialSamples": len(initial),
            "reconnectSamples": len(reconnected), "samples": initial + reconnected,
            "sourceFrameFreshness": "unverified",
        },
        "visualCorrespondence": {
            "status": "unverified",
            "reason": "No independent remote-screen reference was compared; uniform color and black pixels are diagnostic observations only.",
        },
        "video": video,
    }


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
        video = {"status": "not-run", "decode": "not-run"}
        try:
            started = act("start", includeScreenshot=False)
            assert started["state"]["connected"] and started["control"]["mode"] == "manual", started
            initial = capture_samples(act, run, "initial")
            if args.video_node:
                probe = subprocess.run([str(args.video_node), str(Path(__file__).parent / "tests/uu_video_stream.cjs"), str(port), owner, str(run / "video")], timeout=100,
                                       capture_output=True, text=True, encoding="utf-8", creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
                print(probe.stdout, flush=True)
                assert probe.returncode == 0, probe.stderr
                video = {"status": "transport-only", "transport": "passed", "decode": "unverified",
                         "evidence": str(run / "video" / "stream.json")}
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
            reconnected = capture_samples(act, run, "reconnect")
        finally:
            act("close", includeScreenshot=False)
        evidence = capture_evidence(initial, reconnected, video)
        (run / "evidence.json").write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(evidence, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
