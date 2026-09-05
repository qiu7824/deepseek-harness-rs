"""Verify native workspace indexing and source/path APIs in an isolated Host."""
from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import pathlib
import sys
import time
import urllib.parse
from collections.abc import Callable

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc


class PreviewClient:
    def __init__(self, port: int, session_id: str):
        self.port = port
        self.session_id = session_id
        self.requests: list[dict[str, object]] = []

    def request(self, operation: str, params: dict[str, object] | None = None, body: dict[str, object] | None = None) -> tuple[int, dict[str, object]]:
        query = urllib.parse.urlencode({"sessionId": self.session_id, **(params or {})})
        target = "/__dsh-preview/" + operation + ("?" + query if body is None else "")
        return self.raw(target, body)

    def raw(self, target: str, body: dict[str, object] | None = None) -> tuple[int, dict[str, object]]:
        headers = {"origin": f"http://127.0.0.1:{self.port}", "sec-fetch-site": "same-origin"}
        encoded = None
        method = "GET" if body is None else "POST"
        if body is not None:
            encoded = json.dumps(body).encode("utf-8")
            headers["content-type"] = "application/json"
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=15)
        try:
            connection.request(method, target, body=encoded, headers=headers)
            response = connection.getresponse()
            raw = response.read()
            value = json.loads(raw)
            self.requests.append({"method": method, "target": target, "status": response.status})
            return response.status, value
        finally:
            connection.close()

    def ok(self, operation: str, params: dict[str, object] | None = None, body: dict[str, object] | None = None) -> dict[str, object]:
        status, value = self.request(operation, params, body)
        assert status == 200, f"{operation}: HTTP {status}: {value!r}"
        return value

    def graph_until(self, predicate: Callable[[dict[str, object]], bool], description: str, timeout: float = 25) -> dict[str, object]:
        deadline = time.monotonic() + timeout
        latest: dict[str, object] = {}
        while time.monotonic() < deadline:
            latest = self.ok("code-graph")
            if latest.get("status") == "failed":
                raise AssertionError(f"graph failed: {latest!r}")
            if latest.get("status") == "ready" and predicate(latest):
                return latest
            time.sleep(0.3)
        raise AssertionError(f"{description}: {latest!r}")


def symbol(graph: dict[str, object], name: str, file: str) -> dict[str, object]:
    matches = [row for row in graph["symbols"] if row["name"] == name and row["path"] == file]
    assert len(matches) == 1, f"expected one {file}:{name}, got {matches!r}"
    return matches[0]


def write_workspace(root: pathlib.Path) -> str:
    (root / "src").mkdir(parents=True)
    (root / "tsconfig.json").write_text(json.dumps({"compilerOptions": {"baseUrl": ".", "paths": {"@/*": ["src/*"]}}}), encoding="utf-8")
    (root / "src/工具.ts").write_bytes("export function helper() { return 1; }\n".encode("utf-8"))
    (root / "src/其他.ts").write_bytes("export function helper() { return 2; }\n".encode("utf-8"))
    vue = "<template><main>中文预览</main></template>\n<script setup lang=\"ts\">\nimport { helper as invokeHelper } from '@/工具';\nfunction renderView() {\n  return invokeHelper();\n}\n</script>\n"
    (root / "src/视图.vue").write_bytes(vue.encode("utf-8"))
    return vue


def verify(port: int, workdir: pathlib.Path, workspace: pathlib.Path, original: str, without_node: bool) -> dict[str, object]:
    created = require_ok(rpc(port, "workspace.create", {"path": str(workspace)}, 1), "workspace.create")["workspace"]
    session_id = require_ok(rpc(port, "session.create", {"workspaceId": created["workspaceId"]}, 2), "session.create")["sessionId"]
    client = PreviewClient(port, session_id)
    meta = client.ok("meta")
    assert meta["sessionId"] == session_id
    initial = client.graph_until(lambda graph: len(graph["symbols"]) >= 3, "automatic indexing after read-only meta")
    assert initial["engine"] == "rust-tree-sitter"
    assert initial["stats"]["indexedFiles"] == 3
    assert initial["stats"]["parsedFiles"] == 3
    caller = symbol(initial, "renderView", "src/视图.vue")
    helper = symbol(initial, "helper", "src/工具.ts")
    duplicate = symbol(initial, "helper", "src/其他.ts")
    assert caller["line"] == 4 and helper["line"] == duplicate["line"] == 1
    imports = [edge for edge in initial["deps"] if edge["kind"] == "import" and edge["source"] == "src/视图.vue" and edge["target"] == "src/工具.ts"]
    assert any(edge["line"] == 3 for edge in imports), f"TS alias import edge missing: {initial['deps']!r}"
    calls = [edge for edge in initial["calls"] if edge["source"] == caller["id"]]
    assert any(edge["target"] == helper["id"] and edge["line"] == 5 and edge["resolution"] == "import" for edge in calls), f"Vue alias call was not resolved precisely: {calls!r}"
    assert all(edge["target"] != duplicate["id"] for edge in calls), "duplicate helper name must not bind to an unrelated file"
    source = client.ok("source", {"path": "src/视图.vue"})
    assert source == {"path": "src/视图.vue", "text": original}
    resolved = client.ok("file-resolve", {"path": "src/视图.vue"})
    assert resolved["path"] == "src/视图.vue" and resolved["kind"] == "file"
    assert not resolved["absolutePath"].startswith("\\\\?\\"), "Windows display path leaked a verbatim prefix"
    assert pathlib.Path(resolved["absolutePath"]).resolve() == (workspace / "src/视图.vue").resolve()
    absolute = client.ok("file-resolve", {"path": str((workspace / "src/视图.vue").resolve())})
    assert absolute["path"] == resolved["path"]

    changed = original.replace("</script>", "function nextFeature() { return 42; }\n</script>")
    (workspace / "src/视图.vue").write_bytes(changed.encode("utf-8"))
    updated = client.graph_until(lambda graph: any(row["name"] == "nextFeature" for row in graph["symbols"]), "one-file incremental update")
    assert updated["stats"]["parsedFiles"] == 1, updated["stats"]
    assert updated["stats"]["reusedFiles"] == 2, updated["stats"]
    assert symbol(updated, "nextFeature", "src/视图.vue")["line"] == 7
    assert symbol(updated, "helper", "src/工具.ts")["id"] == helper["id"]
    assert client.ok("code-graph-cancel", body={"sessionId": session_id})["cancelled"] is True
    paused = changed.replace("</script>", "function pausedFeature() { return 43; }\n</script>")
    (workspace / "src/视图.vue").write_bytes(paused.encode("utf-8"))
    deadline = time.monotonic() + 4.5
    cancelled_reads = 0
    while time.monotonic() < deadline:
        client.ok("meta")
        graph = client.ok("code-graph")
        assert graph["status"] == "cancelled", "read-only requests must not resume a paused index"
        assert not any(row["name"] == "pausedFeature" for row in graph["symbols"])
        cancelled_reads += 1
        time.sleep(0.5)
    client.ok("code-graph", {"resume": "1"})
    resumed = client.graph_until(lambda graph: any(row["name"] == "pausedFeature" for row in graph["symbols"]), "explicit indexing resume")
    assert symbol(resumed, "pausedFeature", "src/视图.vue")["line"] == 8
    assert resumed["stats"]["parsedFiles"] == 1 and resumed["stats"]["reusedFiles"] == 2
    assert client.ok("source", {"path": "src/视图.vue"})["text"] == paused

    outside = workdir / "越界.txt"
    outside.write_text("OUTSIDE_WORKSPACE_MARKER", encoding="utf-8")
    denials = []
    for operation, target in [("source", "../越界.txt"), ("file-resolve", "../越界.txt"), ("file-resolve", str(outside.resolve()))]:
        status, failure = client.request(operation, {"path": target})
        assert status in (400, 403), (operation, target, status, failure)
        assert "OUTSIDE_WORKSPACE_MARKER" not in json.dumps(failure)
        denials.append({"operation": operation, "path": target, "status": status, "failure": failure})
    status, failure = client.request("file-action", body={"sessionId": session_id, "path": "src/视图.vue", "intent": "invalid-e2e-intent"})
    assert status == 400 and failure.get("error") == "invalid-intent", (status, failure)
    status, runtime = client.raw("/__dsh-runtime?refreshNode=1")
    assert status == 200, runtime
    node = runtime["node"]
    if without_node:
        assert node["status"] == "missing" and node["available"] is False, node
        assert node["path"] is None and node["version"] is None, node
        code_preset = rpc(port, "session.create", {"workspaceId": created["workspaceId"], "agentPreset": "code"}, 3)
        assert code_preset["result"]["ok"] is False, "code mode must reject an unavailable runtime before a model request"
        assert "Node.js" in json.dumps(code_preset, ensure_ascii=False), code_preset
    return {"sessionId": session_id, "workspaceId": created["workspaceId"], "engine": initial["engine"], "initialStats": initial["stats"], "updatedStats": updated["stats"], "resumedStats": resumed["stats"], "importEdges": imports, "preciseCalls": calls, "cancelledReads": cancelled_reads, "resolvedFile": resolved, "pathDenials": denials, "unknownFileAction": failure, "node": node, "withoutNode": without_node, "requests": client.requests}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    parser.add_argument("--without-node", action="store_true")
    args = parser.parse_args()
    binary, workdir = args.binary.resolve(), args.workdir.resolve()
    if not binary.is_file():
        raise ValueError("binary is missing")
    workdir.mkdir(parents=True, exist_ok=True)
    if (workdir / "ui-home").exists() or (workdir / "workspace").exists():
        raise ValueError("workdir already contains a fixture; choose a fresh directory")
    workspace = workdir / "workspace"
    original = write_workspace(workspace)
    env = isolated_environment(workdir, workdir / "ui-home")
    if args.without_node:
        for key in list(env):
            if key.upper() == "PATH":
                env[key] = ""
            elif key.upper().startswith("NODE_"):
                env.pop(key, None)
    with running_fixture_host(binary, workdir, env, None, "workspace-insights") as port:
        evidence = verify(port, workdir, workspace, original, args.without_node)
    evidence["binary"] = str(binary)
    evidence["binarySha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
    result = workdir / "workspace-insights-evidence.json"
    result.write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"PASS native workspace insights: Vue/TS alias and line numbers, duplicate-name isolation, incremental parse/reuse, pause/resume, Chinese source paths, access denials, Node status={evidence['node']['status']}. Evidence: {result}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
