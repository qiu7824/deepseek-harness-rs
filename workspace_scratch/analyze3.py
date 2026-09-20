# -*- coding: utf-8 -*-
import json, sys, io, collections
import zstandard

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

BASE = r"C:\Users\xs\AppData\Local\DeepSeek Harness\sessions"
FILES = [
    ("4a1b3a37", BASE + r"\--~003F-E-rust-deepseek-harness-rs--\agent-session-4a1b3a37-0055-4cdb-9be3-af5254ebdfb3\session.jsonl.zstd"),
    ("c9539d51", BASE + r"\--~003F-E-~8D44~6599-~4EA4~901A-~9F99~6C5F~8DEF-~98CE~9669~8FA8~8BC6--\agent-session-c9539d51-f904-4d1c-8686-962fc518a252\session.jsonl.zstd"),
]

def load(path):
    dctx = zstandard.ZstdDecompressor()
    out = io.BytesIO()
    with open(path, 'rb') as f:
        with dctx.stream_reader(f, read_across_frames=True) as r:
            while True:
                chunk = r.read(1 << 20)
                if not chunk:
                    break
                out.write(chunk)
    lines = []
    for ln in out.getvalue().decode('utf-8', errors='replace').splitlines():
        ln = ln.strip()
        if ln:
            try:
                lines.append(json.loads(ln))
            except Exception:
                lines.append({'_unparsed': ln[:300]})
    return lines

def snip(o, n=500):
    s = json.dumps(o, ensure_ascii=False) if not isinstance(o, str) else o
    return s[:n]

for tag, path in FILES:
    print("#"*110)
    print("SESSION:", tag)
    evs = load(path)
    print("### USER MESSAGES ###")
    for i, e in enumerate(evs):
        if e.get('type') == 'user/message':
            print("[%d] %s" % (i, snip(e, 800)))
            print()
    print("### TOOL CALLS WITH ERROR RESULTS ###")
    # pair tool/call and tool/result by index order
    for i, e in enumerate(evs):
        if e.get('type') == 'tool/result':
            s = json.dumps(e, ensure_ascii=False)
            low = s.lower()
            if ('error' in low or 'failed' in low or 'timed_out' in low or 'denied' in low or 'reject' in low or 'no replacement' in low):
                print("[%d] %s" % (i, snip(e, 900)))
                print()
