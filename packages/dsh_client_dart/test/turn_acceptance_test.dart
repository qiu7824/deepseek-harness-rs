import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  test('retired acceptance metadata stays hidden without rewriting the model answer', () {
    for (final status in [
      'verified',
      'incomplete',
      'blocked',
      'cancelled',
      'unverified',
    ]) {
      final events = [
        HistoryEvent.fromJson({
          'type': 'assistant/message',
          'seq': 1,
          'data': {
            'turn': 1,
            'step': 1,
            'message': {
              'id': 'answer',
              'content': [
                {'type': 'text', 'text': '已完成。'},
              ],
            },
          },
        }),
        HistoryEvent.fromJson({
          'type': 'tool/call',
          'seq': 2,
          'data': {
            'turn': 1,
            'step': 1,
            'callId': 'retired',
            'name': 'task_execution',
            'arguments': '{}',
          },
        }),
        HistoryEvent.fromJson({
          'type': 'tool/result',
          'seq': 3,
          'data': {
            'turn': 1,
            'step': 1,
            'message': {
              'role': 'tool',
              'name': 'task_execution',
              'source': {'kind': 'tool', 'callId': 'retired'},
              'content': [
                {'type': 'text', 'text': 'legacy task payload'},
              ],
            },
          },
        }),
        HistoryEvent.fromJson({
          'type': 'turn/end',
          'seq': 4,
          'data': {
            'turn': 1,
            'reason': {'kind': 'completed'},
            'acceptance': {
              'status': status,
              'summary': '权威验收结果',
              'blockers': ['检查项未通过'],
            },
          },
        }),
      ];
      for (final live in [true, false]) {
        final items = projectTranscript(events, live: live);
        expect(
          items.any((item) => item.kind == 'assistant' && item.text == '已完成。'),
          isTrue,
        );
        expect(items.where((item) => item.title == '任务验收'), isEmpty);
        expect(items.any((item) => item.text.contains('权威验收结果')), isFalse);
        expect(
          items.where((item) => item.kind == 'tool' || item.kind == 'result'),
          isEmpty,
        );
      }
    }
  });
  test('legacy and cancelled turns never infer a successful acceptance', () {
    for (final reason in ['completed', 'aborted', 'error']) {
      final items = projectTranscript([
        HistoryEvent.fromJson({
          'type': 'turn/end',
          'seq': 1,
          'data': {
            'turn': 1,
            'reason': {'kind': reason},
          },
        }),
      ]);
      expect(items.where((item) => item.title == '任务验收'), isEmpty);
    }
  });
}
