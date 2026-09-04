from pathlib import Path

root = Path(__file__).resolve().parents[1]
bundle = (root / "web" / "dist" / "plugins" / "ui-workbench.js").read_text(encoding="utf-8")
manifest = (root / "web" / "dist" / "plugins" / "manifest.json").read_text(encoding="utf-8")
for forbidden in ("任务面板", "AI画布", "workbench-tasks", "workbench-canvas", "canvasMarks"):
    assert forbidden not in bundle, f"removed surface remains: {forbidden}"
assert "@deepseek-ai/dsh-client-ui-goal" in manifest, "goal pause/resume strip is not composed"
assert "@deepseek-ai/dsh-client-ui-code-graph" in manifest, "code graph tab is not composed"
conversation = (root / "web" / "dist" / "plugins" / "ui-conversation.js").read_text(encoding="utf-8")
assert "ctx.plugin(todoDockEntry)" in conversation, "conversation task dock is not composed"
for required in (
    'method: "session.updateTodos"',
    '"todo.edit": "修改任务"',
    '"todo.remove": "停止并移除任务"',
    '"todo.stopTurn": "停止当前运行"',
    '"hero.preview": "Rust 版"',
    'IconEditOutline16, { size: 14 }',
    'IconTrashOutline16, { size: 14 }',
):
    assert required in conversation, f"official task interaction missing: {required}"
for retired in ("dsh-task-manager", "dsh-web-preview-rs", "dsh-composer-expand"):
    assert not (root / "release" / "plugins" / retired).exists(), f"retired plugin remains bundled: {retired}"
for plugin in ("dsh-better-sidebar",):
    package = root / "release" / "plugins" / plugin / "package.json"
    client = root / "release" / "plugins" / plugin / "lib" / "client.js"
    assert package.is_file() and client.is_file(), f"bundled plugin is incomplete: {plugin}"
better_sidebar = (root / "release" / "plugins" / "dsh-better-sidebar" / "lib" / "client.js").read_text(encoding="utf-8")
for required in ('id: "dsh-better-sidebar"', 'conversation.session.header.utilities', 'shell.overlay', '/__dsh-preview/', '显示工作台', 'renderMarkdown', 'file-save', 'git-status', 'git-action', 'terminal-action', 'WorkbenchPanel', 'WorkbenchTerminal', 'WorkbenchBrowser', 'data-layout":"full', 'highlightCode', 'isHtml(state.file)?"preview"', 'stage-all', 'unstage-all', 'commit-push', '提交并推送', 'DocTabs', 'MAX_SESSION_STATES = 16', 'closeWorkbench'):
    assert required in better_sidebar, f"better-sidebar Rust slice missing: {required}"
for forbidden in ('children: "✎"', 'children: "■"', '"hero.preview": "预览版"'):
    assert forbidden not in conversation, f"retired task presentation remains: {forbidden}"
for forbidden in ("SpeechRecognition", "toggleSpeech", "speechRef", "语音输入"):
    assert forbidden not in conversation, f"built-in voice input remains: {forbidden}"
for required in ("目录与运行环境", "storage-paths", "settings.paths.directoryFlow"):
    assert required in bundle, f"required settings surface missing: {required}"
voice = (root / "release" / "plugins" / "dsh-voice-input" / "lib" / "client.js").read_text(encoding="utf-8")
assert 'borderRadius: "8px"' in voice and "interactive-bg-hover" in voice, "voice input button style drifted"
log_bundle = root / "web" / "dist" / "plugins" / "session-log-download.js"
assert 'aria-label": "下载会话日志"' in log_bundle.read_text(encoding="utf-8"), "session log is not icon-only"
import hashlib, json
manifest_value = json.loads((root / "web" / "dist" / "plugins" / "manifest.json").read_text(encoding="utf-8"))
for entry in manifest_value["entries"]:
    declared = entry["url"]
    bundle_path = root / "web" / "dist" / declared.lstrip("/")
    assert bundle_path.is_file(), f"manifest bundle is missing: {declared}"
    actual = hashlib.sha256(bundle_path.read_bytes()).hexdigest()[:16]
    assert entry["rev"] == actual, f"{declared} manifest rev is stale"
runtime_source = (root / "web" / "dist" / "plugins" / "client-runtime.js").read_text(encoding="utf-8")
conversation_source = (root / "web" / "dist" / "plugins" / "ui-conversation.js").read_text(encoding="utf-8")
connection_source = (root / "web" / "dist" / "plugins" / "connection.js").read_text(encoding="utf-8")
for required in ("hasMoreBefore: boolean()", "hasMoreAfter: boolean()", "firstSeq: number().int().nullable()", "lastSeq: number().int().nullable()"):
    assert required in connection_source, f"directional history schema missing: {required}"
for required in ("hasMoreBefore", "hasMoreAfter", "async loadNewer()", "HISTORY_PAGE_MESSAGES", "HISTORY_WINDOW_PAGES", "HISTORY_WINDOW_EVENTS", "Never cut a raw event range", "bufferLive(event, view)"):
    assert required in runtime_source, f"bounded bidirectional history contract missing: {required}"
for required in ("hasMoreAfter", "loadingNewer", "Promise.resolve(loadNewer())", "Promise.resolve(loadOlder())"):
    assert required in conversation_source, f"forward history scroll trigger missing: {required}"
for memory_tool in ("memory_probe.py", "memory_scenarios.py", "memory_fixture.py", "validate_memory_baseline.py"):
    assert (root / "tools" / memory_tool).is_file(), f"memory tool is missing: {memory_tool}"
for memory_test in ("test_memory_probe.py", "test_memory_scenarios.py", "test_memory_fixture.py", "test_memory_baseline.py"):
    assert (root / "tools" / "tests" / memory_test).is_file(), f"memory test is missing: {memory_test}"
scenario_source = (root / "tools" / "memory_scenarios.py").read_text(encoding="utf-8")
for required in ("READ_ONLY_RPC_METHODS", "run_default_matrix", "binary_sha256", "home_path_sha256", '"schema_version": 1'):
    assert required in scenario_source or required in (root / "tools" / "memory_probe.py").read_text(encoding="utf-8"), f"memory contract missing: {required}"
for forbidden in ("session.updateTodos", "settings.write", "credentials.describe", "CommandLine"):
    assert forbidden not in scenario_source, f"memory scenario contains forbidden capability: {forbidden}"
fixture_source = (root / "tools" / "memory_fixture.py").read_text(encoding="utf-8")
assert "read_bytes()" not in fixture_source, "memory fixture hashes by materializing the whole file"
for readme in ("README.md", "README.zh.md"):
    text = (root / readme).read_text(encoding="utf-8")
    assert "test_memory_*.py" in text and "validate_memory_baseline.py" in text, f"memory gates missing from {readme}"
print("product surface contract verified")
