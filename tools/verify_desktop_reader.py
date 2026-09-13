"""Verify the Windows native reader with an isolated, network-free session."""
from __future__ import annotations

import argparse
import json
import pathlib
import subprocess


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    parser.add_argument('--workdir', type=pathlib.Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    from verify_release_version import workspace_version
    version = subprocess.check_output([str(binary), '--version'], text=True, timeout=15).strip()
    assert version == 'dsh-desktop ' + workspace_version(), version
    root = args.workdir.resolve()
    root.mkdir(parents=True, exist_ok=True)
    fixture = root / 'fixture.json'
    fixture.write_text(json.dumps({'sessions': [{'sessionId': 'reader-fixture', 'cwd': str(root),
        'projections': {'values': {'title': 'Native reader verification'}}}], 'history': {'events': [
        {'event': {'seq': 1, 'type': 'user/message', 'data': {'source': {'kind': 'user'}, 'content': [{'type': 'text', 'text': '中文阅读检查'}]}}},
        {'event': {'seq': 2, 'type': 'assistant/message', 'data': {'message': {'content': [{'type': 'text', 'text': '# 阅读\n\n正文 **重点** 和 `code`。\n\n| 名称 | 状态 |\n|---|---|\n| 原生 | 完成 |'}]}}}}
    ]}}, ensure_ascii=False), encoding='utf-8')
    for name, extra in [('light', []), ('dark-narrow', ['--dark', '--width', '760', '--height', '600'])]:
        output = root / name
        subprocess.run([str(binary), '--fixture', str(fixture), '--smoke', str(output), *extra], check=True, timeout=45)
        report = json.loads((output / 'report.json').read_text(encoding='utf-8'))
        assert report['sessionCount'] == 1 and report['messageCount'] == 2, report
        assert not report['loadError'] and not report['loading'], report
        assert report['textLayoutBytes'] <= 4 * 1024 * 1024 and report['glyphCacheBytes'] <= 4 * 1024 * 1024
        assert (output / 'window.png').read_bytes().startswith(b'\x89PNG\r\n\x1a\n')
    print('Native reader: matching version, isolated history, light/dark windows and bounded caches passed')


if __name__ == '__main__':
    main()
