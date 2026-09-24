import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

HistoryEvent e(int seq, String type, Json data, [int? time]) =>
    HistoryEvent.fromJson({
      'seq': seq,
      'type': type,
      'data': data,
      'time': time,
    });
Json text(String value) => {
  'content': [
    {'type': 'text', 'text': value},
  ],
};
void main() {
  test('incomplete historical windows do not masquerade as live requests', () {
    final events = [
      e(1, 'step/start', {'turn': 1, 'step': 1}, 100),
    ];
    final historical = TraceSnapshot.fromEvents(events, live: false);
    expect(historical.records.single.status, 'unavailable');
    expect(historical.records.single.durationMs, isNull);
    expect(historical.spans(actualDuration: true, now: 1000).single.end, 0);
    final live = TraceSnapshot.fromEvents(events);
    expect(live.spans(actualDuration: true, now: 1000).single.end, 1);
  });
  test(
    'semantic rows match tool identities, retain context and replace streaming text',
    () {
      final events = [
        e(1, 'turn/start', {'turn': 1}, 100),
        e(2, 'step/start', {'turn': 1, 'step': 1}, 110),
        e(3, 'user/message', {
          'source': {'kind': 'user'},
          ...text('question'),
        }, 111),
        e(4, 'user/message', {
          'source': {'kind': 'plugin'},
          ...text('context'),
        }, 112),
        e(5, 'assistant/chunk', {
          'turn': 1,
          'step': 1,
          'chunk': {'type': 'text-delta', 'text': 'partial'},
        }, 130),
        e(6, 'assistant/message', {
          'turn': 1,
          'step': 1,
          'message': text('final'),
        }, 160),
        e(7, 'tool/call', {
          'turn': 1,
          'step': 1,
          'callId': 'a',
          'name': 'read',
          'arguments': 'first',
        }, 170),
        e(8, 'tool/call', {
          'turn': 1,
          'step': 1,
          'callId': 'b',
          'name': 'read',
          'arguments': 'second',
        }, 180),
        e(9, 'tool/result', {
          'turn': 1,
          'step': 1,
          'message': {
            'source': {'callId': 'b'},
            'content': [
              {'type': 'tool-result', ...text('second result')},
            ],
          },
        }, 200),
        e(10, 'tool/result', {
          'turn': 1,
          'step': 1,
          'message': {
            'source': {'callId': 'a'},
            'content': [
              {'type': 'tool-result', ...text('first result')},
            ],
          },
        }, 210),
      ];
      final snapshot = TraceSnapshot.fromEvents(events),
          rows = snapshot.records;
      expect(rows.map((r) => r.kind), [
        'user',
        'context',
        'assistant',
        'tool',
        'tool',
      ]);
      expect(rows[2].preview, 'final');
      expect(rows[2].durationMs, 50);
      expect(rows[2].firstTokenTime, 130);
      expect(rows[3].preview, contains('first result'));
      expect(rows[3].preview, isNot(contains('second result')));
      expect(rows[3].durationMs, 40);
      expect(identical(rows[3].events.first, events[6]), isTrue);
      expect(traceDisplayRows(snapshot, search: 'second result').length, 1);
      expect(
        traceDisplayRows(snapshot, collapseTurns: true).single.summaryCount,
        5,
      );
      final folded = traceDisplayRows(snapshot, collapseCalls: true);
      expect(folded.length, 3);
      expect(folded.last.summaryKind, 'request');
      expect(folded.last.summaryCount, 3);
    },
  );
  test('partial history never fabricates tool start time or duration', () {
    final row = TraceSnapshot.fromEvents([
      e(20, 'tool/result', {
        'turn': 3,
        'message': {
          'source': {'callId': 'missing'},
          ...text('result'),
        },
      }, 200),
    ]).records.single;
    expect(row.startTime, isNull);
    expect(row.durationMs, isNull);
    expect(row.status, 'complete');
    final absent = TraceSnapshot.fromEvents([
      e(1, 'user/message', text('no timestamp')),
    ]);
    expect(absent.spans(actualDuration: true), isEmpty);
    expect(absent.spans(actualDuration: false), hasLength(1));
  });
  test(
    'retries remain failed operations and interrupted steps do not look successful',
    () {
      final snapshot = TraceSnapshot.fromEvents([
        e(1, 'step/start', {'turn': 1, 'step': 1}, 100),
        e(2, 'llm/retry', {
          'turn': 1,
          'step': 1,
          'message': 'rate limited',
        }, 120),
        e(3, 'assistant/chunk', {
          'turn': 1,
          'step': 1,
          'chunk': {'type': 'text-delta', 'text': 'retry'},
        }, 140),
        e(4, 'turn/end', {'turn': 1}, 160),
      ]);
      expect(snapshot.records.map((r) => r.status), ['failed', 'interrupted']);
      expect(snapshot.records.last.startTime, isNull);
    },
  );
  test('nested tools and compaction are separately identifiable', () {
    final snapshot = TraceSnapshot.fromEvents([
      e(1, 'turn/start', {'turn': 2}, 100),
      e(2, 'tool/call', {'callId': 'root', 'name': 'code'}, 110),
      e(3, 'tool/code-dispatch-start', {
        'rootCallId': 'root',
        'parentCallId': 'root',
        'subCallId': 'child',
        'name': 'read',
      }, 120),
      e(4, 'tool/code-dispatch', {
        'subCallId': 'child',
        'content': [
          {'type': 'text', 'text': 'child output'},
        ],
      }, 150),
      e(5, 'compaction/start', {'compactionId': 'compact'}, 160),
      e(6, 'compaction/summary', {
        'compactionId': 'compact',
        'summary': 'summary',
      }, 180),
      e(7, 'compaction/end', {'compactionId': 'compact'}, 190),
      e(8, 'turn/end', {'turn': 2}, 200),
    ]);
    expect(snapshot.records.map((r) => r.kind), [
      'tool',
      'subtool',
      'compaction',
    ]);
    expect(snapshot.records[1].parentCallId, 'root');
    expect(snapshot.records[1].durationMs, 30);
    expect(snapshot.records[2].preview, 'summary');
    expect(snapshot.records[0].status, 'interrupted');
  });
  test(
    'timeline preserves actual relative time instead of equal operation widths',
    () {
      final snapshot = TraceSnapshot.fromEvents([
        e(1, 'user/message', text('one'), 100),
        e(2, 'user/message', text('two'), 200),
        e(3, 'user/message', text('three'), 1100),
      ]);
      expect(snapshot.spans(actualDuration: true).map((s) => s.start), [
        0,
        .1,
        1,
      ]);
      expect(snapshot.spans(actualDuration: false).map((s) => s.start), [
        0,
        1 / 3,
        2 / 3,
      ]);
    },
  );
  test(
    'large nested previews are capped and unchanged headers are not duplicated',
    () {
      final header = {
        'reason': 'initial',
        'header': {
          'system': 'system',
          'tools': [
            {
              'name': 'read',
              'parameters': {'type': 'object'},
            },
          ],
        },
      };
      final snapshot = TraceSnapshot.fromEvents([
        e(1, 'request/header', header, 1),
        e(2, 'request/header', {
          'header': {
            'system': 'system',
            'tools': [
              {
                'name': 'read',
                'parameters': {'type': 'object'},
              },
            ],
          },
        }, 2),
        e(3, 'assistant/message', {'message': text('a' * 1000000)}, 3),
      ]);
      expect(snapshot.records.where((r) => r.kind == 'system'), hasLength(1));
      expect(snapshot.records.last.preview.length, lessThanOrEqualTo(512));
    },
  );
}
