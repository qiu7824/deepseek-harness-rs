# -*- coding: utf-8 -*-
import json, sys, io
import zstandard

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

BASE = r"C:\Users\xs\AppData\Local\DeepSeek Harness\sessions"
P = BASE + r"\--~003F-E-rust-deepseek-harness-rs--\agent-session-4a1b3a37-0055-4cdb-9be3-af5254ebdfb3\session.jsonl.zstd"

dctx = zstandard.ZstdDecompressor()
with open(P, 'rb') as f:
    data = dctx.decompress(f.read(), max_output_size=200*1024*1024)
obj = json.loads(data.decode('utf-8', errors='replace'))

def walk(o, depth, path):
    if depth > 3:
        return
    if isinstance(o, dict):
        print("  "*depth + path + " dict keys=" + str(list(o.keys())[:20]))
        for k, v in o.items():
            if isinstance(v, (dict, list)):
                walk(v, depth+1, path+"."+k)
            else:
                s = str(v)
                print("  "*(depth+1) + k + " = " + s[:120])
    elif isinstance(o, list):
        print("  "*depth + path + " list len=%d" % len(o))
        if o:
            walk(o[0], depth+1, path+"[0]")

walk(obj, 0, "root")
