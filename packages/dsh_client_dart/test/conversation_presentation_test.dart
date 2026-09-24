import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

HistoryEvent e(int seq, String type, Json data, {Json? view}) =>
    HistoryEvent.fromJson({
      'seq': seq,
      'type': type,
      'data': data,
      'time': 1000 + seq,
    }, view: view);
void main() {
  test(
    'tool call and result become one labeled row with a truthful summary and output',
    () {
      final items = projectTranscript([
        e(
          1,
          'tool/call',
          {'turn': 1, 'callId': 'a', 'name': 'todo_write', 'arguments': '{}'},
          view: {
            'for': 'call',
            'view': {
              'card': 'generic',
              'title': 'Update todo list',
              'rawInput': [
                {'content': '检查页面', 'status': 'in_progress'},
                {'content': '核对尺寸', 'status': 'pending'},
              ],
            },
          },
        ),
        e(2, 'tool/result', {
          'turn': 1,
          'message': {
            'source': {'callId': 'a'},
            'content': [
              {
                'type': 'tool-result',
                'content': [
                  {'type': 'text', 'text': 'updated'},
                ],
              },
            ],
          },
        }),
      ]);
      expect(items.length, 1);
      expect(items.single.title, '更新任务清单');
      expect(items.single.summary, '0/2 已完成 · 检查页面');
      expect(items.single.status, 'complete');
      expect(items.single.output, 'updated');
    },
  );
  test('failed tools and missing history remain distinguishable', () {
    final items = projectTranscript([
      e(1, 'tool/call', {
        'turn': 1,
        'callId': 'a',
        'name': 'read',
        'arguments': '{}',
      }),
      e(2, 'tool/result', {
        'turn': 1,
        'message': {
          'source': {'callId': 'a'},
          'content': [
            {
              'type': 'tool-result',
              'isError': true,
              'content': [
                {'type': 'text', 'text': 'not found'},
              ],
            },
          ],
        },
      }),
      e(3, 'tool/call', {
        'turn': 2,
        'callId': 'a',
        'name': 'read',
        'arguments': '{}',
      }),
    ], live: false);
    expect(items.length, 2);
    expect(items.first.status, 'failed');
    expect(items.last.status, '');
    expect(items.last.output, isEmpty);
  });
}
