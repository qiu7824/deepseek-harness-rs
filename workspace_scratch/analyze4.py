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
                pass
    return lines

def result_text(e):
    try:
        parts = e['data']['message']['content']
        return " | ".join(p.get('text','') for p in parts if isinstance(p, dict))
    except Exception:
        return json.dumps(e, ensure_ascii=False)[:300]

def call_name(e):
    d = e.get('data', {})
    # try common shapes
    for k in ('name','tool','toolName'):
        if k in d: return d[k]
    m = d.get('message', {})
    for c in m.get('content', []):
        if isinstance(c, dict) and c.get('type') in ('tool-call','tool_call'):
            return c.get('name') or c.get('toolName')
    return json.dumps(d, ensure_ascii=False)[:200]

for tag, path in FILES:
    print("#"*100)
    print("SESSION:", tag)
    evs = load(path)
    # map callId -> tool name from tool/call events
    names = {}
    for e in evs:
        if e.get('type') == 'tool/call':
            d = e.get('data', {})
            s = json.dumps(d, ensure_ascii=False)
            # find call id and name heuristically
            cid = d.get('callId') or d.get('id')
            nm = d.get('name') or d.get('tool')
            if not nm:
                m = d.get('message', {})
                for c in m.get('content', []):
                    if isinstance(c, dict) and 'tool' in str(c.get('type','')).lower():
                        cid = cid or c.get('toolCallId') or c.get('id')
                        nm = nm or c.get('name') or c.get('toolName')
            if cid:
                names[cid] = nm or '?'
    err_count = collections.Counter()
    print("### ERROR TOOL RESULTS ###")
    for i, e in enumerate(evs):
        if e.get('type') != 'tool/result':
            continue
        d = e.get('data', {})
        is_err = False
        errinfo = d.get('error')
        txt = result_text(e)
        try:
            for p in d['message']['content']:
                if isinstance(p, dict) and p.get('isError'):
                    is_err = True
        except Exception:
            pass
        if errinfo:
            is_err = True
        if is_err:
            cid = None
            try:
                for p in d['message']['content']:
                    if isinstance(p, dict) and p.get('toolCallId'):
                        cid = p['toolCallId']
            except Exception:
                pass
            nm = names.get(cid, '?')
            code = (errinfo or {}).get('code','')
            err_count[(nm, code)] += 1
            print("[%d] tool=%s code=%s :: %s" % (i, nm, code, txt[:500].replace('\n',' / ')))
    print("### ERROR SUMMARY ###")
    for (nm, code), c in err_count.most_common():
        print("  %4d  %s  %s" % (c, nm, code))
    # last assistant message
    print("### LAST ASSISTANT MESSAGES ###")
    ams = [e for e in evs if e.get('type') == 'assistant/message']
    for e in ams[-2:]:
        s = json.dumps(e.get('data',{}), ensure_ascii=False)
        print(s[:1500])
        print()
