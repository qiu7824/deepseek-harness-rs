import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  test(
    'selected model uses display metadata without losing an unavailable route',
    () {
      final catalog = ModelCatalog.fromJson({
        'current': {'provider': 'p', 'model': 'machine-id'},
        'groups': [
          {
            'id': 'p',
            'name': '供应商名称',
            'models': [
              {'id': 'machine-id', 'name': '可读模型名称'},
            ],
          },
        ],
      });
      expect(catalog.currentName, '可读模型名称');
      expect(catalog.providerNames['p'], '供应商名称');
      expect(
        ModelCatalog.fromJson({
          'current': {'provider': 'p', 'model': 'removed-model'},
        }).currentName,
        'removed-model',
      );
    },
  );
  test(
    'history retention includes presentation payloads as well as raw events',
    () {
      final history = ConversationWindow(maxBytes: 3000, maxEvents: 1000);
      for (var i = 0; i < 100; i++) {
        history.append(
          HistoryEvent.fromJson(
            {
              'seq': i,
              'type': 'tool/call',
              'data': {'name': 'read', 'arguments': '{}'},
            },
            view: {'preview': 'x' * 700},
          ),
        );
        expect(history.retainedBytes, lessThanOrEqualTo(3000));
      }
      expect(history.eventCount, lessThanOrEqualTo(4));
      expect(history.events.last.seq, 99);
      expect(history.hasBefore, isTrue);
    },
  );
  test(
    'late history and out-of-order events never rewind a live projection',
    () {
      final p = ProjectionWindow();
      final started = p.version;
      p.apply('sessionStats', {'steps': 20}, 20);
      p.snapshot({
        'asOfSeq': 10,
        'values': {
          'sessionStats': {'steps': 10},
          'contextPressure': {'projectedTokens': 30, 'contextWindow': 100},
        },
      }, requestVersion: started);
      expect(object(p.values['sessionStats'])['steps'], 20);
      expect(p.apply('sessionStats', {'steps': 19}, 19), isFalse);
      expect(object(p.values['contextPressure'])['projectedTokens'], 30);
    },
  );
  test(
    'same-sequence live change received during a request wins over its snapshot',
    () {
      final p = ProjectionWindow();
      p.apply('contextPressure', {'projectedTokens': 90}, 4);
      final started = p.version;
      p.apply('contextPressure', {'projectedTokens': 20}, 4);
      p.snapshot({
        'asOfSeq': 4,
        'values': {
          'contextPressure': {'projectedTokens': 90},
        },
      }, requestVersion: started);
      expect(object(p.values['contextPressure'])['projectedTokens'], 20);
    },
  );
  test(
    'snapshot removals are durable until a newer update and session reset clears cursors',
    () {
      final p = ProjectionWindow();
      p.apply('goal', {
        'goal': {'id': 'old'},
      }, 5);
      p.snapshot({'asOfSeq': 8, 'values': {}}, requestVersion: p.version);
      expect(p.values, isEmpty);
      expect(
        p.apply('goal', {
          'goal': {'id': 'old'},
        }, 7),
        isFalse,
      );
      p.clear();
      expect(
        p.apply('goal', {
          'goal': {'id': 'new'},
        }, 0),
        isTrue,
      );
      expect(object(object(p.values['goal'])['goal'])['id'], 'new');
    },
  );
  test(
    'projection retention has fixed keys and a strict aggregate byte budget',
    () {
      final p = ProjectionWindow(maxBytes: 256);
      for (var i = 0; i < 10000; i++) {
        expect(p.apply('extension-$i', 'ignored', i), isFalse);
        p.apply('sessionStats', {'steps': i}, i);
        p.apply('contextInsights', {'reasoningTokens': i}, i);
        expect(p.retainedBytes, lessThanOrEqualTo(256));
      }
      p.apply('goal', {'objective': 'x' * 300}, 10000);
      expect(p.values.containsKey('goal'), isFalse);
      expect(p.oversized, contains('goal'));
      expect(p.values.length, 2);
      p.clear();
      expect(p.retainedBytes, 0);
      expect(p.oversized, isEmpty);
    },
  );
  test(
    'context meter preserves zero after compaction and rejects unavailable capacity',
    () {
      expect(
        ContextOccupancy.fromJson({
          'pressureTokens': 90,
          'projectedTokens': 0,
          'contextWindow': 100,
        })!.percent,
        0,
      );
      expect(
        ContextOccupancy.fromJson({
          'pressureTokens': 130,
          'contextWindow': 100,
        })!.percent,
        100,
      );
      expect(
        ContextOccupancy.fromJson({'projectedTokens': 30, 'contextWindow': 0}),
        isNull,
      );
      expect(ContextOccupancy.fromJson({'contextWindow': 100}), isNull);
    },
  );
  test(
    'cache ratio uses only provider-reported samples rather than all billed input',
    () {
      final usage = {
        'uncachedInputTokens': 1000,
        'cacheReadTokens': 80,
        'cacheWriteTokens': 20,
        'outputTokens': 5,
        'cacheStatistics': {
          'reportedSamples': 1,
          'unreportedSamples': 5,
          'reportedInputTokens': 100,
        },
      };
      expect(billedInputTokens(usage), 1100);
      expect(cacheHitPercent(usage), 80);
      expect(
        cacheHitPercent({'cacheReadTokens': 80, 'uncachedInputTokens': 20}),
        isNull,
      );
      expect(
        cacheHitPercent({
          'cacheReadTokens': 80,
          'cacheStatistics': {'reportedSamples': 0, 'reportedInputTokens': 100},
        }),
        isNull,
      );
    },
  );
}
