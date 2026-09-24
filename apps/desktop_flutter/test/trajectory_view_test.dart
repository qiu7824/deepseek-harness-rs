import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/trajectory_view.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences, FakeClient;

void main() {
  testWidgets(
    'loading older history preserves the first visible operation after async completion',
    (tester) async {
      final api = FakeClient(),
          c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
      await c.connect('http://127.0.0.1');
      await tester.pump();
      HistoryPage page(int first, int count) => HistoryPage.fromJson({
        'firstSeq': first,
        'lastSeq': first + count - 1,
        'hasMoreBefore': true,
        'events': List.generate(
          count,
          (i) => {
            'event': {
              'seq': first + i,
              'type': 'user/message',
              'time': first + i,
              'data': {
                'content': [
                  {'type': 'text', 'text': '锚点 ${first + i}'},
                ],
              },
            },
          },
        ),
      });
      c.selectedId = 's';
      c.window.replace(page(100, 100));
      final pending = Completer<HistoryPage>();
      api.histories['s'] = pending.future;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ListenableBuilder(
              listenable: c,
              builder: (context, _) => TraceView(controller: c),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final timeline = tester.getRect(
        find.byKey(const ValueKey('trajectory-timeline')),
      );
      await tester.tapAt(Offset(timeline.left + .01, timeline.center.dy));
      await tester.pumpAndSettle();
      final top = tester.getTopLeft(find.text('锚点 100')).dy;
      await tester.tap(find.text('加载更早记录'));
      await tester.pump();
      expect(c.loading, isTrue);
      pending.complete(page(50, 50));
      await tester.pumpAndSettle();
      expect(tester.getTopLeft(find.text('锚点 100')).dy, top);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.pump();
    },
  );
  testWidgets('live timeline clock stops when the view is disposed', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences())..selectedId = 's';
    c.preferences.layout['traceActualDuration'] = true;
    c.sessions = [
      SessionSummary.fromJson({'sessionId': 's', 'running': true}),
    ];
    c.window.replace(
      HistoryPage.fromJson({
        'events': [
          {
            'event': {
              'seq': 1,
              'time': 100,
              'type': 'step/start',
              'data': {'turn': 1, 'step': 1},
            },
          },
        ],
      }),
    );
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: TraceView(controller: c)),
      ),
    );
    await tester.pump(const Duration(seconds: 2));
    expect(find.text('运行中'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await tester.pump(const Duration(seconds: 2));
    expect(tester.takeException(), isNull);
  });
  testWidgets(
    'timeline focuses an unmounted operation, folds turns and searches content',
    (tester) async {
      final c = DesktopController(MemoryPreferences());
      c.window.replace(
        HistoryPage.fromJson({
          'events': List.generate(
            2000,
            (i) => {
              'event': {
                'seq': i,
                'type': 'user/message',
                'time': i * 100,
                'data': {
                  'turn': i + 1,
                  'source': {'kind': 'user'},
                  'content': [
                    {'type': 'text', 'text': '事件内容 $i'},
                  ],
                },
              },
            },
          ),
        }),
      );
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 360,
              height: 500,
              child: TraceView(controller: c),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('事件内容 0'), findsNothing);
      final timeline = find.byKey(const ValueKey('trajectory-timeline'));
      final rect = tester.getRect(timeline);
      await tester.tapAt(Offset(rect.left + .01, rect.center.dy));
      await tester.pumpAndSettle();
      expect(find.text('事件内容 0'), findsOneWidget);
      expect(find.byType(Text).evaluate().length, lessThan(200));
      await tester.tap(find.byTooltip('收起轮次'));
      await tester.pumpAndSettle();
      expect(find.text('轮次已收起 · 1 项'), findsWidgets);
      await tester.enterText(find.byType(TextField), '事件内容 1900');
      await tester.pumpAndSettle();
      expect(find.text('第 1901 轮'), findsOneWidget);
      await tester.tap(find.text('轮次已收起 · 1 项'));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byWidgetPredicate((w) => w is Text && w.data == '事件内容 1900'),
      );
      await tester.pumpAndSettle();
      expect(find.text('事件详情 · 用户'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
  testWidgets(
    'actual duration selection is persisted and timeline supports range selection',
    (tester) async {
      final prefs = MemoryPreferences(),
          c = DesktopController(MemoryPreferences());
      final controller = DesktopController(prefs);
      controller.window.replace(
        HistoryPage.fromJson({
          'events': List.generate(
            3,
            (i) => {
              'event': {
                'seq': i,
                'type': 'user/message',
                'time': i * 100,
                'data': {
                  'content': [
                    {'type': 'text', 'text': '消息 $i'},
                  ],
                },
              },
            },
          ),
        }),
      );
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: TraceView(controller: controller)),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('按实际耗时显示'));
      await tester.pumpAndSettle();
      expect(prefs.layout['traceActualDuration'], true);
      await tester.drag(
        find.byKey(const ValueKey('trajectory-timeline')),
        const Offset(100, 0),
      );
      await tester.pumpAndSettle();
      expect(
        tester.widget<TraceTimeline>(find.byType(TraceTimeline)).range,
        isNotNull,
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
      c.dispose();
    },
  );
}
