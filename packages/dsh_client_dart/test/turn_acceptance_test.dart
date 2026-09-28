import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  test(
    'durable acceptance survives replay without rewriting the model answer',
    () {
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
            'type': 'turn/end',
            'seq': 2,
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
            items.any(
              (item) => item.kind == 'assistant' && item.text == '已完成。',
            ),
            isTrue,
          );
          final notice = items.singleWhere((item) => item.title == '任务验收');
          expect(notice.status, status);
          expect(notice.kind, status == 'verified' ? 'notice' : 'error');
          expect(notice.text, contains('检查项未通过'));
        }
      }
    },
  );
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
