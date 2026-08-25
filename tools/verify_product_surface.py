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
):
    assert required in conversation, f"official task interaction missing: {required}"
assert not (root / "release" / "plugins" / "dsh-task-manager").exists(), "retired task-manager plugin remains bundled"
for forbidden in ("SpeechRecognition", "toggleSpeech", "speechRef", "语音输入"):
    assert forbidden not in conversation, f"built-in voice input remains: {forbidden}"
for required in ("目录与运行环境", "storage-paths", "settings.paths.directoryFlow"):
    assert required in bundle, f"required settings surface missing: {required}"
print("product surface contract verified")
