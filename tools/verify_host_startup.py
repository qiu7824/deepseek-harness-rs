"""Check isolated Host readiness before provider checks or packaging."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time
import urllib.request


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--workdir', type=Path, required=True)
    parser.add_argument('--trace-startup', action='store_true')
    args = parser.parse_args()
    work = args.workdir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    stdout_path, stderr_path = work / 'stdout.log', work / 'startup.log'
    workspace = work / 'workspace'
    workspace.mkdir(exist_ok=True)
    environment = dict(os.environ, DSH_HOME=str(work / 'home'), DSH_TRACE_STARTUP='1' if args.trace_startup else '0', DSH_TELEMETRY_DISABLED='1')
    started = time.monotonic()
    result = {'ready': False}
    with stdout_path.open('wb') as stdout, stderr_path.open('wb') as stderr:
        process = subprocess.Popen([str(args.binary.resolve()), 'web', '--port', '0'],
            stdout=stdout, stderr=stderr, stdin=subprocess.DEVNULL, env=environment, cwd=workspace,
            creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
        try:
            while time.monotonic() - started < 45:
                if process.poll() is not None:
                    raise RuntimeError(f'Host exited before readiness: {process.returncode}')
                if stderr_path.stat().st_size > 8 * 1024 * 1024:
                    raise RuntimeError('Startup trace exceeded the diagnostic limit')
                match = re.search(r'dsh web: (http://127\.0\.0\.1:\d+)', stdout_path.read_text(encoding='utf-8', errors='replace'))
                if match:
                    payload = json.dumps({'type': 'client-request', 'rpcId': 'startup-check', 'method': 'session.list', 'payload': {}}).encode()
                    request = urllib.request.Request(match[1] + '/api/session.list', data=payload, headers={'Content-Type': 'application/json'})
                    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=5) as response:
                        value = json.load(response)
                    if value.get('result', {}).get('ok') is not True:
                        raise RuntimeError('Host readiness did not expose a usable session API')
                    result['ready'] = True
                    break
                time.sleep(0.1)
            if not result['ready']:
                raise RuntimeError('Host did not report readiness within 45 seconds')
        except Exception as error:
            result['error'] = str(error)
        finally:
            result['seconds'] = round(time.monotonic() - started, 3)
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
    (work / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result))
    if not result['ready']:
        print('\n'.join(stderr_path.read_text(encoding='utf-8', errors='replace').splitlines()[-35:]))
        raise SystemExit(1)


if __name__ == '__main__':
    main()
