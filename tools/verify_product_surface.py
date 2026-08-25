from pathlib import Path

root = Path(__file__).resolve().parents[1]
bundle = (root / "web" / "dist" / "plugins" / "ui-workbench.js").read_text(encoding="utf-8")
manifest = (root / "web" / "dist" / "plugins" / "manifest.json").read_text(encoding="utf-8")
for forbidden in ("任务面板", "AI画布", "workbench-tasks", "workbench-canvas", "canvasMarks"):
    assert forbidden not in bundle, f"removed surface remains: {forbidden}"
assert "@deepseek-ai/dsh-client-ui-goal" not in manifest, "goal task strip remains composed"
conversation = (root / "web" / "dist" / "plugins" / "ui-conversation.js").read_text(encoding="utf-8")
assert "ctx.plugin(todoDockEntry)" in conversation, "official conversation task dock is not composed"
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
assert not (root / "release" / "plugins" / "dsh-task-manager").exists(), "retired task-manager plugin remains bundled"
for plugin in ("dsh-web-preview-rs", "dsh-better-sidebar"):
    package = root / "release" / "plugins" / plugin / "package.json"
    client = root / "release" / "plugins" / plugin / "lib" / "client.js"
    assert package.is_file() and client.is_file(), f"bundled plugin is incomplete: {plugin}"
better_sidebar = (root / "release" / "plugins" / "dsh-better-sidebar" / "lib" / "client.js").read_text(encoding="utf-8")
for required in ('id: "dsh-better-sidebar"', 'conversation.session.header.utilities', 'shell.overlay', '/__dsh-preview/'):
    assert required in better_sidebar, f"better-sidebar Rust slice missing: {required}"
for forbidden in ('children: "✎"', 'children: "■"', '"hero.preview": "预览版"'):
    assert forbidden not in conversation, f"retired task presentation remains: {forbidden}"
for forbidden in ("SpeechRecognition", "toggleSpeech", "speechRef", "语音输入"):
    assert forbidden not in conversation, f"built-in voice input remains: {forbidden}"
for required in ("目录与运行环境", "storage-paths", "settings.paths.directoryFlow"):
    assert required in bundle, f"required settings surface missing: {required}"
print("product surface contract verified")
