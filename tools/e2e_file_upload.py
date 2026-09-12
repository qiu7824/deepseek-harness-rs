"""Verify generic uploads through the real Host with a local model fixture."""
from __future__ import annotations

import argparse
import base64
import json
import pathlib
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc


class Model(BaseHTTPRequestHandler):
    records = []

    def log_message(self, *_args):
        pass

    def do_POST(self):
        Model.records.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
        payload = 'data: ' + json.dumps({'choices': [{'index': 0, 'delta': {'role': 'assistant', 'content': 'File received.'}, 'finish_reason': 'stop'}]}) + '\n\ndata: [DONE]\n\n'
        body = payload.encode()
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    parser.add_argument('--workdir', type=pathlib.Path, required=True)
    args = parser.parse_args()
    root = args.workdir.resolve() / ('run-' + str(time.time_ns()))
    workspace = root / 'workspace'
    workspace.mkdir(parents=True)
    home = root / 'home'
    env = isolated_environment(root, home)
    server = ThreadingHTTPServer(('127.0.0.1', 0), Model)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {'llm-pi-ai': {'providers': {'upload-fixture': {'keyless': True, 'api': 'openai-completions', 'baseURL': f'http://127.0.0.1:{server.server_port}/v1', 'models': [{'id': 'fixture', 'contextWindow': 65536, 'maxTokens': 4096}]}}}, 'agent-default-model': {'provider': 'upload-fixture', 'model': 'fixture'}}
    (home / 'settings.json').write_text(json.dumps(settings), encoding='utf-8')
    try:
        with running_fixture_host(args.binary.resolve(), root, env, None, 'file-upload') as port:
            sequence = 0

            def call(method, payload, success=True):
                nonlocal sequence
                sequence += 1
                result = rpc(port, method, payload, sequence)
                return require_ok(result, method) if success else result['result']

            wid = call('workspace.create', {'path': str(workspace)})['workspace']['workspaceId']
            sid = call('session.create', {'workspaceId': wid})['sessionId']

            def part(name, data):
                return {'type': 'file', 'name': name, 'mediaType': 'application/octet-stream', 'data': base64.b64encode(data).decode()}

            content = [part('设计说明.txt', b'file-one'), part('report.pdf', b'%PDF fixture')]
            payload = {'sessionId': sid, 'mode': 'queue', 'requestId': 'upload-one', 'content': content}
            assert call('session.prompt', payload)['accepted']
            deadline = time.monotonic() + 20
            while not Model.records:
                assert time.monotonic() < deadline, 'model did not receive the uploaded-file prompt'
                time.sleep(.05)
            messages = json.dumps(Model.records[0]['messages'], ensure_ascii=False)
            assert '设计说明.txt' in messages and 'report.pdf' in messages
            assert '.dsh-attachments' in messages and 'ZmlsZS1vbmU=' not in messages
            paths = [p for p in (workspace / '.dsh-attachments').rglob('*') if p.is_file()]
            assert sorted(p.read_bytes() for p in paths) == sorted([b'file-one', b'%PDF fixture'])
            # An accepted retry must not create a second prompt or overwrite bytes.
            call('session.prompt', payload)
            assert len([p for p in (workspace / '.dsh-attachments').rglob('*') if p.is_file()]) == 2
            before = {str(p): p.read_bytes() for p in paths}
            invalid = {**payload, 'requestId': 'bad-upload', 'content': [part('valid.txt', b'not-published'), part('../escape.txt', b'bad')]}
            assert call('session.prompt', invalid, False)['ok'] is False
            assert {str(p): p.read_bytes() for p in (workspace / '.dsh-attachments').rglob('*') if p.is_file()} == before
            assert not (workspace / 'escape.txt').exists()
            (root / 'evidence.json').write_text(json.dumps({'passed': True, 'realModelCalls': 0, 'originalBytes': True, 'fileOnlyPrompt': True, 'retryNoOverwrite': True, 'batchValidation': True}), encoding='utf-8')
    finally:
        server.shutdown()
        server.server_close()
    print('PASS real Host file upload: original bytes, file-only prompts, retry identity and batch path rejection')


if __name__ == '__main__':
    main()
