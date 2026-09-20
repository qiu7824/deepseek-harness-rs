# -*- coding: utf-8 -*-
import json, sys, io, collections
import zstandard

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

BASE = r"C:\Users\xs\AppData\Local\DeepSeek Harness\sessions"
FILES = [
    BASE + r"\--~003F-E-rust-deepseek-harness-rs--\agent-session-4a1b3a37-0055-4cdb-9be3-af5254ebdfb3\session.jsonl.zstd",
    BASE + r"\--~003F-E-~8D44~6599-~4EA4~901A-~9F99~6C5F~8DEF-~98CE~9669~8FA8~8BC6--\agent-session-c9539d51-f904-4d1c-8686-962fc518a252\session.jsonl.zstd",
]

def load(path):
    dctx = zstandard.ZstdDecompressor()
    with open(path, 'rb') as f:
        data = dctx.decompress(f.read(), max_output_size=200*1024*1024)
    lines = []
    for ln in data.decode('utf-8', errors='replace').splitlines():
        ln = ln.strip()
        if ln:
            try:
                lines.append(json.loads(ln))
            except Exception:
                lines.append({'_unparsed': ln[:200]})
    return lines

for path in FILES:
    print("="*100)
    print("FILE:", path)
    evs = load(path)
    print("total events:", len(evs))
    kinds = collections.Counter()
    for e in evs:
        k = e.get('type') or e.get('kind') or e.get('event') or str(list(e.keys())[:3])
        kinds[str(k)] += 1
    for k, c in kinds.most_common(40):
        print("  %6d  %s" % (c, k))
    print("-"*100)
    print("ERROR-ISH EVENTS:")
    for i, e in enumerate(evs):
        s = json.dumps(e, ensure_ascii=False)
        low = s.lower()
        if ('error' in low or 'failed' in low or 'timed_out' in low or 'denied' in low or 'reject' in low) and '_unparsed' not in e:
            t = e.get('type') or e.get('kind') or '?'
            print("[%d] type=%s :: %s" % (i, t, s[:600]))
            print()
