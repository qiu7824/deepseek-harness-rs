"""Verify canonical workspace attachment preserves older ordinary-path sessions."""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import pathlib
import sys

sys.dont_write_bytecode = True
from e2e_model_management import isolated_environment, running_fixture_host
from e2e_settings_model_preserves_data import require_ok, rpc


def authoritative_entries(entries):
    result = copy.deepcopy(entries)
    for row in result:
        data = row['event'].get('data', {})
        data.pop('__historyStartSeq', None)
        data.pop('__historyEndSeq', None)
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    parser.add_argument('--workdir', type=pathlib.Path, required=True)
    args = parser.parse_args()
    binary, work = args.binary.resolve(), args.workdir.resolve()
    if work.exists():
        raise ValueError('choose a fresh fixture directory')
    work.mkdir(parents=True)
    home = work / 'home'
    env = isolated_environment(work, home)
    project, foreign = work / 'project', work / 'foreign'
    project.mkdir()
    foreign.mkdir()
    (home / 'settings.json').write_text('{}', encoding='utf-8')
    counter = 0
    evidence = {'binarySha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'passed': False}
    try:
        for phase in ('create', 'restart'):
            with running_fixture_host(binary, work, env, None, 'workspace-resume-' + phase) as port:
                def raw(method, payload):
                    nonlocal counter
                    counter += 1
                    return rpc(port, method, payload, counter)

                def call(method, payload):
                    return require_ok(raw(method, payload), method)

                if phase == 'create':
                    session = call('session.create', {'cwd': str(project), 'agentPreset': 'standard'})['sessionId']
                    call('session.rename', {'sessionId': session, 'title': 'Ordinary path continuity'})
                    workspace = call('workspace.create', {'path': str(project)})['workspace']['workspaceId']
                    other = call('workspace.create', {'path': str(foreign)})['workspace']['workspaceId']
                    baseline = call('session.history', {'sessionId': session})['events']
                restored = call('session.create', {'sessionId': session, 'workspaceId': workspace})
                assert restored['sessionId'] == session
                observed = call('session.history', {'sessionId': session})['events']
                (work / ('history-' + phase + '.json')).write_text(json.dumps({'baseline': baseline, 'observed': observed}), encoding='utf-8')
                original, current = authoritative_entries(baseline), authoritative_entries(observed)
                assert current[:len(original)] == original, 'restoring changed existing authoritative events'
                # Idle disposal may append its native final seed between Host
                # generations. Public page-boundary hints move to that seed.
                assert all(row['event']['type'] == 'session/end-seed' for row in current[len(original):]), current[len(original):]
                rejected = raw('session.create', {'sessionId': session, 'workspaceId': other})['result']
                assert rejected['ok'] is False and rejected['error']['code'] == 'session-conflict', rejected
                missing = raw('session.create', {'sessionId': session, 'cwd': str(work / 'missing')})['result']
                assert missing['ok'] is False and missing['error']['code'] == 'session-conflict', missing
                evidence[phase] = {'sameSessionRestored': True, 'historyUnchanged': True, 'foreignWorkspaceRejected': True, 'missingPathRejected': True}
        evidence['passed'] = True
    finally:
        (work / 'evidence.json').write_text(json.dumps(evidence, indent=2), encoding='utf-8')
    print('PASS ordinary session cwd resumes through canonical workspace before and after restart; foreign and missing paths reject')


if __name__ == '__main__':
    main()
