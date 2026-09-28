import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

HistoryEvent delta(int seq, {String type = 'text-delta', int index = 0}) =>
    HistoryEvent.fromJson({
      'type': 'assistant/chunk',
      'seq': seq,
      'time': seq,
      'data': {
        'turn': 1,
        'step': 1,
        'chunk': {'type': type, 'index': index, 'text': '文'},
      },
    });

void main() {
  test('a long live reply keeps the prompt and every received character', () {
    final window = ConversationWindow();
    window.append(
      HistoryEvent.fromJson({
        'type': 'user/message',
        'seq': 0,
        'data': {
          'content': [
            {'type': 'text', 'text': '保留问题'},
          ],
        },
      }),
    );
    for (var seq = 1; seq <= 25000; seq++) {
      window.append(delta(seq));
    }
    final items = window.project();
    expect(items.where((item) => item.kind == 'user').single.text, '保留问题');
    expect(items.last.text, '文' * 25000);
    expect(window.lastSeq, 25000);
    expect(window.hasBefore, isFalse);
    expect(window.needsRefresh, isFalse);
    expect(window.eventCount, lessThan(100));
    final bytes = window.events.fold<int>(0, (n, e) => n + e.retainedBytes);
    expect(window.retainedBytes, bytes);
  });

  test('coalescing preserves block identities, watermarks and final text', () {
    final window = ConversationWindow();
    window.append(delta(0, type: 'reasoning-delta'));
    window.append(delta(1, type: 'reasoning-delta'));
    window.append(delta(2, index: 1));
    window.append(delta(3, index: 1));
    expect(window.project().map((e) => e.text), ['文文', '文文']);
    expect(window.events.map((e) => (e.startSeq, e.endSeq)), [(0, 1), (2, 3)]);
    expect(window.append(delta(3, index: 1)), isFalse);
    final ids = window.project().map((e) => e.id).toList();
    window.append(
      HistoryEvent.fromJson({
        'type': 'assistant/message',
        'seq': 4,
        'data': {
          'turn': 1,
          'step': 1,
          'message': {
            'content': [
              {'type': 'reasoning', 'text': '文文'},
              {'type': 'text', 'text': '文文'},
            ],
          },
        },
      }),
    );
    expect(window.project().map((e) => e.id), ids);
    expect(window.project().map((e) => e.text), ['文文', '文文']);
    expect(window.project().every((e) => !e.streaming), isTrue);
  });
}
