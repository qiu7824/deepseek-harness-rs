import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  final fixture = object(
    jsonDecode(
      File('../../test-fixtures/reliability/session-replay.json')
          .readAsStringSync(),
    ),
  );
  final expected = object(fixture['expected']),
      faults = object(fixture['faults']);
  final events = objects(fixture['events']).map(HistoryEvent.fromJson).toList();
  HistoryPage page(Iterable<HistoryEvent> rows) => HistoryPage.fromJson({
    'events': rows.map((e) => {'event': e.raw}).toList(),
    'hasMoreBefore': false,
    'hasMoreAfter': false,
    'firstSeq': rows.first.startSeq,
    'lastSeq': rows.last.endSeq,
  });
  void verify(ConversationWindow window) {
    final items = window.project();
    expect(
      items.where((item) => item.kind == 'user').map((item) => item.text),
      [expected['prompt'], '开始下一步'],
    );
    expect(
      items.where((item) => item.kind == 'assistant').map((item) => item.text),
      [expected['assistant'], expected['cancelledPartial']],
    );
    expect(
      items.any(
        (item) =>
            item.text.startsWith('任务尚未验收。') &&
            item.status == expected['acceptance'],
      ),
      isTrue,
    );
    expect(items.last.text, '任务已停止。');
    expect(window.lastSeq, events.last.seq);
  }

  test('shared corpus preserves Unicode, long streams, duplicate delivery, acceptance and cancellation', () {
    final window = ConversationWindow();
    for (final event in events) {
      window.append(event);
      if (event.seq % (faults['duplicateEvery'] as int) == 0)
        expect(window.append(event), isFalse);
    }
    verify(window);
    expect(window.hasBefore, isFalse);
    expect(window.needsRefresh, isFalse);
  });
  test(
    'shared corpus gap repair and cold history reconstruct identical messages',
    () {
      final window = ConversationWindow();
      final start = faults['gapStart'] as int, end = faults['gapEnd'] as int;
      for (final event in events.take(start)) {
        window.append(event);
      }
      expect(window.append(events[end]), isFalse);
      expect(window.needsRefresh, isTrue);
      expect(window.lastSeq, start - 1);
      window.replace(page(events));
      verify(window);
      final cold = ConversationWindow()..replace(page(events));
      verify(cold);
    },
  );
  test('raw pages overlapping packed live ranges never duplicate text or evict the prompt', () {
    final window = ConversationWindow();
    for (final event in events.take(6000)) {
      window.append(event);
    }
    window.mergePage(page(events.skip(5000)), older: false);
    verify(window);
    expect(window.needsRefresh, isFalse);
    final partial = HistoryEvent.fromJson({
      'type': 'assistant/chunk',
      'seq': 4,
      'time': 0,
      'data': {
        'turn': 1,
        'step': 1,
        '__historyStartSeq': 4,
        '__historyEndSeq': 9000,
        'chunk': {
          'type': 'text-delta',
          'index': 0,
          'text': 'not safely sliceable',
        },
      },
    });
    final before = window.project().map((item) => item.text).toList();
    window.mergePage(page([partial]), older: false);
    expect(window.needsRefresh, isTrue);
    expect(window.project().map((item) => item.text), before);
  });
}
