import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

HistoryEvent event(int seq, String type, Json data) => HistoryEvent.fromJson({
  'seq': seq,
  'time': seq,
  'type': type,
  'data': data,
});

List<HistoryEvent> permission(int seq, String preset, {bool error = false}) => [
  event(seq, 'command/run', {'commandId': '$seq', 'name': 'permission'}),
  event(seq + 1, 'command/done', {
    'commandId': '$seq',
    'kind': error ? 'error' : 'success',
    'text': error ? 'unknown preset "$preset"' : 'preset $preset',
  }),
];

void main() {
  test('historical permission receipts collapse with localized details', () {
    final items = projectTranscript([
      ...permission(0, 'danger-full-access'),
      ...permission(2, 'danger-full-access'),
      ...permission(4, 'workspace-write'),
      ...permission(6, 'danger-full-access'),
    ]);
    expect(items, hasLength(1));
    expect(items.single.title, '权限已切换');
    expect(items.single.summary, '完全访问 · 4 次变更');
    expect(items.single.text.split('\n'), hasLength(4));
    expect(items.single.text, contains('工作区内修改'));
    expect(items.single.text, isNot(contains('preset')));
    expect(items.single.seq, 7);
  });

  test('failures and user messages delimit permission groups', () {
    final items = projectTranscript([
      ...permission(0, 'danger-full-access'),
      ...permission(2, 'invalid', error: true),
      ...permission(4, 'read-only'),
      event(6, 'user/message', {
        'source': {'kind': 'user'},
        'content': [
          {'type': 'text', 'text': '继续工作'},
        ],
      }),
      ...permission(7, 'workspace-write'),
    ]);
    expect(items, hasLength(5));
    expect(items[1].status, 'failed');
    expect(items[1].title, '权限切换失败');
    expect(items[1].text, 'unknown preset "invalid"');
    expect(items[2].summary, '只读');
    expect(items.last.summary, '工作区内修改');
  });

  test('other commands and custom permission details remain available', () {
    final items = projectTranscript([
      event(0, 'command/run', {'commandId': 'x', 'name': 'help'}),
      event(1, 'command/done', {
        'commandId': 'x',
        'text': 'available commands',
      }),
      ...permission(2, 'review'),
    ]);
    expect(items.first.title, 'help');
    expect(items.first.text, 'available commands');
    expect(items.last.summary, '自定义权限（review）');
  });
}
