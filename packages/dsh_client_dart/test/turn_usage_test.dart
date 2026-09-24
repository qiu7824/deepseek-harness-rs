import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

HistoryEvent e(int seq, int time, String type, Json data) =>
    HistoryEvent.fromJson({
      'seq': seq,
      'time': time,
      'type': type,
      'data': data,
    });

void main() {
  final complete = [
    e(1, 1000, 'turn/start', {'turn': 1}),
    e(2, 1100, 'step/start', {'turn': 1, 'step': 1}),
    e(3, 1800, 'assistant/message', {
      'turn': 1,
      'step': 1,
      'message': {
        'id': 'answer',
        'source': {'provider': 'p', 'model': 'm'},
        'content': [
          {'type': 'text', 'text': '完成'},
        ],
      },
      'usage': {
        'inputTokens': 5,
        'outputTokens': 3,
        'cacheReadTokens': 2,
        'cacheWriteTokens': 0,
        'reasoningTokens': 1,
        'totalTokens': 10,
      },
      'requestMetrics': {
        'firstTokenMs': 200,
        'requestMeasurement': {
          'phase': 'completed',
          'measurement': 'request-average',
          'attemptId': 'a',
          'executionInstanceId': 'instance',
          'networkElapsedMs': 500,
          'outputTokens': 3,
        },
      },
    }),
    e(4, 1801, 'step/end', {'turn': 1, 'step': 1}),
    e(5, 3000, 'turn/end', {'turn': 1}),
  ];

  test(
    'closed turn exposes exact usage, elapsed time and measured request rate',
    () {
      final metrics = deriveTurnFooterMetrics(complete);
      expect(metrics.runMs, 2000);
      expect(metrics.ttftMs, 200);
      expect(metrics.tokensPerSecond, 6);
      expect(metrics.usage, {
        'uncachedInputTokens': 5,
        'outputTokens': 3,
        'totalTokens': 10,
        'cacheReadTokens': 2,
        'cacheWriteTokens': 0,
        'reasoningTokens': 1,
        'routes': [
          {'provider': 'p', 'model': 'm'},
        ],
      });
      final tail = projectTranscript(
        complete,
      ).where((item) => item.kind == 'turn-tail').single;
      expect(tail.runMs, 2000);
      expect(tail.turnUsage?['totalTokens'], 10);
      expect(tail.tokensPerSecond, 6);
    },
  );

  test('incomplete and contradictory usage never produce a guessed total', () {
    final missing = [
      ...complete.take(2),
      e(3, 1800, 'assistant/message', {
        'turn': 1,
        'step': 1,
        'message': {
          'id': 'answer',
          'content': [
            {'type': 'text', 'text': '完成'},
          ],
        },
        'usage': {'inputTokens': 5, 'outputTokens': 3, 'cacheReadTokens': 2},
      }),
      ...complete.skip(3),
    ];
    expect(deriveTurnUsage(missing), isNull);
    expect(deriveTurnFooterMetrics(missing).runMs, 2000);
    final contradictory = [...complete];
    contradictory[2] = e(3, 1800, 'assistant/message', {
      ...complete[2].data,
      'usage': {...object(complete[2].data['usage']), 'totalTokens': 9},
    });
    expect(deriveTurnUsage(contradictory), isNull);
    expect(deriveTurnUsage(complete.skip(1)), isNull);
  });

  test(
    'a retry counts both complete attempts and omits unattributed routes',
    () {
      final events = [
        e(1, 1000, 'turn/start', {'turn': 1}),
        e(2, 1100, 'step/start', {'turn': 1, 'step': 1}),
        e(3, 1200, 'assistant/chunk', {
          'turn': 1,
          'step': 1,
          'chunk': {
            'type': 'usage',
            'usage': {
              'inputTokens': 4,
              'outputTokens': 0,
              'cacheReadTokens': 0,
              'cacheWriteTokens': 0,
            },
          },
        }),
        e(4, 1300, 'llm/retry', {'turn': 1, 'step': 1, 'retryId': 'r'}),
        e(5, 1400, 'llm/retry-started', {'turn': 1, 'step': 1, 'retryId': 'r'}),
        e(6, 1700, 'assistant/message', {
          'turn': 1,
          'step': 1,
          'message': {
            'source': {'provider': 'p', 'model': 'm'},
            'content': [
              {'type': 'text', 'text': '完成'},
            ],
          },
          'usage': {
            'inputTokens': 5,
            'outputTokens': 2,
            'cacheReadTokens': 0,
            'cacheWriteTokens': 0,
          },
        }),
        e(7, 1701, 'step/end', {'turn': 1, 'step': 1}),
        e(8, 2000, 'turn/end', {'turn': 1}),
      ];
      expect(deriveTurnUsage(events), {
        'uncachedInputTokens': 9,
        'outputTokens': 2,
        'totalTokens': 11,
        'cacheReadTokens': 0,
        'cacheWriteTokens': 0,
      });
    },
  );
}
