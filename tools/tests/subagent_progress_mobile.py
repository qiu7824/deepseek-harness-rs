"""Measure the shipped child catalog in isolated headless Chrome at phone widths."""
import argparse
import base64
import json
import pathlib
import subprocess
import time
import urllib.request
import uuid
import websocket


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    args = parser.parse_args()
    workdir = args.workdir.resolve()
    source = (workdir / "subagent-progress-mobile.html").read_text(encoding="utf-8")
    measured = workdir / "subagent-progress-mobile.html"
    results = []
    for width in (320, 390):
        profile = workdir / ("chrome-cdp-" + str(width) + "-" + uuid.uuid4().hex[:8])
        profile.mkdir()
        connection = None
        with (workdir / ("chrome-mobile-" + str(width) + ".log")).open("w", encoding="utf-8") as log:
            process = subprocess.Popen([str(args.browser.resolve()), "--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--no-proxy-server", "--hide-scrollbars", "--user-data-dir=" + str(profile), "--remote-debugging-port=0", "about:blank"], stdout=log, stderr=log, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
            try:
                deadline = time.monotonic() + 20
                port_file = profile / "DevToolsActivePort"
                while not port_file.exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
                port = int(port_file.read_text().splitlines()[0])
                with urllib.request.urlopen("http://127.0.0.1:" + str(port) + "/json/list", timeout=5) as response:
                    page = next(item for item in json.load(response) if item["type"] == "page")
                connection = websocket.create_connection(page["webSocketDebuggerUrl"], suppress_origin=True, timeout=10)
                sequence = 0

                def call(method, params):
                    nonlocal sequence
                    sequence += 1
                    connection.send(json.dumps({"id": sequence, "method": method, "params": params}))
                    while True:
                        message = json.loads(connection.recv())
                        if message.get("id") == sequence:
                            assert "error" not in message, message
                            return message.get("result", {})

                call("Emulation.setDeviceMetricsOverride", {"width": width, "height": 720, "deviceScaleFactor": 1, "mobile": True})
                call("Page.navigate", {"url": measured.as_uri()})
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    ready = call("Runtime.evaluate", {"expression": "document.readyState === 'complete' && !!document.querySelector('[role=tree]')", "returnByValue": True})
                    if ready["result"].get("value"):
                        break
                    time.sleep(0.05)
                expression = "(()=>{const r=document.querySelector('[role=tree]').getBoundingClientRect();return {width:innerWidth,left:r.left,right:r.right,top:r.top,bottom:r.bottom,scrollWidth:document.documentElement.scrollWidth}})()"
                bounds = call("Runtime.evaluate", {"expression": expression, "returnByValue": True})["result"]["value"]
                assert bounds["width"] == width, bounds
                assert bounds["left"] >= 15.5 and bounds["right"] <= width - 15.5, bounds
                assert bounds["scrollWidth"] <= width, bounds
                capture = call("Page.captureScreenshot", {"format": "png"})
                (workdir / ("subagent-progress-" + str(width) + ".png")).write_bytes(base64.b64decode(capture["data"]))
                results.append(bounds)
            finally:
                if connection is not None:
                    try:
                        connection.send(json.dumps({"id": 99999, "method": "Browser.close"}))
                    finally:
                        connection.close()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.terminate()
                    process.wait(timeout=5)
    (workdir / "subagent-progress-mobile-geometry.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
    print("PASS isolated Chrome: child catalog stays inside 320px and 390px viewports with 16px side margins")


if __name__ == "__main__":
    main()
