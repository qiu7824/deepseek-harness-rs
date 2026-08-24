from pathlib import Path

root = Path(__file__).resolve().parents[1]
bundle = (root / "web" / "dist" / "plugins" / "ui-workbench.js").read_text(encoding="utf-8")
manifest = (root / "web" / "dist" / "plugins" / "manifest.json").read_text(encoding="utf-8")
for forbidden in ("任务面板", "AI画布", "workbench-tasks", "workbench-canvas", "canvasMarks"):
    assert forbidden not in bundle, f"removed surface remains: {forbidden}"
assert "@deepseek-ai/dsh-client-ui-goal" not in manifest, "goal task strip remains composed"
conversation = (root / "web" / "dist" / "plugins" / "ui-conversation.js").read_text(encoding="utf-8")
assert "ctx.plugin(todoDockEntry)" not in conversation, "conversation task dock remains composed"
for forbidden in ("SpeechRecognition", "toggleSpeech", "speechRef", "语音输入"):
    assert forbidden not in conversation, f"built-in voice input remains: {forbidden}"
for required in ("目录与运行环境", "storage-paths", "settings.paths.directoryFlow"):
    assert required in bundle, f"required settings surface missing: {required}"
print("product surface contract verified")
