import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

HistoryEvent e(int seq, String type, Json data) => HistoryEvent.fromJson({
  'seq': seq,
  'time': seq * 1000,
  'type': type,
  'data': data,
});
HistoryEvent answer(int seq, int turn, int step, String text) =>
    e(seq, 'assistant/message', {
      'turn': turn,
      'step': step,
      'message': {
        'id': 'message-$seq',
        'content': [
          {'type': 'text', 'text': text},
        ],
      },
    });
void main() {
  test(
    'retry chain keeps a single row and follows started and cancelled events',
    () {
      HistoryEvent retry(int seq, int attempt) => e(seq, 'llm/retry', {
        'turn': 1,
        'step': 1,
        'retryId': 'r',
        'retry': attempt,
        'maxRetries': 2,
        'mode': 'normal',
        'delayMs': 468,
      });
      final events = [
        retry(1, 1),
        e(2, 'llm/retry-started', {
          'turn': 1,
          'step': 1,
          'retryId': 'r',
          'retry': 1,
        }),
      ];
      expect(projectTranscript(events).single.status, 'started');
      events.add(retry(3, 2));
      expect(projectTranscript(events).single.status, 'scheduled');
      expect(projectTranscript(events).single.seq, 1);
      events.add(e(4, 'step/end', {'turn': 1, 'step': 1}));
      expect(projectTranscript(events).single.status, 'cancelled');
    },
  );
  test('generic tool labels and action summaries match Web rows', () {
    final items = projectTranscript([
      e(1, 'tool/call', {
        'turn': 1,
        'callId': 'c',
        'name': 'computer_use',
        'arguments': '{"action":"capture"}',
      }),
      e(2, 'tool/call', {
        'turn': 1,
        'callId': 's',
        'name': 'tool_search',
        'arguments': '{"query":"shell execute"}',
      }),
    ]);
    expect(items.map((i) => i.title), ['工具调用', '工具调用']);
    expect(items.map((i) => i.summary), [
      'computer_use · 截取画面',
      'tool_search · shell execute',
    ]);
    expect(items.map((i) => i.iconKind), ['generic', 'generic']);
  });
  test(
    'intermediate steps and completed requests never produce a turn footer',
    () {
      final events = [
        answer(1, 1, 1, '正在检查'),
        e(2, 'step/end', {'turn': 1, 'step': 1}),
        e(3, 'request/phase', {'turn': 1, 'phase': 'completed'}),
        answer(4, 1, 2, '继续检查'),
      ];
      expect(
        projectTranscript(events).where((i) => i.kind == 'turn-tail'),
        isEmpty,
      );
      events.add(
        e(5, 'turn/end', {
          'turn': 1,
          'reason': {'kind': 'completed'},
        }),
      );
      final items = projectTranscript(events);
      final tail = items.where((i) => i.kind == 'turn-tail').single;
      expect(items.last, same(tail));
      expect(tail.text, '继续检查');
      expect(tail.messageId, 'message-4');
      expect(tail.seq, 4);
      expect(tail.time, 4000);
      expect(tail.status, 'complete');
    },
  );
  test(
    'multiple text blocks share one footer and each closed turn owns its footer',
    () {
      final items = projectTranscript([
        e(1, 'assistant/message', {
          'turn': 1,
          'step': 1,
          'message': {
            'id': 'a',
            'content': [
              {'type': 'text', 'text': '第一段'},
              {'type': 'reasoning', 'text': '思考'},
              {'type': 'text', 'text': '第二段'},
            ],
          },
        }),
        e(2, 'turn/end', {'turn': 1}),
        answer(3, 2, 1, '另一轮'),
        e(4, 'turn/end', {'turn': 2}),
      ]);
      expect(items.map((i) => i.kind), [
        'assistant',
        'reasoning',
        'assistant',
        'turn-tail',
        'assistant',
        'turn-tail',
      ]);
      expect(items[3].text, '第一段\n\n第二段');
      expect(items[3].messageId, 'a');
    },
  );
  test(
    'later tool results or errors make the closing answer unavailable for branching',
    () {
      for (final later in [
        e(2, 'tool/result', {
          'turn': 1,
          'callId': 'c',
          'message': {
            'content': [
              {'type': 'text', 'text': '结果'},
            ],
          },
        }),
        e(2, 'llm/retry', {'turn': 1, 'step': 2}),
        e(2, 'turn/end', {
          'turn': 1,
          'reason': {'kind': 'error'},
        }),
      ]) {
        final items = projectTranscript([
          answer(1, 1, 1, '中间回复'),
          later,
          e(3, 'turn/end', {'turn': 1}),
        ]);
        expect(
          items.where((i) => i.kind == 'turn-tail').single.status,
          'branch-unavailable',
        );
      }
    },
  );
}
