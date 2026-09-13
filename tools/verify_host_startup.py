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
    binary = args.binary.resolve()
    for relative in ['config/agent-presets/standard/agent.cordis.yml',
                     'plugins/dsh-better-sidebar/lib/client.js', 'web/dist/plugins/manifest.json']:
        if not (binary.parent / relative).is_file():
            raise SystemExit('Startup verification requires the complete adjacent payload: ' + relative)
    work = args.workdir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    stdout_path, stderr_path = work / 'stdout.log', work / 'startup.log'
    workspace = work / 'workspace'
    workspace.mkdir(exist_ok=True)
    environment = dict(os.environ, DSH_HOME=str(work / 'home'), DSH_TRACE_STARTUP='1' if args.trace_startup else '0', DSH_TELEMETRY_DISABLED='1')
    started = time.monotonic()
    result = {'ready': False}
    with stdout_path.open('wb') as stdout, stderr_path.open('wb') as stderr:
        process = subprocess.Popen([str(binary), 'web', '--port', '0'],
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
                    def rpc(method, value):
                        payload = json.dumps({'type': 'client-request', 'rpcId': 'startup-' + method, 'method': method, 'payload': value}).encode()
                        request = urllib.request.Request(match[1] + '/api/' + method, data=payload, headers={'Content-Type': 'application/json'})
                        remaining = 45 - (time.monotonic() - started)
                        if remaining <= 0:
                            raise TimeoutError('Startup API verification exceeded 45 seconds')
                        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=min(15, remaining)) as response:
                            result = json.load(response)['result']
                        if result.get('ok') is not True:
                            raise RuntimeError('Host startup API failed: ' + method)
                        return result['value']
                    rpc('session.list', {})
                    registered = rpc('workspace.create', {'path': str(workspace)})
                    created = rpc('session.create', {'workspaceId': registered['workspace']['workspaceId']})
                    if not created.get('sessionId'):
                        raise RuntimeError('Default preset did not create a session')
                    result['defaultPresetSessionCreated'] = True
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
