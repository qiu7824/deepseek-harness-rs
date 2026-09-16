"""Verify collaboration controls against a real isolated Host and a local model fixture."""
from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import pathlib
import re
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler

from e2e_http import ThreadingHTTPServer
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc


class Model(BaseHTTPRequestHandler):
    records = []
    dispatched = set()
    release = threading.Event()
    release_root = threading.Event()
    lock = threading.Lock()

    def log_message(self, *_args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        serialized = json.dumps(request.get('messages', []), ensure_ascii=False)
        names = re.findall(r'Your teammate name is ([a-z0-9-]+)\.', serialized)
        actor = names[-1] if names else 'lead'
        with Model.lock:
            Model.records.append({'actor': actor, 'request': request})
        if actor == 'slow-worker' and 'CONTROL_RESUME' not in serialized:
            Model.release.wait(30)
        if actor == 'lead' and 'CONTROL_ROOT_HOLD' in serialized and 'CONTROL_ROOT_RESUME' not in serialized:
            Model.release_root.wait(30)
        action, tool_name = None, 'agent_team'
        if actor == 'worker' and 'Task job-one:' in serialized:
            with Model.lock:
                if 'job-one-read' not in Model.dispatched:
                    Model.dispatched.add('job-one-read')
                    action, tool_name = {'file_path': 'fixture.txt'}, 'read'
                elif 'job-one-denial' not in Model.dispatched:
                    Model.dispatched.add('job-one-denial')
                    action, tool_name = {'file_path': 'must-not-exist.txt', 'content': 'DENIED'}, 'write'
                elif 'job-one' not in Model.dispatched:
                    Model.dispatched.add('job-one')
                    revision = int(re.findall(r'expectedRevision (\d+), status review', serialized)[-1])
                    action = {'action': 'task', 'taskId': 'job-one', 'expectedRevision': revision,
                              'status': 'completed', 'result': 'Fixture evidence: verification passed.'}
        if actor == 'job-worker':
            with Model.lock:
                if 'background-job' not in Model.dispatched:
                    Model.dispatched.add('background-job')
                    action, tool_name = {'command': 'Start-Sleep -Seconds 60', 'description': 'isolated background cancellation', 'run_in_background': True}, 'pwsh'
        if action:
            delta = {'role': 'assistant', 'tool_calls': [{'index': 0, 'id': str(uuid.uuid4()), 'type': 'function',
                     'function': {'name': tool_name, 'arguments': json.dumps(action)}}]}
            finish = 'tool_calls'
        else:
            delta, finish = {'role': 'assistant', 'content': 'CONTROL_READY'}, 'stop'
        rows = [{'choices': [{'index': 0, 'delta': delta, 'finish_reason': None}]},
                {'choices': [{'index': 0, 'delta': {}, 'finish_reason': finish}],
                 'usage': {'prompt_tokens': 20, 'completion_tokens': 20, 'total_tokens': 40}}]
        body = (''.join('data: ' + json.dumps(row) + '\n\n' for row in rows) + 'data: [DONE]\n\n').encode()
        try:
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    parser.add_argument('--workdir', type=pathlib.Path, required=True)
    parser.add_argument('--live', action='store_true', help='Keep the final isolated fixture open until ui-stop.request exists')
    args = parser.parse_args()
    root = args.workdir.resolve() / ('run-' + str(time.time_ns()))
    workspace = root / 'workspace'
    workspace.mkdir(parents=True)
    (workspace / 'fixture.txt').write_text('fixture-read-sentinel', encoding='utf-8')
    home = root / 'home'
    env = isolated_environment(root, home)
    env['RUST_BACKTRACE'] = '1'
    server = ThreadingHTTPServer(('127.0.0.1', 0), Model)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    settings = {'agent-teams': {'enabled': False}, 'agent-default-model': {'provider': 'controls-fixture', 'model': 'lead-model'},
                'llm-pi-ai': {'providers': {'controls-fixture': {'keyless': True, 'api': 'openai-completions',
                    'baseURL': f'http://127.0.0.1:{server.server_port}/v1',
                    'models': [{'id': name, 'contextWindow': 65536, 'maxTokens': 4096} for name in ['lead-model', 'worker-model', 'later-model']]}}}}
    (home / 'settings.json').write_text(json.dumps(settings), encoding='utf-8')
    profile = {'id': 'coding', 'name': '编码方案', 'roles': [{'id': 'builder', 'name': '实现成员',
               'instructions': 'Use only assigned files and provide evidence.', 'provider': 'controls-fixture',
               'model': 'worker-model', 'maxTokens': 1024, 'allowTools': ['read', 'agent_team'], 'canSpawn': False}]}
    profile['roles'].append({'id': 'runner', 'name': '执行成员', 'provider': 'controls-fixture', 'model': 'worker-model',
                             'maxTokens': 1024, 'allowTools': ['pwsh'], 'canSpawn': False})
    evidence = {'passed': False, 'realModelCalls': 0, 'checks': [],
                'binary': str(args.binary.resolve()), 'binarySha256': hashlib.sha256(args.binary.read_bytes()).hexdigest()}
    try:
        lead = None
        for phase in ['first', 'restart']:
            with running_fixture_host(args.binary.resolve(), root, env, None, 'collaboration-' + phase) as port:
                sequence = 0

                def call(method, payload):
                    nonlocal sequence
                    sequence += 1
                    return require_ok(rpc(port, method, payload, sequence), method)

                def request(session, action=None, expected=200, origin=None):
                    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
                    try:
                        payload = {'sessionId': session}
                        if action is not None:
                            payload.update(action='control', arguments=action)
                        connection.request('POST', '/__dsh-agent-team', json.dumps(payload),
                                           {'Origin': origin or f'http://127.0.0.1:{port}', 'Content-Type': 'application/json'})
                        response = connection.getresponse()
                        value = json.loads(response.read(4 * 1024 * 1024))
                        assert response.status == expected, (response.status, action, value)
                        if action is None and expected == 200:
                            evidence['lastBoard'] = value.get('board')
                        return value
                    finally:
                        connection.close()

                def until(predicate, timeout=25):
                    deadline = time.monotonic() + timeout
                    while True:
                        value = predicate()
                        if value:
                            return value
                        assert time.monotonic() < deadline, 'collaboration did not settle: ' + json.dumps(evidence.get('lastBoard'))
                        time.sleep(.05)

                def mutate_settings(ops):
                    namespace = next(row for row in call('settings.describe', {})['namespaces'] if row['ns'] == 'agent-teams')
                    assert namespace['applies'] == 'live', namespace
                    return call('settings.mutate', {'ns': 'agent-teams', 'ops': ops, 'expectedRevision': namespace['revision']})

                def member_done(name):
                    member = request(lead)['board']['members'][name]
                    if member['status'] == 'failed':
                        raise AssertionError(member)
                    return member['status'] in ('idle', 'inactive') and member.get('lastOutcome', {}).get('kind') == 'completed'

                if phase == 'first':
                    wid = call('workspace.create', {'path': str(workspace)})['workspace']['workspaceId']
                    lead = call('session.create', {'workspaceId': wid, 'agentPreset': 'blank'})['sessionId']
                    assert next(row for row in call('session.list', {})['items'] if row['sessionId']==lead)['blank'] is True
                    assert request(lead)['enabled'] is False
                    request(lead, {'action': 'create', 'name': 'denied', 'prompt': 'MEMBER_READY'}, expected=400)
                    assert not Model.records
                    mutate_settings([{'op': 'set', 'path': ['enabled'], 'value': True},
                                     {'op': 'set', 'path': ['profiles'], 'value': [profile]}])
                    assert request(lead)['enabled'] is True
                    evidence['checks'].append('global-enable-applies-without-restart')
                    initial_revision=request(lead)['board']['config']['revision']
                    board = request(lead, {'action': 'configure', 'mode': 'custom', 'profileId': 'coding', 'expectedRevision': initial_revision})['board']
                    assert board['config']['revision'] == initial_revision+1
                    assert next(row for row in call('session.list', {})['items'] if row['sessionId']==lead)['blank'] is False
                    evidence['checks'].append('configured-main-conversation-remains-visible-without-a-chat-prompt')
                    request(lead, {'action': 'configure', 'mode': 'off', 'expectedRevision': 0}, expected=400)
                    request(lead, {'action': 'create', 'name': 'invalid', 'prompt': 'MEMBER_READY', 'roleId': 'missing'}, expected=400)
                    assert request(lead)['board']['members'] == {}
                    create = {'action': 'create', 'name': 'worker', 'description': '实现成员', 'prompt': 'MEMBER_READY',
                              'roleId': 'builder', 'requestId': 'stable-member-create'}
                    child = request(lead, create)['board']['members']['worker']['id']
                    assert request(lead, create)['board']['members']['worker']['id'] == child
                    until(lambda: member_done('worker'))
                    worker = next(row['request'] for row in Model.records if row['actor'] == 'worker')
                    assert worker['model'] == 'worker-model', worker
                    assert worker.get('max_tokens', worker.get('max_completion_tokens')) == 1024, worker
                    assert {tool['function']['name'] for tool in worker['tools']} <= {'read', 'agent_team'}
                    evidence['checks'].extend(['per-session-configuration-cas', 'stable-create-retry', 'role-model-and-tool-policy-enforced'])
                    changed = json.loads(json.dumps(profile))
                    changed['roles'][0]['model'] = 'later-model'
                    mutate_settings([{'op': 'set', 'path': ['profiles'], 'value': [changed]}])
                    snapshot = request(lead)['board']['config']['profile']
                    assert snapshot['roles'][0]['model'] == 'worker-model'
                    evidence['checks'].append('running-profile-snapshot-survives-default-edits')
                    request(lead, {'action': 'task', 'taskId': 'job-one', 'expectedRevision': 0, 'subject': 'Verify module',
                                   'owner': child, 'acceptance': 'Provide verification evidence'})
                    request(lead, {'action': 'dispatch', 'taskId': 'job-one', 'expectedRevision': 1})
                    until(lambda: request(lead)['board']['tasks']['job-one']['status'] == 'review')
                    task = request(lead)['board']['tasks']['job-one']
                    assert task['revision'] == 3 and task['result']
                    assert not (workspace / 'must-not-exist.txt').exists()
                    assert any('fixture-read-sentinel' in json.dumps(row['request']) for row in Model.records if row['actor']=='worker')
                    evidence['checks'].append('scoped-file-tools-inherit-and-forbidden-writes-never-execute')
                    request(lead, {'action': 'task', 'taskId': 'job-one', 'expectedRevision': 1, 'status': 'completed'}, expected=400)
                    request(lead, {'action': 'task', 'taskId': 'job-one', 'expectedRevision': 3, 'status': 'completed'})
                    assert request(lead)['board']['tasks']['job-one']['revision'] == 4
                    evidence['checks'].append('dispatch-and-worker-review-require-lead-acceptance')
                    other = call('session.create', {'workspaceId': wid, 'sessionId': 'agent-session-' + str(uuid.uuid4())})['sessionId']
                    request(other, {'action': 'message', 'target': child, 'message': 'wrong-team', 'messageId': 'wrong'}, expected=400)
                    request(lead, create, expected=403, origin='https://untrusted.invalid')
                    evidence['checks'].append('cross-team-and-cross-origin-controls-rejected')
                    if __import__('os').name == 'nt':
                        request(lead, {'action': 'create', 'name': 'job-worker', 'roleId': 'runner', 'prompt': 'CONTROL_BACKGROUND', 'requestId': 'job-create'})
                        until(lambda: any(job['status']=='running' for job in request(lead)['board']['members']['job-worker'].get('jobs',[])), timeout=60)
                    slow = request(lead, {'action': 'create', 'name': 'slow-worker', 'roleId': 'builder', 'prompt': 'CONTROL_HOLD', 'requestId': 'slow-create'})['board']['members']['slow-worker']['id']
                    until(lambda: any(row['actor'] == 'slow-worker' for row in Model.records))
                    request(lead, {'action': 'task', 'taskId': 'slow-job', 'expectedRevision': 0, 'subject': 'Slow operation', 'owner': slow, 'status': 'in_progress'})
                    call('session.prompt', {'sessionId': lead, 'mode': 'queue', 'requestId': 'root-hold', 'content': [{'type':'text','text':'CONTROL_ROOT_HOLD'}]})
                    until(lambda: any(row['actor']=='lead' and 'CONTROL_ROOT_HOLD' in json.dumps(row['request']) for row in Model.records))
                    root_cancelled={'sessionId':lead,'mode':'queue','requestId':'cancel-root-request','content':[{'type':'text','text':'OLD_ROOT_QUEUE_MUST_NOT_RUN'}]}
                    call('session.prompt',root_cancelled)
                    child_cancelled={'parentSessionId':lead,'childSessionId':slow,'mode':'continuable','delivery':'queue','requestId':'cancel-child-request','content':[{'type':'text','text':'OLD_CHILD_QUEUE_MUST_NOT_RUN'}]}
                    call('subagent.prompt',child_cancelled)
                    request(lead, {'action': 'message', 'target': slow, 'message': 'QUEUED_WORK', 'messageId': 'cancel-queued'})
                    call('session.cancel', {'sessionId': lead})
                    stopped = request(lead)['board']
                    assert stopped['tasks']['slow-job']['status'] == 'blocked'
                    assert not any(member['status'] == 'running' for member in stopped['members'].values())
                    assert stopped['pendingMessages'] == 0
                    if 'job-worker' in stopped['members']:
                        assert stopped['members']['job-worker']['jobs']
                        assert all(job['status'] not in ('running','stopping') for job in stopped['members']['job-worker']['jobs'])
                        evidence['checks'].append('stop-all-settles-owned-background-processes')
                    Model.release.set()
                    Model.release_root.set()
                    for method,payload in [('session.prompt',root_cancelled),('subagent.prompt',child_cancelled)]:
                        evidence['pendingCheck'] = {'method': method, 'requestId': payload['requestId']}
                        sequence+=1
                        rejected=rpc(port,method,payload,sequence)['result']
                        assert rejected['ok'] is False and rejected['error']['code']=='cancelled',rejected
                    evidence.pop('pendingCheck', None)
                    root_calls=sum(row['actor']=='lead' for row in Model.records)
                    time.sleep(.2)
                    assert sum(row['actor']=='lead' for row in Model.records)==root_calls,'stopped children must not wake the stopped main conversation'
                    evidence['checks'].append('cancelled-human-retries-stay-cancelled-and-stop-does-not-wake-the-lead')
                    evidence['checks'].append('stop-all-cancels-members-and-pending-delivery')
                    request(lead, {'action': 'message', 'target': slow, 'message': 'CONTROL_RESUME', 'messageId': 'resume-after-stop'})
                    until(lambda: member_done('slow-worker'))
                    evidence['checks'].append('new-work-resumes-after-stop')
                else:
                    assert call('host.describe', {})['attachedSessions'] == 0
                    catalog = call('subagent.list', {'parentSessionId': lead})
                    assert catalog['parentAvailable'] is False and catalog['parentResumable'] is True, catalog
                    call('subagent.history', {'parentSessionId': lead, 'childSessionId': child, 'mode': 'continuable'})
                    assert call('host.describe', {})['attachedSessions'] == 0, 'browsing a cold member must not activate its owner'
                    start = len(Model.records)
                    prompt = {'parentSessionId': lead, 'childSessionId': child, 'mode': 'continuable', 'requestId': 'cold-member-followup',
                              'content': [{'type': 'text', 'text': 'AFTER_RESTART'}]}
                    receipt = call('subagent.prompt', prompt)
                    assert call('subagent.prompt', prompt)['messageId'] == receipt['messageId'], 'cold follow-up retry must not duplicate the input'
                    until(lambda: any(row['actor'] == 'worker' for row in Model.records[start:]))
                    until(lambda: member_done('worker'))
                    board = request(lead)['board']
                    assert board['config']['profile']['roles'][0]['model'] == 'worker-model'
                    assert board['tasks']['job-one']['status'] == 'completed'
                    resumed = next(row['request'] for row in Model.records[start:] if row['actor'] == 'worker')
                    assert resumed['model'] == 'worker-model', resumed
                    assert resumed.get('max_tokens', resumed.get('max_completion_tokens')) == 1024, resumed
                    assert {tool['function']['name'] for tool in resumed['tools']} == {'read', 'agent_team'}
                    evidence['checks'].append('direct-member-followup-restores-cold-owner-without-activating-on-browse')
                    evidence['checks'].append('cold-main-and-member-resume-preserve-role-and-board')
                    if args.live:
                        (root / 'ui-context.json').write_text(json.dumps({'port': port, 'lead': lead, 'child': child, 'workspace': str(workspace)}, indent=2), encoding='utf-8')
                        print(f'COLLABORATION_UI_READY http://127.0.0.1:{port}/ {root}', flush=True)
                        while not (root / 'ui-stop.request').exists():
                            time.sleep(.2)
        for log in root.glob('model-management-host-collaboration-*.log'):
            assert 'panicked at' not in log.read_text(encoding='utf-8'), f'runtime panic recorded in {log.name}'
        evidence['checks'].append('no-runtime-panics-during-stop-and-cold-resume')
        evidence['passed'] = True
    finally:
        Model.release.set()
        Model.release_root.set()
        server.shutdown()
        server.server_close()
        (root / 'evidence.json').write_text(json.dumps(evidence, indent=2), encoding='utf-8')
        (root / 'model-requests.json').write_text(json.dumps(Model.records, ensure_ascii=False), encoding='utf-8')
    print('PASS collaboration controls:', ', '.join(evidence['checks']))


if __name__ == '__main__':
    main()
