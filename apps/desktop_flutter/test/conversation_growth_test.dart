import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/streaming_presentation.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'workbench_test.dart' show TestController;
import 'controller_test.dart' show FakeClient, MemoryPreferences;

void main() {
  for (final running in [false, true]) {
    testWidgets(
      'wheel reading stays still through live output and resumes at bottom ($running)',
      (tester) async {
        final api = FakeClient();
        final c = DesktopController(
          MemoryPreferences(),
          clientFactory: (_) => api,
        );
        final events = <Json>[
          for (var seq = 0; seq < 30; seq++)
            {
              'event': {
                'seq': seq,
                'type': 'assistant/message',
                'data': {
                  'turn': 1,
                  'step': seq,
                  'message': {
                    'content': [
                      {'type': 'text', 'text': '历史回复 $seq\n\n正文段落'},
                    ],
                  },
                },
              },
            },
        ];
        HistoryPage history() => HistoryPage.fromJson({
          'events': events,
          'firstSeq': 0,
          'lastSeq': events.length - 1,
        });
        api.histories['s'] = Future.value(history());
        api.liveSessions = [
          SessionSummary.fromJson({'sessionId': 's', 'running': running}),
        ];
        await c.connect('http://127.0.0.1');
        await tester.pump();
        await c.select('s');
        addTearDown(() async {
          await tester.pumpWidget(const SizedBox());
          c.dispose();
        });
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(body: Conversation(controller: c)),
          ),
        );
        await tester.pumpAndSettle();
        final list = tester.widget<ListView>(
          find.byKey(const PageStorageKey('messages-s')),
        );
        list.controller!.jumpTo(350);
        await tester.pumpAndSettle();
        final cards = tester
            .widgetList<MessageCard>(find.byType(MessageCard))
            .toList();
        final anchor = find
            .byKey(ValueKey(cards[cards.length ~/ 2].item.id))
            .last;
        final before = tester.getRect(anchor), snapshot = c.transcript;
        final incoming = {
          'seq': 30,
          'type': 'assistant/message',
          'data': {
            'turn': 1,
            'step': 30,
            'message': {
              'content': [
                {'type': 'text', 'text': '新回复'},
              ],
            },
          },
        };
        events.add({'event': incoming});
        api.histories['s'] = Future.value(history());
        api.channels.first.data.add(
          HostFrame.fromJson({
            'type': 'server-request',
            'rpcId': 'live',
            'payload': {
              'type': 'session/event',
              'sessionId': 's',
              'event': incoming,
            },
          }),
        );
        await tester.pump(const Duration(milliseconds: 200));
        expect(c.transcript, same(snapshot));
        expect(tester.getRect(anchor), before);
        expect(c.window.needsRefresh, isTrue);
        final reads = api.historyReads;
        await c.loadHistory();
        expect(
          api.historyReads,
          reads,
          reason: 'Background snapshots must not reset wheel reading.',
        );
        list.controller!.jumpTo(0);
        await tester.pumpAndSettle();
        expect(c.holdingLiveHistory, isFalse);
        expect(find.text('新回复'), findsOneWidget);
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets('long histories build only rows near the finite viewport', (
    tester,
  ) async {
    final c = TestController()..selectedId = 's';
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    });
    c.transcript = [
      for (var i = 0; i < 1000; i++)
        TranscriptItem(id: 'row-$i', kind: 'assistant', text: '回复 $i\n\n下一段'),
    ];
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    expect(tester.widgetList(find.byType(MessageCard)).length, lessThan(30));
    final list = tester.widget<ListView>(
      find.byKey(const PageStorageKey('messages-s')),
    );
    expect(list.controller!.position.maxScrollExtent, greaterThan(10000));
    // Lazy rows refine the estimated extent after the first long jump.
    for (
      var attempt = 0;
      attempt < 3 && find.text('回复 0').evaluate().isEmpty;
      attempt++
    ) {
      list.controller!.jumpTo(list.controller!.position.maxScrollExtent);
      await tester.pumpAndSettle();
    }
    expect(find.text('回复 0'), findsOneWidget);
    expect(tester.widgetList(find.byType(MessageCard)).length, lessThan(30));
    expect(tester.takeException(), isNull);
  });

  testWidgets('growing from eight to nine rows keeps text and top alignment', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1280, 1000));
    final c = TestController()..selectedId = 's';
    c.transcript = [
      for (var i = 0; i < 8; i++)
        TranscriptItem(id: 'row-$i', kind: 'assistant', text: '回复 $i'),
    ];
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    });
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    final first = find.descendant(
      of: find.byKey(const ValueKey('row-0')).last,
      matching: find.byType(ProgressiveText),
    );
    final textState = tester.state(first);
    final top = tester.getTopLeft(first).dy;
    c.transcript = [
      ...c.transcript,
      TranscriptItem(id: 'row-8', kind: 'assistant', text: '回复 8'),
    ];
    c.messageChanges.value++;
    await tester.pumpAndSettle();
    expect(tester.getTopLeft(first).dy, closeTo(top, 1));
    expect(tester.state(first), same(textState));
    expect(find.text('回复 8'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
