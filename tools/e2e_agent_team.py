"""Exercise opt-in teams, durable peer delivery and task CAS in a real Host."""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc
from e2e_session_workspace_resume import flushed_raw_log


class Model(BaseHTTPRequestHandler):
    rounds = {}
    records = []
    lock = threading.Lock()

    def log_message(self, *_args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        serialized = json.dumps(request.get('messages', []))
        matches = re.findall(r'Your teammate name is ([a-z-]+)\.', serialized)
        actor = matches[-1] if matches else 'lead'
        if not request.get('tools'):
            actor = 'title'
        with Model.lock:
            round_ = Model.rounds.get(actor, 0)
            Model.rounds[actor] = round_ + 1
            Model.records.append({'actor': actor, 'request': request})
        actions = [
            {'action': 'create', 'name': 'alice', 'prompt': 'Report readiness to the lead.', 'context': 'fresh'},
            {'action': 'create', 'name': 'bob', 'prompt': 'Send one peer message to alice.', 'context': 'fresh'},
            {'action': 'task', 'taskId': 'check', 'expectedRevision': 0, 'subject': 'Inspect module', 'owner': 'alice', 'writeScopes': ['src/module']},
            {'action': 'task', 'taskId': 'check', 'expectedRevision': 1, 'status': 'in_progress'},
            {'action': 'task', 'taskId': 'check', 'expectedRevision': 1, 'status': 'completed'},
            {'action': 'task', 'taskId': 'check', 'expectedRevision': 2, 'status': 'completed'},
            {'action': 'message', 'target': 'bob', 'messageId': 'lead-to-bob', 'message': 'Task completed.'},
            {'action': 'message', 'target': 'bob', 'messageId': 'lead-to-bob', 'message': 'Task completed.'},
            {'action': 'status'},
        ]
        action = actions[round_] if actor == 'lead' and round_ < len(actions) else None
        if actor == 'alice' and round_ == 0:
            action = {'action': 'message', 'target': 'lead', 'messageId': 'alice-ready', 'message': 'Alice is ready.'}
        if actor == 'bob' and round_ == 0:
            action = {'action': 'message', 'target': 'alice', 'messageId': 'bob-to-alice', 'message': 'Peer check complete.'}
        if action:
            delta = {'role': 'assistant', 'tool_calls': [{'index': 0, 'id': f'{actor}-{round_}', 'type': 'function', 'function': {'name': 'agent_team', 'arguments': json.dumps(action)}}]}
            finish = 'tool_calls'
        else:
            delta, finish = {'role': 'assistant', 'content': f'{actor} finished.'}, 'stop'
        rows = [{'choices': [{'index': 0, 'delta': delta, 'finish_reason': None}]}, {'choices': [{'index': 0, 'delta': {}, 'finish_reason': finish}], 'usage': {'prompt_tokens': 20, 'completion_tokens': 20, 'total_tokens': 40}}]
        body = (''.join('data: ' + json.dumps(row) + '\n\n' for row in rows) + 'data: [DONE]\n\n').encode()
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass


def main():
    import http.client
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
    settings = {'agent-teams': {'enabled': True, 'maxMembers': 4}, 'llm-pi-ai': {'providers': {'team-fixture': {'keyless': True, 'api': 'openai-completions', 'baseURL': f'http://127.0.0.1:{server.server_port}/v1', 'models': [{'id': 'fixture', 'contextWindow': 65536, 'maxTokens': 4096}]}}}, 'agent-default-model': {'provider': 'team-fixture', 'model': 'fixture'}}
    (home / 'settings.json').write_text(json.dumps(settings), encoding='utf-8')
    evidence = {'passed': False, 'realModelCalls': 0}
    try:
        sid = None
        for phase in ('first', 'restart'):
            with running_fixture_host(args.binary.resolve(), root, env, None, 'teams-' + phase) as port:
                sequence = 0

                def call(method, payload):
                    nonlocal sequence
                    sequence += 1
                    return require_ok(rpc(port, method, payload, sequence), method)

                def board(id_, origin=None):
                    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
                    try:
                        connection.request('POST', '/__dsh-agent-team', json.dumps({'sessionId': id_}), {'Origin': origin or f'http://127.0.0.1:{port}', 'Content-Type': 'application/json'})
                        response = connection.getresponse()
                        return response.status, json.loads(response.read(4 * 1024 * 1024))
                    finally:
                        connection.close()

                if phase == 'first':
                    wid = call('workspace.create', {'path': str(workspace)})['workspace']['workspaceId']
                    sid = call('session.create', {'workspaceId': wid})['sessionId']
                    call('session.prompt', {'sessionId': sid, 'mode': 'queue', 'content': [{'type': 'text', 'text': 'Explicitly create an agent team for this isolated integration test.'}]})
                    deadline = time.monotonic() + 60
                    while True:
                        status, result = board(sid)
                        assert status == 200, result
                        state = result.get('board', {})
                        if state.get('tasks', {}).get('check', {}).get('status') == 'completed' and state.get('pendingMessages') == 0 and Model.rounds.get('lead', 0) >= 10:
                            break
                        assert time.monotonic() < deadline, json.dumps({'board': result, 'rounds': Model.rounds})
                        time.sleep(.1)
                    assert len(state['members']) == 2, state
                    assert state['tasks']['check']['revision'] == 3, state
                    assert board(sid, 'https://untrusted.invalid')[0] == 403
                    raw = flushed_raw_log(port, sid)
                    queued = [row['data']['message'] for row in raw[1:] if row['type'] == 'team/message/queued']
                    assert {mail['id'] for mail in queued} == {'lead-to-bob', 'alice-ready', 'bob-to-alice'}, queued
                    assert sum(mail['id'] == 'lead-to-bob' for mail in queued) == 1
                    revisions = [row['data']['task']['revision'] for row in raw[1:] if row['type'] == 'team/task']
                    assert revisions == [1, 2, 3], revisions
                    assert any('task revision conflict' in json.dumps(record['request']) for record in Model.records), 'stale CAS must produce a visible failure'
                    ids = {member['id'] for member in state['members'].values()}
                    for id_ in ids:
                        child = flushed_raw_log(port, id_)
                        deliveries = [row for row in child[1:] if row['type'] == 'user/message' and row['data']['source']['kind'] == 'team-message']
                        assert len(deliveries) == 1, deliveries
                    evidence.update({'peerDelivery': True, 'taskCas': True, 'stableMessageRetry': True, 'originBoundary': True})
                else:
                    status, result = board(sid)
                    assert status == 200 and result['board']['tasks']['check']['revision'] == 3, result
                    assert set(member['id'] for member in result['board']['members'].values()) == ids
                    assert not any(member['status'] == 'running' for member in result['board']['members'].values())
                    fork = call('session.fork', {'sessionId': sid})['sessionId']
                    _, independent = board(fork)
                    assert independent['board']['members'] == {} and independent['board']['tasks'] == {}, independent
                    call('workspace.archiveSession', {'sessionId': sid})
                    call('workspace.deleteArchivedSession', {'sessionId': sid})
                    surviving = {item['sessionId'] for item in call('session.list', {})['items']}
                    assert not ({sid} | ids) & surviving
                    assert fork in surviving
                    evidence.update({'restartRecovery': True, 'forkIsolation': True, 'treeDeletion': True})
        evidence['passed'] = True
    finally:
        server.shutdown()
        server.server_close()
        (root / 'evidence.json').write_text(json.dumps(evidence, indent=2), encoding='utf-8')
        (root / 'model-requests.json').write_text(json.dumps(Model.records), encoding='utf-8')
    print('PASS real Host teams: named members, peer delivery, message retries, task CAS, restart, fork isolation and tree deletion')


if __name__ == '__main__':
    main()
