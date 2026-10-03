import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/rich_content.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show MemoryPreferences, FakeClient;

class ReadingClient extends FakeClient {
  int? requestedAfter;
  @override
  Future<HistoryPage> history(
    String id, {
    int? before,
    int? after,
    RequestScope? scope,
  }) {
    requestedAfter = after;
    return super.history(id, before: before, after: after, scope: scope);
  }
}

void main() {
  test(
    'reading bookmarks restore their bounded page and stay scoped to Host',
    () async {
      final api = ReadingClient();
      api.liveSessions = [
        SessionSummary.fromJson({'sessionId': 's'}),
      ];
      api.histories['s'] = Future.value(
        HistoryPage.fromJson({
          'events': [
            {
              'event': {
                'seq': 100,
                'type': 'user/message',
                'data': {
                  'content': [
                    {'type': 'text', 'text': '保存的阅读位置'},
                  ],
                },
              },
            },
          ],
          'firstSeq': 100,
          'lastSeq': 100,
          'hasMoreBefore': true,
          'hasMoreAfter': true,
        }),
      );
      var currentApi = api;
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => currentApi,
      );
      await c.connect('http://127.0.0.1');
      c.rememberReadingPosition(
        's',
        seq: 100,
        itemId: '100',
        viewportOffset: -40,
        follow: false,
      );
      await c.select('s');
      expect(api.requestedAfter, 100);
      expect(c.window.firstSeq, 100);
      expect(c.readingHistory, isTrue);
      for (var i = 0; i < 100; i++) {
        c.rememberReadingPosition(
          's$i',
          seq: i,
          itemId: '$i',
          viewportOffset: -20,
          follow: false,
        );
      }
      expect(c.readingPositions.length, 64);
      c.rememberReadingPosition(
        's99',
        seq: 99,
        itemId: '99',
        viewportOffset: 0,
        follow: true,
      );
      expect(c.readingPositions.containsKey('s99'), isFalse);
      currentApi = ReadingClient();
      await c.connect('http://127.0.0.1:2');
      expect(c.readingPositions, isEmpty);
      c.dispose();
    },
  );
  test(
    'background state does not invalidate selected conversation or theme',
    () {
      final c = DesktopController(MemoryPreferences())..selectedId = 'selected';
      c.sessions = [
        SessionSummary.fromJson({'sessionId': 'selected'}),
        SessionSummary.fromJson({'sessionId': 'background'}),
      ];
      c.emit();
      final revision = c.conversationChanges.value;
      var themeChanges = 0;
      c.themeChanges.addListener(() => themeChanges++);
      c.sessions[1].running = true;
      c.emit();
      expect(c.conversationChanges.value, revision);
      expect(themeChanges, 0);
      c.transcript = [
        TranscriptItem(id: 'response', kind: 'assistant', text: '回复'),
      ];
      c.messageChanges.value++;
      c.sessions[1].running = false;
      c.emit();
      expect(c.conversationChanges.value, revision);
      c.sessions[0].running = true;
      c.emit();
      expect(c.conversationChanges.value, revision + 1);
      c.preferences.dark = true;
      c.emit();
      expect(themeChanges, 1);
      c.dispose();
    },
  );

  test(
    'body size persists within readable bounds and invalid values fall back',
    () async {
      final c = DesktopController(DesktopPreferences(writer: (_) async {}));
      expect(c.bodyFontSize, 15);
      await c.setBodyFontSize(20);
      expect(c.bodyFontSize, 18);
      await c.setBodyFontSize(10);
      expect(c.bodyFontSize, 14);
      c.preferences.layout['bodyFontSize'] = 'invalid';
      expect(c.bodyFontSize, 15);
      c.dispose();
    },
  );

  testWidgets('background refresh reuses completed markdown layout', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1280, 800));
    final c = DesktopController(MemoryPreferences())..selectedId = 'selected';
    c.sessions = [
      SessionSummary.fromJson({'sessionId': 'selected'}),
      SessionSummary.fromJson({'sessionId': 'background'}),
    ];
    c.transcript = [
      TranscriptItem(
        id: 'response',
        kind: 'assistant',
        text: '已完成的正文\n\n- 第一项',
      ),
    ];
    c.emit();
    await tester.pumpWidget(DesktopApp(controller: c));
    await tester.pumpAndSettle();
    final before = tester
        .widgetList<DshMarkdownBlock>(find.byType(DshMarkdownBlock))
        .toList();
    expect(before, isNotEmpty);
    c.sessions[1].running = true;
    c.emit();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 250));
    final after = tester
        .widgetList<DshMarkdownBlock>(find.byType(DshMarkdownBlock))
        .toList();
    expect(after.length, before.length);
    for (var i = 0; i < before.length; i++) {
      expect(identical(after[i], before[i]), isTrue);
    }
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await tester.binding.setSurfaceSize(null);
  });
}
