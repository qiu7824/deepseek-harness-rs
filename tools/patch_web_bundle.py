from pathlib import Path

path = Path(__file__).resolve().parents[1] / "web" / "dist" / "plugins" / "ui-conversation.js"
text = path.read_text(encoding="utf-8")
old = ".puIAHG_root{text-align:center;max-width:var(--dsh-chat-content-width);box-sizing:border-box;width:100%;padding:4px calc(var(--dsh-composer-side-clearance) + 16px) 0px;color:var(--dsw-alias-label-tertiary);white-space:nowrap;text-overflow:ellipsis;margin:0 auto;font-size:12px;line-height:20px;display:block;overflow:hidden}"
new = ".puIAHG_root{text-align:center;max-width:var(--dsh-chat-content-width);box-sizing:border-box;width:100%;padding:4px calc(var(--dsh-composer-side-clearance) + 16px) 0px;color:var(--dsw-alias-label-tertiary);white-space:normal;text-overflow:clip;margin:0 auto;font-size:12px;line-height:20px;display:block;overflow:visible}"
count = text.count(old)
if count == 1:
    text = text.replace(old, new)
elif count == 0 and new not in text:
    raise SystemExit("expected StatsLine CSS signature was not found")
elif count > 1:
    raise SystemExit(f"expected one StatsLine CSS signature, found {count}")

todo = "\t\t\tctx.plugin(todoDockEntry);\n"
queue = "\t\t\tctx.plugin(queueDockEntry);\n"
if todo not in text:
    if queue not in text:
        raise SystemExit("expected queue dock registration was not found")
    text = text.replace(queue, todo + queue, 1)

path.write_text(text, encoding="utf-8")
print("patched ui-conversation statistics layout and retained official task dock")
