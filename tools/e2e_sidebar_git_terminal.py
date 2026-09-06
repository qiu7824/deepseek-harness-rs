"""Exercise advanced Git preview routes and the interactive native PTY."""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import time
import urllib.parse
import urllib.request
import urllib.error

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc

ANSI = re.compile(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))")


def git(cwd: pathlib.Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(cwd), "-c", "user.name=DSH Test", "-c", "user.email=test@dsh.invalid", *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    return completed.stdout


def request(port: int, operation: str, *, query: dict[str, object] | None = None, body: dict[str, object] | None = None) -> dict[str, object]:
    suffix = "?" + urllib.parse.urlencode(query or {}) if query else ""
    url = f"http://127.0.0.1:{port}/__dsh-preview/{operation}{suffix}"
    raw = None if body is None else json.dumps(body).encode("utf-8")
    headers = {"Origin": f"http://127.0.0.1:{port}", "Sec-Fetch-Site": "same-origin"}
    if raw is not None:
        headers["Content-Type"] = "application/json"
    try:
        with urllib.request.urlopen(urllib.request.Request(url, data=raw, headers=headers), timeout=40) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as failure:
        detail = failure.read().decode("utf-8", errors="replace")
        raise AssertionError(f"{operation} failed with HTTP {failure.code}: {detail}") from failure


def process_alive(pid: int) -> bool:
    if sys.platform == "win32":
        listed = subprocess.run(
            ["tasklist", "/FI", f"PID eq {pid}", "/FO", "CSV", "/NH"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        return f'"{pid}"' in listed.stdout
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def has_output_line(text: str, expected: str) -> bool:
    return any(line.strip() == expected for line in ANSI.sub("", text).splitlines())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    parser.add_argument("--without-node", action="store_true")
    args = parser.parse_args()
    run = args.workdir.resolve() / f"run-{int(time.time() * 1000)}"
    home = run / "home"
    workspace = run / "workspace"
    main_repo = workspace / "main repository"
    nested_repo = workspace / "packages" / "nested repository"
    linked = run / "linked worktree"
    main_repo.mkdir(parents=True)
    nested_repo.mkdir(parents=True)
    git(main_repo, "init", "-q")
    git(main_repo, "checkout", "-q", "-b", "main")
    (main_repo / "tracked.txt").write_text("base\n", encoding="utf-8")
    git(main_repo, "add", "-A")
    git(main_repo, "commit", "-q", "-m", "base")
    git(main_repo, "worktree", "add", "-q", "-b", "feature/sidebar", str(linked))
    (linked / "tracked.txt").write_text("changed in linked worktree\n", encoding="utf-8")
    git(nested_repo, "init", "-q")
    (nested_repo / "untracked.txt").write_text("nested\n", encoding="utf-8")
    env = isolated_environment(run, home)
    if args.without_node:
        kept = []
        for entry in env.get("PATH", "").split(os.pathsep):
            directory = pathlib.Path(entry or ".")
            if any((directory / executable).is_file() for executable in ("node.exe", "node", "npm.cmd", "npm")):
                continue
            kept.append(entry)
        env["PATH"] = os.pathsep.join(kept)
        env["DSH_NODE_COMMAND"] = str(run / "node-not-installed.exe")
        for key in ("NODE", "NODE_PATH", "NPM_CONFIG_PREFIX"):
            env.pop(key, None)
        assert shutil.which("node", path=env["PATH"]) is None, env["PATH"]
    shutdown_pid = 0
    with running_fixture_host(args.binary.resolve(), run, env, None, "git-terminal") as port:
        sequence = 0

        def call(method: str, payload: dict[str, object]) -> dict[str, object]:
            nonlocal sequence
            sequence += 1
            return require_ok(rpc(port, method, payload, sequence), method)

        created = call("workspace.create", {"path": str(workspace)})["workspace"]
        session_id = call("session.create", {"workspaceId": created["workspaceId"]})["sessionId"]
        status = request(port, "git-status", query={"sessionId": session_id})
        assert len(status["repositories"]) == 2, status
        repository = next(item for item in status["repositories"] if item["relativePath"] == "main repository")
        target = request(port, "git-status", query={"sessionId": session_id, "repository": repository["path"], "worktree": str(linked.resolve())})
        assert target["branch"] == "feature/sidebar", target
        assert target["entries"][0]["path"] == "tracked.txt", target
        history = request(port, "git-log", query={"sessionId": session_id, "repository": repository["path"], "worktree": target["worktree"], "count": 30})
        assert history["entries"][0]["subject"] == "base", history
        diff = request(port, "git-diff", query={"sessionId": session_id, "repository": repository["path"], "worktree": target["worktree"], "path": "tracked.txt"})
        assert "changed in linked worktree" in diff["diff"], diff

        opened = request(port, "terminal-action", body={"sessionId": session_id, "action": "open", "name": "E2E PTY"})
        terminal_id = opened["id"]
        resized = request(port, "terminal-action", body={"sessionId": session_id, "action": "resize", "terminalId": terminal_id, "rows": 24, "cols": 100})
        assert resized == {"cols": 100, "resized": True, "rows": 24}, resized
        if sys.platform == "win32":
            request(port, "terminal-action", body={"sessionId": session_id, "action": "input", "terminalId": terminal_id, "text": "chcp 65001>nul\r"})
            time.sleep(0.2)
            request(port, "terminal-action", body={"sessionId": session_id, "action": "input", "terminalId": terminal_id, "text": "cd\r"})
            time.sleep(0.2)
            command = "echo DSH_INTERACTIVE_PTY_O^K_终端_✓\r"
            expected = "DSH_INTERACTIVE_PTY_OK_终端_✓"
        else:
            command = "printf 'DSH_INTERACTIVE_PTY_O''K_终端_✓\\n'\r"
            expected = "DSH_INTERACTIVE_PTY_OK_终端_✓"
        request(port, "terminal-action", body={"sessionId": session_id, "action": "input", "terminalId": terminal_id, "text": command})
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            read = request(port, "terminal-read", query={"sessionId": session_id, "terminalId": terminal_id, "count": 2000})
            # The command spelling is deliberately different from its output;
            # terminal line wrapping can split the echoed command arbitrarily.
            if expected in read["text"]:
                break
            time.sleep(0.1)
        else:
            raise AssertionError(f"interactive PTY output did not arrive: {read!r}")
        if sys.platform == "win32":
            assert str(workspace).lower() in ANSI.sub("", read["text"]).lower(), read
        listed = request(port, "terminal-list", query={"sessionId": session_id})
        assert listed["entries"][0]["status"] == "running", listed
        viewer = call("session.create", {"workspaceId": created["workspaceId"]})["sessionId"]
        assert viewer != session_id
        pinned_read = request(port, "terminal-read", query={"sessionId": session_id, "terminalId": terminal_id, "count": 2000})
        assert expected in pinned_read["text"], pinned_read
        long_command = "ping -t 127.0.0.1 >nul\r" if sys.platform == "win32" else "sleep 60\r"
        request(port, "terminal-action", body={"sessionId": session_id, "action": "input", "terminalId": terminal_id, "text": long_command})
        time.sleep(0.4)
        interrupted = request(port, "terminal-action", body={"sessionId": session_id, "action": "signal", "terminalId": terminal_id, "signal": "SIGINT"})
        assert interrupted["delivered"] is True, interrupted
        time.sleep(0.3)
        recovery_command = "echo DSH_AFTER_CTRL_^C\r" if sys.platform == "win32" else "printf 'DSH_AFTER_CTRL_''C\\n'\r"
        request(port, "terminal-action", body={"sessionId": session_id, "action": "input", "terminalId": terminal_id, "text": recovery_command})
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            read = request(port, "terminal-read", query={"sessionId": session_id, "terminalId": terminal_id, "count": 2000})
            if "DSH_AFTER_CTRL_C" in read["text"]:
                break
            time.sleep(0.1)
        else:
            raise AssertionError(f"terminal did not recover after Ctrl+C: {read!r}")
        closed = request(port, "terminal-action", body={"sessionId": session_id, "action": "close", "terminalId": terminal_id})
        assert closed["closed"] is True, closed
        try:
            remaining = request(port, "terminal-list", query={"sessionId": session_id})["entries"]
            assert remaining == [], remaining
        except AssertionError as failure:
            assert "agent-not-live" in str(failure), failure

        exit_session = call("session.create", {"workspaceId": created["workspaceId"]})["sessionId"]
        exited = request(port, "terminal-action", body={"sessionId": exit_session, "action": "open", "name": "Exit code"})
        request(port, "terminal-action", body={"sessionId": exit_session, "action": "input", "terminalId": exited["id"], "text": "exit 7\r"})
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            exit_row = next((entry for entry in request(port, "terminal-list", query={"sessionId": exit_session})["entries"] if entry["id"] == exited["id"]), None)
            if exit_row and exit_row["status"] == "exited":
                break
            time.sleep(0.1)
        else:
            raise AssertionError(f"terminal exit status did not settle: {exit_row!r}")
        assert exit_row["exitCode"] == 7, exit_row
        request(port, "terminal-action", body={"sessionId": exit_session, "action": "close", "terminalId": exited["id"]})

        shutdown_session = call("session.create", {"workspaceId": created["workspaceId"]})["sessionId"]
        shutdown = request(port, "terminal-action", body={"sessionId": shutdown_session, "action": "open", "name": "Shutdown cleanup"})
        shutdown_pid = int(shutdown["pid"])
        request(port, "terminal-action", body={"sessionId": shutdown_session, "action": "input", "terminalId": shutdown["id"], "text": long_command})
        assert process_alive(shutdown_pid), shutdown
    deadline = time.monotonic() + 10
    while process_alive(shutdown_pid) and time.monotonic() < deadline:
        time.sleep(0.1)
    if process_alive(shutdown_pid):
        if sys.platform == "win32":
            subprocess.run(["taskkill", "/PID", str(shutdown_pid), "/T", "/F"], capture_output=True)
        else:
            os.kill(shutdown_pid, 9)
        raise AssertionError(f"Host shutdown left terminal pid {shutdown_pid} alive")
    print(json.dumps({"status": "passed", "workspace": str(workspace), "withoutNode": args.without_node, "gitRepositories": 2, "terminal": "native-pty-unicode-input-resize-sigint-exit-shutdown"}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
