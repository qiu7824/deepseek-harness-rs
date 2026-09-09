from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import time
import urllib.request
from collections import Counter
from urllib.parse import urlparse

import psutil
from playwright.sync_api import Page, sync_playwright


BASE_URL = "http://127.0.0.1:58080"
CHROME = r"C:\Program Files\Google\Chrome\Application\chrome.exe"


def rpc(method: str, payload: dict[str, object], index: int) -> dict[str, object]:
    body = json.dumps(
        {
            "type": "client-request",
            "rpcId": f"browser-perf-{index}",
            "method": method,
            "payload": payload,
        },
        separators=(",", ":"),
    ).encode("utf-8")
    request = urllib.request.Request(
        f"{BASE_URL}/api/{method}",
        data=body,
        headers={"content-type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        envelope = json.load(response)
    result = envelope.get("result")
    if not isinstance(result, dict) or result.get("ok") is not True:
        raise RuntimeError(f"{method} failed: {result!r}")
    value = result.get("value")
    return value if isinstance(value, dict) else {}


def title_of(item: dict[str, object]) -> str | None:
    projections = item.get("projections")
    if not isinstance(projections, dict):
        return None
    values = projections.get("values")
    if not isinstance(values, dict):
        return None
    title = values.get("title")
    return title if isinstance(title, str) and title else None


def session_item(items: list[dict[str, object]], session_id: str) -> dict[str, object]:
    for item in items:
        if item.get("sessionId") == session_id:
            return item
    raise RuntimeError(f"target session is not listed: {session_id}")


def choose_alternate(items: list[dict[str, object]], target: dict[str, object]) -> dict[str, object]:
    titles = Counter(filter(None, (title_of(item) for item in items)))
    target_id = target["sessionId"]
    target_cwd = target.get("cwd")
    for item in items:
        title = title_of(item)
        if (
            item.get("sessionId") != target_id
            and item.get("cwd") == target_cwd
            and title is not None
            and titles[title] == 1
        ):
            return item
    for item in items:
        title = title_of(item)
        if item.get("sessionId") != target_id and title is not None and titles[title] == 1:
            return item
    raise RuntimeError("no unique-title alternate session is available")


def listener_process(port: int) -> psutil.Process:
    for connection in psutil.net_connections(kind="tcp"):
        if (
            connection.laddr
            and connection.laddr.port == port
            and connection.status == psutil.CONN_LISTEN
            and connection.pid
        ):
            return psutil.Process(connection.pid)
    raise RuntimeError(f"no listener on port {port}")


def host_sample(process: psutil.Process) -> dict[str, int]:
    memory = process.memory_info()
    return {
        "pid": process.pid,
        "working_set_bytes": memory.rss,
        "private_bytes": getattr(memory, "private", memory.vms),
        "threads": process.num_threads(),
        "handles": process.num_handles(),
    }


def collect_sample(page: Page, cdp, process: psutil.Process, label: str) -> dict[str, object]:
    cdp.send("HeapProfiler.collectGarbage")
    page.wait_for_timeout(750)
    heap = cdp.send("Runtime.getHeapUsage")
    dom = cdp.send("Memory.getDOMCounters")
    return {
        "label": label,
        **host_sample(process),
        "browser_heap_used_bytes": int(heap["usedSize"]),
        "browser_heap_total_bytes": int(heap["totalSize"]),
        "browser_documents": int(dom["documents"]),
        "browser_nodes": int(dom["nodes"]),
        "browser_event_listeners": int(dom["jsEventListeners"]),
        "page_dom_elements": int(page.evaluate("document.getElementsByTagName('*').length")),
        "turn_mark_nodes": page.locator(".dshAlpha3TurnMark").count(),
        "chat_anchor_nodes": page.locator("[data-chat-anchor-key]").count(),
    }


def expand_session_overflow(page: Page) -> None:
    buttons = page.locator('button[class*="sessionOverflowButton"]')
    for index in range(buttons.count()):
        button = buttons.nth(index)
        if button.is_visible():
            button.click()
            page.wait_for_timeout(100)


def click_session(page: Page, title: str) -> None:
    locator = page.get_by_text(title, exact=True)
    if locator.count() == 0:
        expand_session_overflow(page)
    locator = page.get_by_text(title, exact=True)
    if locator.count() == 0:
        raise RuntimeError("session title is not present in the expanded sidebar")
    for index in range(locator.count()):
        candidate = locator.nth(index)
        if candidate.is_visible():
            candidate.click(timeout=15_000)
            return
    raise RuntimeError("session title is present but not visible in the expanded sidebar")


def wait_for_history_count(counter: list[int], expected: int, timeout: float = 15.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if counter[0] >= expected:
            return
        time.sleep(0.01)
    raise RuntimeError(f"history request did not reach {expected}; observed {counter[0]}")


def run() -> dict[str, object]:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--home", required=True)
    parser.add_argument("--target-session", required=True)
    parser.add_argument("--jump-repetitions", type=int, default=100)
    parser.add_argument("--switch-repetitions", type=int, default=100)
    parser.add_argument("--output")
    args = parser.parse_args()

    binary = pathlib.Path(args.binary).resolve()
    home = pathlib.Path(args.home).resolve()
    if args.jump_repetitions < 1 or args.switch_repetitions < 1:
        raise ValueError("repetition counts must be positive")
    process = listener_process(58080)
    if pathlib.Path(process.exe()).resolve() != binary:
        raise RuntimeError("port 58080 is not owned by the requested binary")

    listed = rpc("session.list", {}, 0)
    raw_items = listed.get("sessions", listed.get("items", []))
    items = [item for item in raw_items if isinstance(item, dict)]
    target = session_item(items, args.target_session)
    alternate = choose_alternate(items, target)
    target_title = title_of(target)
    alternate_title = title_of(alternate)
    if target_title is None or alternate_title is None:
        raise RuntimeError("target or alternate session has no title projection")

    history_requests = [0]
    history_responses = [0]
    api_request_counts: Counter[str] = Counter()
    samples: list[dict[str, object]] = []
    jump_latencies: list[float] = []
    switch_latencies: list[float] = []
    jump_network_requests = 0

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(
            headless=True,
            executable_path=CHROME,
            args=["--enable-precise-memory-info", "--disable-background-timer-throttling"],
        )
        context = browser.new_context(viewport={"width": 1440, "height": 1000})
        page = context.new_page()

        def count_request(request) -> None:
            path = urlparse(request.url).path
            if "/api/" in path:
                api_request_counts[path.rsplit("/api/", 1)[-1]] += 1
            if path.endswith("/api/session.history"):
                history_requests[0] += 1

        def count_response(response) -> None:
            if response.url.endswith("/api/session.history"):
                history_responses[0] += 1

        page.on("request", count_request)
        page.on("response", count_response)
        page.goto(BASE_URL + "/", wait_until="domcontentloaded", timeout=60_000)
        page.wait_for_timeout(2500)
        expand_session_overflow(page)
        click_session(page, target_title)
        page.locator("[data-chat-flow]").wait_for(state="visible", timeout=30_000)
        wait_for_history_count(history_requests, 1)
        page.wait_for_timeout(500)

        # Warm both resident session views before the baseline, so the stress
        # phase measures repeat behavior rather than first-use allocation.
        click_session(page, alternate_title)
        page.wait_for_timeout(250)
        click_session(page, target_title)
        page.locator("[data-chat-flow]").wait_for(state="visible", timeout=30_000)
        page.wait_for_timeout(500)

        # Warm both directional historical windows before the baseline. The
        # stress comparison must not mistake first-use historical DOM/cache
        # allocation for per-click growth.
        for edge in ("oldest", "latest"):
            frame = page.locator(".dshAlpha3TurnRailFrame")
            frame.evaluate(
                "(element, edge) => { element.scrollTop = edge === 'oldest' ? 0 : element.scrollHeight; element.dispatchEvent(new Event('scroll')); }",
                edge,
            )
            page.wait_for_timeout(75)
            unloaded = page.locator(".dshAlpha3TurnMark[data-unloaded]")
            if unloaded.count() == 0:
                raise RuntimeError("warm turn navigator exposes no unloaded target")
            mark = unloaded.first if edge == "oldest" else unloaded.last
            before = history_requests[0]
            mark.click(timeout=15_000)
            page.wait_for_timeout(100)
            delta = history_requests[0] - before
            if delta > 1:
                raise RuntimeError(f"warm indexed jump issued {delta} history requests")
            if delta == 1:
                jump_network_requests += 1
            page.wait_for_function(
                "!document.querySelector('.dshAlpha3TurnMark[data-busy]')",
                timeout=15_000,
            )
            page.wait_for_timeout(350)
        page.wait_for_timeout(750)

        cdp = context.new_cdp_session(page)
        cdp.send("Performance.enable")
        cdp.send("HeapProfiler.enable")
        samples.append(collect_sample(page, cdp, process, "warmed"))

        for index in range(args.jump_repetitions):
            frame = page.locator(".dshAlpha3TurnRailFrame")
            if frame.count() == 0:
                raise RuntimeError("long session rendered no turn navigator")
            if index % 2 == 0:
                frame.evaluate("element => { element.scrollTop = 0; element.dispatchEvent(new Event('scroll')); }")
            else:
                frame.evaluate("element => { element.scrollTop = element.scrollHeight; element.dispatchEvent(new Event('scroll')); }")
            page.wait_for_timeout(50)
            unloaded = page.locator(".dshAlpha3TurnMark[data-unloaded]")
            if unloaded.count() == 0:
                raise RuntimeError("turn navigator exposes no unloaded target for indexed jump")
            mark = unloaded.first if index % 2 == 0 else unloaded.last
            before = history_requests[0]
            started = time.perf_counter()
            mark.click(timeout=15_000)
            page.wait_for_timeout(100)
            delta = history_requests[0] - before
            if delta > 1:
                raise RuntimeError(f"indexed turn jump issued {delta} history requests")
            if delta == 1:
                jump_network_requests += 1
            page.wait_for_function(
                "!document.querySelector('.dshAlpha3TurnMark[data-busy]')",
                timeout=15_000,
            )
            page.wait_for_timeout(300)
            jump_latencies.append(time.perf_counter() - started)
            if index + 1 in {20, args.jump_repetitions}:
                samples.append(collect_sample(page, cdp, process, f"jump_{index + 1}"))

        for index in range(args.switch_repetitions):
            started = time.perf_counter()
            click_session(page, alternate_title if index % 2 == 0 else target_title)
            page.wait_for_timeout(20)
            switch_latencies.append(time.perf_counter() - started)
            if index + 1 in {20, args.switch_repetitions}:
                samples.append(collect_sample(page, cdp, process, f"switch_{index + 1}"))

        if args.switch_repetitions % 2 == 1:
            click_session(page, target_title)
        page.wait_for_timeout(30_000)
        samples.append(collect_sample(page, cdp, process, "settled"))
        context.close()
        browser.close()

    baseline = samples[0]
    settled = samples[-1]
    reference_label = "switch_20" if args.switch_repetitions >= 20 else f"switch_{args.switch_repetitions}"
    reference = next(sample for sample in samples if sample["label"] == reference_label)
    if settled["page_dom_elements"] > reference["page_dom_elements"] + 250:
        raise RuntimeError(
            f"page DOM grew after stabilization: {reference['page_dom_elements']} -> {settled['page_dom_elements']}"
        )
    if settled["browser_nodes"] > reference["browser_nodes"] + 500:
        raise RuntimeError(
            f"browser DOM nodes grew after stabilization: {reference['browser_nodes']} -> {settled['browser_nodes']}"
        )
    if settled["browser_heap_used_bytes"] > reference["browser_heap_used_bytes"] + 24 * 1024 * 1024:
        raise RuntimeError("browser heap grew beyond the bounded stress allowance")
    if settled["working_set_bytes"] > reference["working_set_bytes"] + 32 * 1024 * 1024:
        raise RuntimeError("Host working set grew beyond the bounded stress allowance")
    if settled["private_bytes"] > reference["private_bytes"] + 32 * 1024 * 1024:
        raise RuntimeError("Host private bytes grew beyond the bounded stress allowance")
    max_working_set = max(sample["working_set_bytes"] for sample in samples)
    if settled["working_set_bytes"] > 50 * 1024 * 1024:
        raise RuntimeError(
            "Host Working Set exceeded the 50 MiB stable production ceiling: "
            f"{settled['working_set_bytes']}"
        )
    if settled["handles"] > reference["handles"] + 10 or settled["threads"] > reference["threads"] + 5:
        raise RuntimeError("Host handles or threads grew beyond the bounded stress allowance")
    if max(sample["turn_mark_nodes"] for sample in samples) > 56:
        raise RuntimeError("turn navigator exceeded its virtualized DOM bound")

    result = {
        "schema_version": 1,
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "home_path_sha256": hashlib.sha256(str(home).encode("utf-8")).hexdigest(),
        "target_session_id": args.target_session,
        "jump_repetitions": args.jump_repetitions,
        "switch_repetitions": args.switch_repetitions,
        "history_requests": history_requests[0],
        "history_responses": history_responses[0],
        "api_request_counts": dict(sorted(api_request_counts.items())),
        "jump_network_requests": jump_network_requests,
        "peak_host_working_set_bytes": max_working_set,
        "jump_latency_ms": {
            "max": max(jump_latencies) * 1000,
            "average": sum(jump_latencies) / len(jump_latencies) * 1000,
        },
        "switch_latency_ms": {
            "max": max(switch_latencies) * 1000,
            "average": sum(switch_latencies) / len(switch_latencies) * 1000,
        },
        "samples": samples,
    }
    if args.output:
        output = pathlib.Path(args.output).resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return result


if __name__ == "__main__":
    report = run()
    print(json.dumps(report, separators=(",", ":"), sort_keys=True))
