"""Isolated real-Host queue/reference admission regression; no external models."""
import argparse, base64, json, pathlib, sys, threading, time, uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
sys.dont_write_bytecode = True
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import rpc, require_ok
FACT = 'fixed-reference-fact-7392'

class Model(BaseHTTPRequestHandler):
    records = []
    child_started = threading.Event()
    release = threading.Event()
    spawned = False

    def log_message(self, *a):
        pass

    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps({'data': [{'id': m} for m in ('parent', 'child', 'fact-source')]}).encode())

    def chunk(self, delta, finish=None):
        self.wfile.write(('data: ' + json.dumps({'id': 'fixture', 'choices': [{'index': 0, 'delta': delta, 'finish_reason': finish}]}) + '\n\n').encode())
        self.wfile.flush()

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        Model.records.append(request)
        try:
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Connection', 'close')
            self.end_headers()
            model = request['model']
            if model == 'parent' and request.get('tools') and (not Model.spawned):
                Model.spawned = True
                self.chunk({'role': 'assistant', 'tool_calls': [{'index': 0, 'id': 'queue-child-call', 'type': 'function', 'function': {'name': 'subagent', 'arguments': json.dumps({'description': 'Queue admission fixture', 'prompt': 'hold-child', 'provider': 'spawn', 'model': 'child'})}}]}, 'tool_calls')
            elif model == 'child':
                first = not Model.child_started.is_set()
                self.chunk({'role': 'assistant', 'content': 'Child input accepted.'})
                if first:
                    Model.child_started.set()
                    Model.release.wait(30)
                self.chunk({'content': ' Child completed.'}, 'stop')
            else:
                self.chunk({'role': 'assistant', 'content': FACT if model == 'fact-source' else 'Parent completed.'}, 'stop')
            self.wfile.write(b'data: [DONE]\n\n')
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass

def main():
    p = argparse.ArgumentParser()
    p.add_argument('--binary', type=pathlib.Path, required=True)
    p.add_argument('--workdir', type=pathlib.Path, required=True)
    a = p.parse_args()
    work = a.workdir.resolve()
    if work.exists():
        raise RuntimeError('use a fresh test workdir')
    work.mkdir(parents=True)
    home = work / 'home'
    env = isolated_environment(work, home)
    workspace = work / 'workspace'
    workspace.mkdir()
    server = ThreadingHTTPServer(('127.0.0.1', 0), Model)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {'llm-pi-ai': {'providers': {'queue-fixture': {'keyless': True, 'api': 'openai-completions', 'baseURL': f'http://127.0.0.1:{server.server_port}/v1', 'models': [{'id': m, 'contextWindow': 65536, 'maxTokens': 2048} for m in ('parent', 'child', 'fact-source')]}}}, 'agent-default-model': {'provider': 'queue-fixture', 'model': 'parent'}}
    (home / 'settings.json').write_text(json.dumps(settings), encoding='utf-8')
    evidence = {'passed': False}
    try:
        with running_fixture_host(a.binary.resolve(), work, env, None, 'queue-reference') as port:
            counter = 0

            def raw(method, payload):
                nonlocal counter
                counter += 1
                return rpc(port, method, payload, counter)

            def call(method, payload):
                return require_ok(raw(method, payload), method)

            def until(read, predicate, label, timeout=20):
                until_time = time.monotonic() + timeout
                while time.monotonic() < until_time:
                    value = read()
                    if predicate(value):
                        return value
                    time.sleep(0.05)
                raise AssertionError(label)
            wid = call('workspace.create', {'path': str(workspace)})['workspace']['workspaceId']
            source = call('session.create', {'workspaceId': wid, 'agentPreset': 'standard'})['sessionId']
            call('session.selectModel', {'sessionId': source, 'provider': 'queue-fixture', 'model': 'fact-source'})
            call('session.prompt', {'sessionId': source, 'mode': 'queue', 'requestId': 'source-request', 'content': [{'type': 'text', 'text': 'Record a fixed fact.'}]})
            until(lambda: call('session.history', {'sessionId': source}), lambda v: FACT in json.dumps(v) and any((e['event']['type'] == 'turn/end' for e in v['events'])), 'source must finish')
            parent = call('session.create', {'workspaceId': wid, 'agentPreset': 'standard'})['sessionId']
            call('session.prompt', {'sessionId': parent, 'mode': 'queue', 'requestId': 'parent-request', 'content': [{'type': 'text', 'text': 'Start the queue child.'}]})
            assert Model.child_started.wait(20), 'child must start'
            catalog = until(lambda: call('subagent.list', {'parentSessionId': parent}), lambda v: any((i.get('activity') == 'running' for i in v['entries'])), 'running catalog')
            child = next((i for i in catalog['entries'] if i.get('activity') == 'running'))['id']
            address = {'parentSessionId': parent, 'childSessionId': child, 'mode': 'continuable'}
            queued = {**address, 'requestId': 'stable-queue-request', 'delivery': 'queue', 'content': [{'type': 'text', 'text': 'remove-me'}]}
            first = call('subagent.prompt', queued)
            second = call('subagent.prompt', queued)
            evidence['sameRequestHasSameMessageId'] = first['messageId'] == second['messageId']
            assert evidence['sameRequestHasSameMessageId'], 'duplicate browser request created duplicate work'
            item = {'sessionId': child, 'parentSessionId': parent, 'mode': 'continuable', 'itemId': first['messageId']}
            call('session.updateQueue', {**item, 'action': {'kind': 'edit', 'content': [{'type': 'text', 'text': 'edited-and-removed'}]}})
            call('session.updateQueue', {**item, 'action': {'kind': 'remove'}})
            assert raw('subagent.prompt', {**queued, 'mode': 'one-shot'})['result']['ok'] is False
            mention = '@[Fixed fact](dsh-session:' + base64.urlsafe_b64encode(json.dumps(source, separators=(',', ':')).encode()).decode().rstrip('=') + ')'
            clear_ref = call('subagent.prompt', {**address, 'requestId': 'reference-to-clear', 'delivery': 'queue', 'content': [{'type': 'text', 'text': 'Use ' + mention}]})
            clear_item = {'sessionId': child, 'parentSessionId': parent, 'mode': 'continuable', 'itemId': clear_ref['messageId']}
            call('session.updateQueue', {**clear_item, 'action': {'kind': 'edit', 'content': [{'type': 'text', 'text': 'reference-cleared'}]}})
            clear_history = call('subagent.history', {**address, 'maxMessages': 100})
            assert any((e['event']['type'] == 'agent/inbox/spliced' and clear_ref['messageId'] in e['event']['data'].get('additionalContext', {}) and (e['event']['data']['additionalContext'][clear_ref['messageId']] is None) for e in clear_history['events'])), 'editing away the canonical mention must clear its old snapshot'
            call('session.updateQueue', {**clear_item, 'action': {'kind': 'remove'}})
            evidence['editedReferenceClearsSnapshot'] = True
            ref = call('subagent.prompt', {**address, 'requestId': 'reference-steer', 'delivery': 'queue', 'content': [{'type': 'text', 'text': 'Use ' + mention}]})
            assert all((FACT not in json.dumps(r) for r in Model.records if r.get('model') == 'child')), 'queued context contaminated active request'
            call('session.updateQueue', {'sessionId': child, 'parentSessionId': parent, 'mode': 'continuable', 'itemId': ref['messageId'], 'action': {'kind': 'steer'}})
            call('subagent.prompt', {**address, 'requestId': 'direct-steer', 'delivery': 'steer', 'content': [{'type': 'text', 'text': 'direct-steer-marker'}]})
            Model.release.set()
            until(lambda: Model.records, lambda rs: any((r.get('model') == 'child' and FACT in json.dumps(r) and ('direct-steer-marker' in json.dumps(r)) for r in rs)), 'reference snapshot and steer must enter child model')
            history = until(lambda: call('subagent.history', {**address, 'maxMessages': 100}), lambda v: any((e['event']['type'] == 'user/message' and e['event']['data'].get('source', {}).get('plugin') == 'session-reference' for e in v['events'])), 'reference plugin history')
            messages = [e['event']['data'] for e in history['events'] if e['event']['type'] == 'user/message']
            context = next((i for i, m in enumerate(messages) if m.get('source', {}).get('plugin') == 'session-reference'))
            target = next((i for i, m in enumerate(messages) if m.get('source', {}).get('rpcId') == 'reference-steer'))
            assert context + 1 == target, 'reference must be adjacent to its exact human claim'
            assert 'edited-and-removed' not in json.dumps(messages), 'removed queued work was delivered'
            evidence.update({'passed': True, 'referenceClaimIsAdjacent': True, 'referenceDidNotEnterActiveRequest': True, 'removedWorkAbsent': True, 'childId': child})
    finally:
        Model.release.set()
        server.shutdown()
        server.server_close()
        (work / 'evidence.json').write_text(json.dumps(evidence, indent=2), encoding='utf-8')
        (work / 'model-requests.json').write_text(json.dumps(Model.records, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps(evidence), flush=True)
if __name__ == '__main__':
    main()
