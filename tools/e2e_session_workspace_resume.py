"""Verify canonical workspace attachment preserves older ordinary-path sessions."""
from __future__ import annotations

import argparse
import copy
import hashlib
import http.client
import io
import json
import pathlib
import sys
import urllib.parse
import zipfile

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


def first_difference(expected, actual, path='$'):
    if type(expected) is not type(actual):
        return {'path': path, 'expectedType': type(expected).__name__, 'actualType': type(actual).__name__}
    if isinstance(expected, dict):
        if expected.keys() != actual.keys():
            return {'path': path, 'missingKeys': sorted(expected.keys() - actual.keys()), 'addedKeys': sorted(actual.keys() - expected.keys())}
        for key in expected:
            difference = first_difference(expected[key], actual[key], path + '.' + key)
            if difference:
                return difference
    elif isinstance(expected, list):
        for index, (old, new) in enumerate(zip(expected, actual)):
            difference = first_difference(old, new, f'{path}[{index}]')
            if difference:
                return difference
        if len(expected) != len(actual):
            return {'path': path, 'expectedLength': len(expected), 'actualLength': len(actual)}
    elif expected != actual:
        # This fixture contains only synthetic setup metadata. Still bound
        # log output so failures remain useful without dumping whole events.
        def short(value):
            if isinstance(value, str) and len(value) > 120:
                return {'length': len(value), 'sha256': hashlib.sha256(value.encode()).hexdigest()}
            return value
        return {'path': path, 'expected': short(expected), 'actual': short(actual)}
    return None


def flushed_raw_log(port, session_id):
    """Use the export endpoint's durability barrier, then read its raw JSONL."""
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=20)
    try:
        connection.request('GET', '/api/session.export?' + urllib.parse.urlencode({'sessionId': session_id, 'includeDescendants': 'false'}), headers={'Origin': f'http://127.0.0.1:{port}', 'Sec-Fetch-Site': 'same-origin'})
        response = connection.getresponse()
        body = response.read(1024 * 1024 + 1)
        assert response.status == 200, f'session.export failed: HTTP {response.status}'
        assert len(body) <= 1024 * 1024, 'synthetic export exceeds its bound'
        with zipfile.ZipFile(io.BytesIO(body)) as archive:
            logs = [entry for entry in archive.infolist() if entry.filename.endswith('.jsonl')]
            assert len(logs) == 1 and logs[0].file_size <= 1024 * 1024, 'expected one bounded synthetic raw log'
            return [json.loads(line) for line in archive.read(logs[0]).splitlines() if line.strip()]
    finally:
        connection.close()


def page_bounds(page):
    return {key: page.get(key) for key in ('firstSeq', 'lastSeq', 'hasMore', 'hasMoreBefore', 'hasMoreAfter')}


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
                    renamed = call('session.rename', {'sessionId': session, 'title': 'Ordinary path continuity'})
                    workspace = call('workspace.create', {'path': str(project)})['workspace']['workspaceId']
                    other = call('workspace.create', {'path': str(foreign)})['workspace']['workspaceId']
                    # The Host batches writes for up to 200ms. This helper's
                    # restart forcibly terminates the process; establish a
                    # durable baseline through the public flush barrier first.
                    raw_baseline = flushed_raw_log(port, session)
                    assert raw_baseline[0].get('id') == session, 'export returned a different session'
                    assert any(row.get('seq') == renamed['seq'] and row.get('type') == 'session/title' and row.get('data', {}).get('title') == 'Ordinary path continuity' for row in raw_baseline), 'durability barrier did not persist the acknowledged title'
                    baseline_page = call('session.history', {'sessionId': session})
                    baseline = baseline_page['events']
                restored = call('session.create', {'sessionId': session, 'workspaceId': workspace})
                assert restored['sessionId'] == session
                raw_observed = flushed_raw_log(port, session)
                observed_page = call('session.history', {'sessionId': session})
                observed = observed_page['events']
                (work / ('history-' + phase + '.json')).write_text(json.dumps({'baseline': baseline, 'observed': observed, 'baselinePage': page_bounds(baseline_page), 'observedPage': page_bounds(observed_page), 'rawBaseline': raw_baseline, 'rawObserved': raw_observed}), encoding='utf-8')
                raw_difference = first_difference(raw_baseline, raw_observed[:len(raw_baseline)])
                assert raw_difference is None, 'restoring changed the stored raw log: ' + json.dumps({'phase': phase, 'firstDifference': raw_difference}, ensure_ascii=False)
                assert all(row['type'] == 'session/end-seed' for row in raw_observed[len(raw_baseline):]), 'unexpected authoritative events during workspace restore'
                assert baseline_page.get('firstSeq') == observed_page.get('firstSeq') == 0
                assert not any(page.get(key) for page in (baseline_page, observed_page) for key in ('hasMoreBefore', 'hasMoreAfter')), 'synthetic history is unexpectedly paginated'
                original, current = authoritative_entries(baseline), authoritative_entries(observed)
                difference = first_difference(original, current[:len(original)])
                assert difference is None, 'restoring changed existing authoritative events: ' + json.dumps({'phase': phase, 'firstDifference': difference, 'expectedEvents': len(original), 'observedEvents': len(current)}, ensure_ascii=False)
                # Idle disposal may append its native final seed between Host
                # generations. Public page-boundary hints move to that seed.
                assert all(row['event']['type'] == 'session/end-seed' for row in current[len(original):]), current[len(original):]
                rejected = raw('session.create', {'sessionId': session, 'workspaceId': other})['result']
                assert rejected['ok'] is False and rejected['error']['code'] == 'session-conflict', rejected
                missing = raw('session.create', {'sessionId': session, 'cwd': str(work / 'missing')})['result']
                assert missing['ok'] is False and missing['error']['code'] == 'session-conflict', missing
                evidence[phase] = {'sameSessionRestored': True, 'historyUnchanged': True, 'rawLogPrefixUnchanged': True, 'durabilityBarrier': 'session.export', 'foreignWorkspaceRejected': True, 'missingPathRejected': True}
        evidence['passed'] = True
    finally:
        (work / 'evidence.json').write_text(json.dumps(evidence, indent=2), encoding='utf-8')
    print('PASS ordinary session cwd resumes through canonical workspace before and after restart; foreign and missing paths reject')


if __name__ == '__main__':
    main()
