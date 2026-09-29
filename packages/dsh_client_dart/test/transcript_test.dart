import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

HistoryEvent event(int seq, String type, Json data) =>
    HistoryEvent.fromJson({'seq': seq, 'time': 0, 'type': type, 'data': data});
HistoryPage page(
  List<HistoryEvent> events, {
  bool older = false,
  bool newer = false,
  int? last,
}) => HistoryPage.fromJson({
  'events': events.map((e) => {'event': e.raw}).toList(),
  'hasMoreBefore': older,
  'hasMoreAfter': newer,
  'firstSeq': events.firstOrNull?.seq,
  'lastSeq': last ?? events.lastOrNull?.endSeq,
});

void main() {
  test('feedback targets only stored assistant identity, never display or replacement IDs', () {
    HistoryEvent answer(Json message, [Object? surface]) =>
        HistoryEvent.fromJson({
          'seq': 4,
          'type': 'assistant/message',
          'surfaceOp': surface,
          'data': {'messageId': 'not-authoritative', 'message': message},
        });
    final content = [
      {'type': 'text', 'text': 'answer'},
    ];
    expect(
      projectTranscript([
        answer({'content': content}),
      ]).single.messageId,
      isNull,
    );
    expect(
      projectTranscript([
        answer({'id': 'real', 'content': content}, 'append'),
      ]).single.messageId,
      'real',
    );
    expect(
      projectTranscript([
        answer(
          {'id': 'replacement', 'content': content},
          {'op': 'replace', 'start': 0, 'end': 3},
        ),
      ]).single.messageId,
      isNull,
    );
  });
  test('Host context injections are not displayed as human messages', () {
    final items = projectTranscript([
      event(0, 'user/message', {
        'source': {'kind': 'plugin'},
        'content': [
          {'type': 'text', 'text': 'internal context'},
        ],
      }),
      event(1, 'user/message', {
        'source': {'kind': 'user'},
        'content': [
          {'type': 'text', 'text': 'actual prompt'},
        ],
      }),
      event(2, 'turn/end', {
        'reason': {
          'kind': 'aborted',
          'reason': {'kind': 'user'},
        },
      }),
    ]);
    expect(items.where((e) => e.kind == 'user').map((e) => e.text), [
      'actual prompt',
    ]);
    expect(
      items.where((e) => e.kind == 'context').single.text,
      'internal context',
    );
    expect(items.last.text, '任务已停止。');
  });
  test(
    'automatic context updates have clear labels and preserve every body',
    () {
      final sources = [
        {'kind': 'runtime-context'},
        {'kind': 'plugin', 'plugin': '@deepseek-ai/dsh-system-prompt'},
        {'kind': 'repeat-tool-reminder'},
        {'kind': 'plugin', 'plugin': 'repeat-tool-reminder'},
        {'kind': 'plugin', 'plugin': 'third-party'},
      ];
      final items = projectTranscript([
        for (var i = 0; i < sources.length; i++)
          event(i, 'user/message', {
            'source': sources[i],
            'content': [
              {'type': 'text', 'text': 'durable context $i'},
            ],
          }),
      ]);
      expect(items.map((e) => e.title), [
        '运行信息更新',
        '运行信息更新',
        '重复调用提醒',
        '重复调用提醒',
        '补充上下文',
      ]);
      expect(items.take(2).map((e) => e.summary), everyElement('自动同步，无需操作'));
      expect(items.last.summary, 'third-party');
      expect(
        items.map((e) => e.text),
        List.generate(5, (i) => 'durable context $i'),
      );
      expect(items.map((e) => e.kind), everyElement('context'));
    },
  );
  test('final assistant replaces streamed chunks without duplicate text', () {
    final events = [
      event(0, 'user/message', {
        'content': [
          {'type': 'text', 'text': '你好'},
        ],
      }),
      event(1, 'assistant/chunk', {
        'turn': 1,
        'step': 1,
        'chunk': {'type': 'text-delta', 'index': 0, 'text': '你'},
      }),
      event(2, 'assistant/chunk', {
        'turn': 1,
        'step': 1,
        'chunk': {'type': 'text-delta', 'index': 0, 'text': '好'},
      }),
    ];
    expect(projectTranscript(events).last.text, '你好');
    final streamingId = projectTranscript(events).last.id;
    events.add(
      event(3, 'assistant/message', {
        'turn': 1,
        'step': 1,
        'message': {
          'id': 'assistant-message-1',
          'content': [
            {'type': 'text', 'text': '你好！'},
          ],
        },
      }),
    );
    final items = projectTranscript(events);
    expect(items.length, 2);
    expect(items.last.text, '你好！');
    expect(items.last.streaming, isFalse);
    expect(items.last.id, streamingId);
    expect(items.last.messageId, 'assistant-message-1');
  });
  test('coalesced history watermark prevents replay duplication', () {
    final window = ConversationWindow();
    window.replace(
      page([
        event(3, 'assistant/chunk', {
          '__historyStartSeq': 3,
          '__historyEndSeq': 8,
          'turn': 1,
          'step': 1,
          'chunk': {'type': 'text-delta', 'index': 0, 'text': 'hello'},
        }),
      ], last: 8),
    );
    expect(window.append(event(7, 'step/start', {})), isFalse);
    expect(window.append(event(9, 'step/end', {})), isTrue);
    expect(window.lastSeq, 9);
  });
  test('gap requests a snapshot without advancing past missing events', () {
    final window = ConversationWindow()
      ..replace(page([event(1, 'step/start', {})]));
    expect(window.append(event(3, 'step/end', {})), isFalse);
    expect(window.lastSeq, 1);
    expect(window.needsRefresh, isTrue);
  });
  test('reading an older page buffers only a refresh marker', () {
    final window = ConversationWindow()
      ..replace(page([event(1, 'step/start', {})], newer: true));
    for (var seq = 2; seq < 500; seq++) {
      window.append(event(seq, 'step/end', {}));
    }
    expect(window.events.length, 1);
    expect(window.lastSeq, 1);
    expect(window.needsRefresh, isTrue);
  });
  test('event and byte budgets bound retained history', () {
    final window = ConversationWindow(maxEvents: 2, maxBytes: 400);
    window.replace(page([event(0, 'step/start', {})]));
    for (var seq = 1; seq < 100; seq++) {
      window.append(event(seq, 'step/end', {}));
    }
    expect(window.events.length, lessThanOrEqualTo(2));
    expect(window.hasBefore, isTrue);
    expect(window.needsRefresh, isFalse);
    window.append(
      event(100, 'user/message', {
        'content': [
          {'type': 'text', 'text': 'x' * 1000},
        ],
      }),
    );
    expect(window.events.length, 2);
    expect(window.oversizedEventSeq, 100);
    expect(window.lastSeq, 100);
    expect(window.needsRefresh, isFalse);
    expect(window.append(event(101, 'step/end', {})), isTrue);
    expect(window.needsRefresh, isFalse);
  });
  group('live token streams', () {
    HistoryEvent chunk(int seq, String type, String text, {int step = 1}) =>
        HistoryEvent.fromJson({
          'seq': seq,
          'time': 1000 + seq,
          'type': 'assistant/chunk',
          'data': {
            'turn': 1,
            'step': step,
            'chunk': {'type': type, 'index': 0, 'text': text},
          },
        });
    HistoryEvent answer(int seq, Object sources, {int step = 1}) =>
        HistoryEvent.fromJson({
          'seq': seq,
          'time': 1000 + seq,
          'type': 'assistant/message',
          'surfaceOp': 'append',
          'sourceEventSeqs': sources,
          'data': {
            'turn': 1,
            'step': step,
            'message': {
              'id': 'm$step',
              'content': [
                {'type': 'reasoning', 'text': 'thinking'},
              ],
            },
          },
        });
    ConversationWindow started({int maxEvents = 64}) =>
        ConversationWindow(maxEvents: maxEvents)..replace(
          page([
            event(0, 'user/message', {
              'content': [
                {'type': 'text', 'text': 'question'},
              ],
            }),
            event(1, 'turn/start', {'turn': 1}),
          ]),
        );

    test('one block keeps bounded runs and their first token time', () {
      final window = started();
      for (var seq = 2; seq < 20002; seq++) {
        expect(window.append(chunk(seq, 'reasoning-delta', 't')), isTrue);
      }
      expect(window.eventCount, 7);
      expect(window.hasBefore, isFalse);
      final retained = window.events.elementAt(2);
      expect(retained.seq, 2);
      expect(retained.raw['time'], 1002);
      expect(window.events.last.endSeq, 20001);
      for (final run in window.events.skip(2)) {
        expect(
          object(run.data['chunk'])['text'].length,
          lessThanOrEqualTo(4096),
        );
        expect(run.raw['time'], 1000 + run.startSeq);
      }
      expect(window.lastSeq, 20001);
      final items = window.project();
      expect(items.first.text, 'question');
      expect(items.last.text, 't' * 20000);
      expect(items.last.streaming, isTrue);
      window.append(event(20002, 'request/phase', {'turn': 1, 'step': 1}));
      window.append(chunk(20003, 'reasoning-delta', 'u'));
      window.append(chunk(20004, 'text-delta', 'answer'));
      expect(window.eventCount, 10);
      expect(
        window.project().where((i) => i.kind == 'reasoning').single.text,
        '${'t' * 20000}u',
      );
    });

    test('a completed message releases the chunks it cites', () {
      for (final sources in [
        [for (var seq = 2; seq < 300; seq++) seq],
        [
          [2, 299],
        ],
      ]) {
        final window = started(maxEvents: 1024);
        for (var seq = 2; seq < 300; seq++) {
          window.append(
            chunk(seq, seq.isEven ? 'reasoning-delta' : 'usage', 'x'),
          );
        }
        window.append(answer(300, sources));
        expect(window.events.map((e) => e.type), [
          'user/message',
          'turn/start',
          'assistant/message',
        ]);
        expect(window.project().map((i) => i.text), ['question', 'thinking']);
      }
    });

    test(
      'completed messages release only fully covered runs of their step',
      () {
        for (final sources in [
          [
            [2, 5],
          ],
          [
            [5, 10],
          ],
          [
            [2, 5],
            [7, 10],
          ],
          [
            [2.5, 10],
          ],
        ]) {
          final window = started();
          for (var seq = 2; seq <= 10; seq++) {
            window.append(chunk(seq, 'reasoning-delta', 'r'));
          }
          window.append(answer(11, sources));
          expect(
            window.events.where((e) => e.type == 'assistant/chunk'),
            hasLength(1),
          );
        }
        for (final step in [1, 2]) {
          final window = started();
          for (var seq = 2; seq <= 10; seq++) {
            window.append(chunk(seq, 'reasoning-delta', 'r'));
          }
          window.append(
            answer(11, [
              [6, 10],
              [2, 4],
              5,
            ], step: step),
          );
          expect(
            window.events.where((e) => e.type == 'assistant/chunk'),
            hasLength(step == 1 ? 0 : 1),
          );
        }
        final window = started();
        window.append(chunk(2, 'reasoning-delta', 'r'));
        final otherTurn = answer(3, [2]);
        window.append(
          HistoryEvent.fromJson({
            ...otherTurn.raw,
            'data': {...otherTurn.data, 'turn': 2},
          }),
        );
        expect(
          window.events.where((e) => e.type == 'assistant/chunk'),
          hasLength(1),
        );
      },
    );

    test('a long turn no longer evicts the question it answers', () {
      final window = started(maxEvents: 256);
      var seq = 2;
      for (var step = 1; step <= 40; step++) {
        final first = seq;
        window.append(event(seq++, 'step/start', {'turn': 1, 'step': step}));
        for (var token = 0; token < 400; token++) {
          window.append(
            chunk(
              seq++,
              token % 100 == 99 ? 'usage' : 'reasoning-delta',
              'r',
              step: step,
            ),
          );
        }
        window.append(
          answer(seq++, [
            [first + 1, seq - 2],
          ], step: step),
        );
        window.append(event(seq++, 'step/end', {'turn': 1, 'step': step}));
      }
      expect(window.hasBefore, isFalse);
      expect(window.eventCount, 2 + 40 * 3);
      final items = window.project();
      expect(items.first.text, 'question');
      expect(items.where((i) => i.kind == 'reasoning'), hasLength(40));
    });
  });
}
