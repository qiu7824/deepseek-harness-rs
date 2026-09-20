# -*- coding: utf-8 -*-
import json, sys, io
import zstandard

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

BASE = r"C:\Users\xs\AppData\Local\DeepSeek Harness\sessions"
P = BASE + r"\--~003F-E-~8D44~6599-~4EA4~901A-~9F99~6C5F~8DEF-~98CE~9669~8FA8~8BC6--\agent-session-c9539d51-f904-4d1c-8686-962fc518a252\session.jsonl.zstd"

dctx = zstandard.ZstdDecompressor()
out = io.BytesIO()
with open(P, 'rb') as f:
    with dctx.stream_reader(f, read_across_frames=True) as r:
        while True:
            c = r.read(1 << 20)
            if not c: break
            out.write(c)
evs = [json.loads(l) for l in out.getvalue().decode('utf-8','replace').splitlines() if l.strip()]

WANT = [56, 131, 248, 249, 935, 1686, 3870, 10330, 11410]
for i in WANT:
    e = evs[i]
    print("="*90)
    print("[%d] type=%s" % (i, e.get('type')))
    s = json.dumps(e.get('data', {}), ensure_ascii=False)
    print(s[:1200])
    print()
