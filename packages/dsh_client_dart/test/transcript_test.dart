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
  test(
    'feedback targets only stored assistant identity, never display or replacement IDs',
    () {
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
    },
  );
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
}
