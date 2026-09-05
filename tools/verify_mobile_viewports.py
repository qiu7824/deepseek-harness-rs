"""Measure source-component fixtures in an isolated Chromium viewport."""
import argparse
import functools
import http.server
import json
import os
from pathlib import Path
import subprocess
import threading
import time
import urllib.request

import websocket


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixtures", type=Path, required=True)
    parser.add_argument("--browser", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    profile = args.output / "browser-profile"
    if profile.exists():
        raise SystemExit("Use a fresh output directory for isolated browser verification")
    profile.mkdir()
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(args.fixtures))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    browser_log = (args.output / "browser.log").open("wb")
    process = subprocess.Popen([str(args.browser), "--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "--remote-debugging-port=0", "--remote-allow-origins=http://localhost", "--user-data-dir=" + str(profile), "about:blank"], stdout=browser_log, stderr=browser_log, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    connection = None
    try:
        port_file = profile / "DevToolsActivePort"
        deadline = time.monotonic() + 20
        while not port_file.exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        port = int(port_file.read_text().splitlines()[0])
        with urllib.request.urlopen("http://127.0.0.1:%d/json/list" % port) as response:
            target = next(item for item in json.load(response) if item["type"] == "page")
        connection = websocket.create_connection(target["webSocketDebuggerUrl"], origin="http://localhost", timeout=20)
        sequence = 0

        def command(method, params=None):
            nonlocal sequence
            sequence += 1
            connection.send(json.dumps({"id": sequence, "method": method, "params": params or {}}))
            while True:
                reply = json.loads(connection.recv())
                if reply.get("id") == sequence:
                    if "error" in reply:
                        raise RuntimeError(reply["error"])
                    return reply.get("result", {})

        fixtures = json.loads((args.fixtures / "fixtures.json").read_text())
        report = []
        for fixture in fixtures:
            widths = [320, 375, 390, 430, 1280] if fixture["name"] in ("appearance", "memory-editor", "model-editor", "skill-editor") else [fixture["width"]]
            for width in widths:
                command("Emulation.setDeviceMetricsOverride", {"width": width, "height": 844, "deviceScaleFactor": 1, "mobile": width <= 768})
                command("Page.navigate", {"url": "http://127.0.0.1:%d/%s-%d.html" % (server.server_port, fixture["name"], fixture["width"])})
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    ready = command("Runtime.evaluate", {"expression": "document.readyState", "returnByValue": True})
                    if ready.get("result", {}).get("value") == "complete":
                        break
                    time.sleep(0.05)
                expression = """(() => {
                  const width=innerWidth, root=document.documentElement;
                  const visible=e=>{const r=e.getBoundingClientRect(),s=getComputedStyle(e);return r.width>0&&r.height>0&&s.visibility!=='hidden'&&s.display!=='none'&&!e.closest('._7h7_Oq_navList')&&!e.closest('.j9qSJG_frame:not([data-mobile-details-open])[data-details-collapsed] .j9qSJG_detailsCol');};
                  const overflow=[...document.querySelectorAll('button,input,textarea,select,._l4_0G_card,.dshMemoryItem,.dshCapsEditor')].filter(visible).map(e=>({tag:e.tagName,cls:e.className,text:(e.textContent||e.getAttribute('aria-label')||'').slice(0,50),left:e.getBoundingClientRect().left,right:e.getBoundingClientRect().right})).filter(r=>r.left<-.5||r.right>width+.5);
                  const panel=document.querySelector('._7h7_Oq_panel'), center=document.querySelector('.j9qSJG_centerCol'), details=document.querySelector('[data-mobile-details-open] .j9qSJG_detailsCol');
                  return {viewport:width,documentWidth:root.scrollWidth,overflow,panelWidth:panel?.getBoundingClientRect().width,centerWidth:center?.getBoundingClientRect().width,detailsWidth:details?.getBoundingClientRect().width,settingsFlow:document.querySelector('._7h7_Oq_navList')?getComputedStyle(document.querySelector('._7h7_Oq_navList')).flexDirection:null};
                })()"""
                result = command("Runtime.evaluate", {"expression": expression, "returnByValue": True})["result"]["value"]
                result.update(fixture=fixture["name"], width=width)
                report.append(result)
                assert result["documentWidth"] <= width, result
                assert not result["overflow"], result
                if result.get("panelWidth") is not None:
                    assert result["settingsFlow"] == ("row" if width <= 768 else "column"), result
                if result.get("detailsWidth") is not None:
                    assert result["detailsWidth"] >= width - 57, result
                if result.get("centerWidth") is not None and width <= 768:
                    assert result["centerWidth"] >= width - 57, "Overlay must keep the conversation in grid column 2: %r" % result
                if width == 390 and fixture["name"] in ("appearance", "memory-editor", "conversation", "model-editor"):
                    import base64
                    image = command("Page.captureScreenshot", {"format": "png"})["data"]
                    (args.output / (fixture["name"] + ".png")).write_bytes(base64.b64decode(image))
        (args.output / "viewport-results.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print("PASS %d Chromium viewport cases; no horizontal control overflow; mobile settings navigation and details panel accessible" % len(report))
    finally:
        if connection:
            try:
                connection.send(json.dumps({"id": 999999, "method": "Browser.close"}))
            except websocket.WebSocketException:
                pass
            connection.close()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
        server.shutdown()
        browser_log.close()


if __name__ == "__main__":
    main()
