"""Verify live skill catalogs, on-demand loading, updates, and revocation through a real Host."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc


SKILL = "fixture-live-skill"
UNLOADED = "fixture-unloaded-skill"
SLASH_SKILL = "fixture-slash-skill"
DESCRIPTION = "Inspect the isolated live skill fixture."
BODY = {version: "SKILL_BODY_VERSION_" + version for version in ("V1", "V2", "V3", "V4")}
SLASH_BODY = {version: "SLASH_BODY_VERSION_" + version for version in ("V1", "V2")}
UNLOADED_BODY = "UNLOADED_SKILL_PRIVATE_BODY"
MODELS = [{"id": "skill-fixture-model", "contextWindow": 65536, "maxTokens": 2048}]


def skill_document(name, marker):
    return "---\nname: " + name + "\ndescription: " + DESCRIPTION + "\n---\n\n# Fixture procedure\n\n" + marker + "\n"


def message_text(request):
    parts = []
    for message in request.get("messages", []):
        content = message.get("content")
        if isinstance(content, str):
            parts.append(content)
        elif isinstance(content, list):
            parts.extend(block.get("text", "") for block in content if isinstance(block, dict))
    return "\n".join(parts)


def latest_catalog(request):
    catalogs = re.findall(r"<available_skills>(.*?)</available_skills>", message_text(request), re.S)
    return catalogs[-1] if catalogs else ""


def state_blocks(request, name, status):
    pattern = r'<skill_state\b(?=[^>]*\bname="' + re.escape(name) + r'")(?=[^>]*\bstatus="' + re.escape(status) + r'")[^>]*>.*?</skill_state>'
    return re.findall(pattern, message_text(request), re.S)


class SkillFixture(BaseHTTPRequestHandler):
    phase = "initial"
    call_skill = False
    called_phases = set()
    records = []

    def log_message(self, *_args):
        pass

    def respond(self, value, content_type="application/json"):
        body = (value if isinstance(value, str) else json.dumps(value)).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def do_GET(self):
        self.respond({"data": MODELS})

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        exposed = [tool.get("function", {}).get("name") for tool in request.get("tools", [])]
        primary = "skill" in exposed
        phase = SkillFixture.phase
        SkillFixture.records.append({"phase": phase, "primary": primary, "request": request})
        delta = {"role": "assistant", "content": "Skill verification complete."}
        finish = "stop"
        if primary and SkillFixture.call_skill and phase not in SkillFixture.called_phases:
            SkillFixture.called_phases.add(phase)
            delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": "skill-call-" + uuid.uuid4().hex, "type": "function", "function": {"name": "skill", "arguments": json.dumps({"name": SKILL})}}]}
            finish = "tool_calls"
        events = [
            {"id": "fixture-" + uuid.uuid4().hex, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": {"prompt_tokens": 256, "completion_tokens": 64, "total_tokens": 320}},
        ]
        self.respond("".join("data: " + json.dumps(event) + "\n\n" for event in events) + "data: [DONE]\n\n", "text/event-stream")


class Client:
    def __init__(self, port):
        self.port, self.sequence = port, 0

    def call(self, method, payload):
        self.sequence += 1
        return require_ok(rpc(self.port, method, payload, self.sequence), method)

    def create_session(self, workspace_id):
        session = self.call("session.create", {"workspaceId": workspace_id, "agentPreset": "standard"})["sessionId"]
        self.call("session.selectModel", {"sessionId": session, "provider": "skill-fixture", "model": MODELS[0]["id"]})
        return session

    def turn(self, session, phase, call_skill=False, prompt=None):
        previous = self.call("session.history", {"sessionId": session})["events"]
        previous_ends = sum(item["event"]["type"] == "turn/end" for item in previous)
        SkillFixture.phase, SkillFixture.call_skill = phase, call_skill
        self.call("session.prompt", {"sessionId": session, "content": [{"type": "text", "text": prompt or "skill-e2e:" + phase}], "mode": "queue"})
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            history = self.call("session.history", {"sessionId": session})
            events = [item["event"] for item in history["events"]]
            ends = [event for event in events if event["type"] == "turn/end"]
            if len(ends) > previous_ends:
                assert ends[-1]["data"]["reason"]["kind"] == "completed", ends[-1]
                records = [row["request"] for row in SkillFixture.records if row["phase"] == phase and row["primary"]]
                assert records, "no real model request exposing the skill loader in " + phase
                return records, events[len(previous):]
            time.sleep(0.05)
        raise AssertionError(phase + ": model turn did not finish")


def tool_results(events):
    return [event for event in events if event["type"] == "tool/result"]


def loader_result(events, marker, is_error):
    results = [block for event in tool_results(events) for block in event.get("data", {}).get("message", {}).get("content", []) if block.get("type") == "tool-result"]
    return len(results) == 1 and results[0].get("isError") is is_error and marker in json.dumps(results[0], ensure_ascii=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    parser.add_argument("--observe-only", action="store_true", help="Collect all unmet requirements for an older binary; the report never labels them as passed.")
    args = parser.parse_args()
    binary, workdir = args.binary.resolve(), args.workdir.resolve()
    if not binary.is_file():
        raise ValueError("binary does not exist")
    workdir.mkdir(parents=True, exist_ok=True)
    home = workdir / "ui-home"
    if home.exists():
        raise ValueError("choose a fresh isolated workdir")
    env = isolated_environment(workdir, home)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    workspace = workdir / "project"
    workspace.mkdir()
    server = ThreadingHTTPServer(("127.0.0.1", 0), SkillFixture)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {"llm-pi-ai": {"providers": {"skill-fixture": {"keyless": True, "api": "openai-completions", "baseURL": "http://127.0.0.1:" + str(server.server_port) + "/v1", "models": MODELS}}}, "agent-default-model": {"provider": "skill-fixture", "model": MODELS[0]["id"]}}
    (home / "settings.json").write_text(json.dumps(settings), encoding="utf-8")
    report = {"binary": str(binary), "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "observeOnly": args.observe_only, "checks": [], "phases": {}, "fixtureOnly": True}
    checks = report["checks"]

    def check(name, condition, evidence=None):
        row = {"name": name, "passed": bool(condition)}
        if evidence is not None:
            row["evidence"] = evidence
        checks.append(row)

    def turn(client, session, phase, call_skill=False, prompt=None):
        requests, events = client.turn(session, phase, call_skill, prompt)
        report["phases"][phase] = {
            "requestCount": len(requests),
            "bodyCounts": [{marker: message_text(request).count(marker) for marker in list(BODY.values()) + list(SLASH_BODY.values()) + [UNLOADED_BODY]} for request in requests],
            "latestCatalog": latest_catalog(requests[-1]),
            "toolResults": tool_results(events),
            "stateNotices": re.findall(r"<skill_state\b.*?</skill_state>", message_text(requests[-1]), re.S),
        }
        for request in requests:
            names = [tool.get("function", {}).get("name") for tool in request.get("tools", [])]
            check(phase + ": one generic skill loader", names.count("skill") == 1 and all(name not in names for name in (SKILL, UNLOADED, SLASH_SKILL)))
        return requests, events

    fatal = None
    try:
        with running_fixture_host(binary, workdir, env, None, "skills") as port:
            client = Client(port)
            workspace_id = client.call("workspace.create", {"path": str(workspace)})["workspace"]["workspaceId"]
            session = client.create_session(workspace_id)
            report["sessionId"] = session
            initial, _ = turn(client, session, "initial")
            check("isolated session has no fixture skills", SKILL not in latest_catalog(initial[-1]) and all(marker not in message_text(initial[-1]) for marker in BODY.values()))

            client.call("capabilities.skillSave", {"name": SKILL, "content": skill_document(SKILL, BODY["V1"])})
            added, _ = turn(client, session, "added")
            check("new skill enters the live summary catalog", SKILL in latest_catalog(added[-1]) and DESCRIPTION in latest_catalog(added[-1]))
            check("catalog discovery does not load full instructions", BODY["V1"] not in message_text(added[-1]))
            loaded, events = turn(client, session, "loaded", True)
            check("skill tool loads the requested full body", len(loaded) == 2 and BODY["V1"] in message_text(loaded[-1]) and loader_result(events, BODY["V1"], False))
            check("body remains absent before explicit loading", BODY["V1"] not in message_text(loaded[0]))
            repeated, _ = turn(client, session, "unchanged")
            check("unchanged body is not reinjected", message_text(repeated[-1]).count(BODY["V1"]) == message_text(loaded[-1]).count(BODY["V1"]) == 1)

            client.call("capabilities.skillSave", {"name": SKILL, "content": skill_document(SKILL, BODY["V2"]), "overwrite": True})
            updated, _ = turn(client, session, "body-updated")
            check("UI body-only save reaches the next request", BODY["V2"] in message_text(updated[0]))
            check("UI update explicitly replaces earlier instructions", any(BODY["V2"] in block for block in state_blocks(updated[0], SKILL, "active")))
            check("body-only save keeps its original summary", latest_catalog(updated[0]) == latest_catalog(added[-1]))
            updated_repeat, _ = turn(client, session, "body-updated-unchanged")
            check("updated body is injected only once", message_text(updated_repeat[-1]).count(BODY["V2"]) == message_text(updated[-1]).count(BODY["V2"]) == 1)

            path = home / "capabilities" / "skills" / SKILL / "SKILL.md"
            before_stat = path.stat()
            before_bytes = path.read_bytes()
            after_bytes = before_bytes.replace(BODY["V2"].encode(), BODY["V3"].encode())
            assert before_bytes != after_bytes and len(before_bytes) == len(after_bytes), "same-size external edit fixture must change the body"
            path.write_bytes(after_bytes)
            os.utime(path, ns=(before_stat.st_atime_ns, before_stat.st_mtime_ns))
            after_stat = path.stat()
            assert (before_stat.st_size, before_stat.st_mtime_ns) == (after_stat.st_size, after_stat.st_mtime_ns), "external edit fixture must preserve size and mtime"
            report["sameMetadataEdit"] = {"size": after_stat.st_size, "mtimeNs": after_stat.st_mtime_ns, "beforeSha256": hashlib.sha256(before_bytes).hexdigest(), "afterSha256": hashlib.sha256(after_bytes).hexdigest()}
            disk, _ = turn(client, session, "disk-body-updated")
            check("same-size same-mtime disk edit reaches the next request", BODY["V3"] in message_text(disk[0]))
            check("disk update explicitly replaces earlier instructions", any(BODY["V3"] in block for block in state_blocks(disk[0], SKILL, "active")))
            disk_repeat, _ = turn(client, session, "disk-body-unchanged")
            check("disk-updated body is injected only once", message_text(disk_repeat[-1]).count(BODY["V3"]) == message_text(disk[-1]).count(BODY["V3"]) == 1)

            client.call("capabilities.skillSave", {"name": UNLOADED, "content": skill_document(UNLOADED, UNLOADED_BODY)})
            second, _ = turn(client, session, "second-added")
            check("another skill enters the live catalog", SKILL in latest_catalog(second[0]) and UNLOADED in latest_catalog(second[0]))
            check("an unrequested skill body is never injected", UNLOADED_BODY not in message_text(second[-1]))

            client.call("capabilities.skillToggle", {"name": SKILL, "enabled": False})
            disabled, events = turn(client, session, "disabled", True)
            check("disabled skill leaves the latest catalog", SKILL not in latest_catalog(disabled[0]) and UNLOADED in latest_catalog(disabled[0]))
            check("disabled loader call fails", loader_result(events, "unknown or no longer available", True))
            check("disabled loaded body receives an inactive notice", bool(state_blocks(disabled[0], SKILL, "inactive")))
            disabled_repeat, _ = turn(client, session, "disabled-unchanged")
            check("inactive notice is not repeated on unchanged requests", len(state_blocks(disabled_repeat[0], SKILL, "inactive")) == len(state_blocks(disabled[0], SKILL, "inactive")) == 1)

            client.call("capabilities.skillToggle", {"name": SKILL, "enabled": True})
            reenabled, _ = turn(client, session, "reenabled")
            check("reenabled skill returns to the live catalog", SKILL in latest_catalog(reenabled[0]))
            check("reenabling restores the current body explicitly", any(BODY["V3"] in block for block in state_blocks(reenabled[0], SKILL, "active")) and message_text(reenabled[0]).rfind('status="active"') > message_text(reenabled[0]).rfind('status="inactive"'))
            reloaded, events = turn(client, session, "reloaded", True)
            check("reenabled loader reads the current disk body", len(reloaded) == 2 and loader_result(events, BODY["V3"], False))

            client.call("capabilities.skillRemove", {"name": SKILL})
            removed, events = turn(client, session, "removed", True)
            check("removed skill leaves the latest catalog", SKILL not in latest_catalog(removed[0]))
            check("removed loader call fails", loader_result(events, "unknown or no longer available", True))
            check("removal revokes the latest loaded body", len(state_blocks(removed[0], SKILL, "inactive")) == 2 and message_text(removed[0]).rfind('status="inactive"') > message_text(removed[0]).rfind('status="active"'))
            history = client.call("session.history", {"sessionId": session})
            (workdir / "skill-session-history.json").write_text(json.dumps(history, ensure_ascii=False, indent=2), encoding="utf-8")
            check("historical original body remains durable", BODY["V1"] in json.dumps(history))
            check("later requests retain history with explicit current state", BODY["V1"] in message_text(removed[-1]) and bool(state_blocks(removed[-1], SKILL, "inactive")))

            fresh_session = client.create_session(workspace_id)
            fresh, _ = turn(client, fresh_session, "fresh-session")
            check("new session contains no old loaded body", all(marker not in message_text(fresh[-1]) for marker in BODY.values()))
            check("new session has only the currently available summary", SKILL not in latest_catalog(fresh[-1]) and UNLOADED in latest_catalog(fresh[-1]) and UNLOADED_BODY not in message_text(fresh[-1]))

            client.call("capabilities.skillSave", {"name": SLASH_SKILL, "content": skill_document(SLASH_SKILL, SLASH_BODY["V1"])})
            slash_session = client.create_session(workspace_id)
            report["slashSessionId"] = slash_session
            slash, events = turn(client, slash_session, "slash-loaded", prompt="/" + SLASH_SKILL + " skill-e2e:slash-loaded")
            check("explicit slash invocation loads the full body without a tool call", len(slash) == 1 and not tool_results(events) and message_text(slash[0]).count(SLASH_BODY["V1"]) == 1)
            check("slash invocation records its own structured source", any(event.get("type") == "user/message" and event.get("data", {}).get("source", {}).get("kind") == "skill-invocation" for event in events))
            client.call("capabilities.skillSave", {"name": SLASH_SKILL, "content": skill_document(SLASH_SKILL, SLASH_BODY["V2"]), "overwrite": True})
            slash_updated, _ = turn(client, slash_session, "slash-body-updated")
            check("slash-loaded instructions receive live body replacements", any(SLASH_BODY["V2"] in block for block in state_blocks(slash_updated[0], SLASH_SKILL, "active")))
            check("slash replacement retains the historical original body", SLASH_BODY["V1"] in message_text(slash_updated[0]))
            slash_repeat, _ = turn(client, slash_session, "slash-body-unchanged")
            check("slash body replacement is injected only once", message_text(slash_repeat[0]).count(SLASH_BODY["V2"]) == message_text(slash_updated[0]).count(SLASH_BODY["V2"]) == 1)
            slash_history = client.call("session.history", {"sessionId": slash_session})
            (workdir / "skill-slash-session-history.json").write_text(json.dumps(slash_history, ensure_ascii=False, indent=2), encoding="utf-8")

            client.call("capabilities.skillSave", {"name": SKILL, "content": skill_document(SKILL, BODY["V3"])})
            prepared, _ = turn(client, session, "restart-prepared")
            check("restored skill reactivates its latest instructions", bool(state_blocks(prepared[0], SKILL, "active")) and message_text(prepared[0]).rfind('status="active"') > message_text(prepared[0]).rfind('status="inactive"'))
        with running_fixture_host(binary, workdir, env, None, "skills-restart") as port:
            client = Client(port)
            cold, _ = turn(client, session, "cold-unchanged")
            check("cold restart resumes the same persisted session", "skill-e2e:loaded" in message_text(cold[0]) and BODY["V1"] in message_text(cold[0]))
            check("cold restart restores loaded state without reinjection", message_text(cold[0]).count(BODY["V3"]) == message_text(prepared[0]).count(BODY["V3"]) and len(state_blocks(cold[0], SKILL, "active")) == len(state_blocks(prepared[0], SKILL, "active")))
            client.call("capabilities.skillSave", {"name": SKILL, "content": skill_document(SKILL, BODY["V4"]), "overwrite": True})
            cold_updated, _ = turn(client, session, "cold-body-updated")
            check("persisted loaded state observes a later body update", any(BODY["V4"] in block for block in state_blocks(cold_updated[0], SKILL, "active")))
            cold_repeat, _ = turn(client, session, "cold-body-unchanged")
            check("body updated after cold restart is injected only once", message_text(cold_repeat[0]).count(BODY["V4"]) == message_text(cold_updated[0]).count(BODY["V4"]) == 1)
            final_history = client.call("session.history", {"sessionId": session})
            (workdir / "skill-restarted-session-history.json").write_text(json.dumps(final_history, ensure_ascii=False, indent=2), encoding="utf-8")
            check("unrequested body stays unloaded through all phases", all(UNLOADED_BODY not in message_text(row["request"]) for row in SkillFixture.records if row["primary"]))
    except Exception as error:
        fatal = str(error)
        report["fatalError"] = fatal
    finally:
        server.shutdown()
        server.server_close()
        failed = [check for check in checks if not check["passed"]]
        report.update({"passed": not failed and fatal is None, "failedCheckCount": len(failed), "checkCount": len(checks), "failedChecks": [check["name"] for check in failed]})
        (workdir / "skill-model-requests.json").write_text(json.dumps(SkillFixture.records, ensure_ascii=False, indent=2), encoding="utf-8")
        evidence = workdir / "skill-injection-evidence.json"
        evidence.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    if fatal:
        raise AssertionError("skill fixture did not complete: " + fatal + "; evidence: " + str(evidence))
    if failed:
        print("OBSERVED FAILURES" if args.observe_only else "FAIL", ":", len(failed), "of", len(checks), "checks; evidence:", evidence)
        for row in failed:
            print(" - " + row["name"])
        return 0 if args.observe_only else 1
    print("PASS live skill catalog, demand loading, body refresh, revocation, and session isolation; evidence:", evidence)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
