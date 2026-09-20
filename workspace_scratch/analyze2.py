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

for tag, path in FILES:
    print("="*100)
    print("SESSION:", tag)
    evs = load(path)
    print("total events:", len(evs))
    kinds = collections.Counter()
    for e in evs:
        k = e.get('type') or e.get('kind') or str(list(e.keys())[:4])
        kinds[str(k)] += 1
    for k, c in kinds.most_common(50):
        print("  %6d  %s" % (c, k))
