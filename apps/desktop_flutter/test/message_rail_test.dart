import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/message_rail.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

HistoryPage railPage(int seq, {int replies = 1, bool after = false}) =>
    HistoryPage.fromJson({
      'events': [
        {
          'event': {
            'seq': seq,
            'type': 'user/message',
            'data': {
              'source': {'kind': 'user'},
              'content': [
                {'type': 'text', 'text': '定位消息 $seq'},
              ],
            },
          },
        },
        for (var i = 1; i <= replies; i++)
          {
            'event': {
              'seq': seq + i,
              'type': 'assistant/message',
              'data': {
                'message': {
                  'content': [
                    {'type': 'text', 'text': '回复 $i\n${'保留上下文。' * 50}'},
                  ],
                },
              },
            },
          },
      ],
      'firstSeq': seq,
      'lastSeq': seq + replies,
      'hasMoreBefore': seq > 0,
      'hasMoreAfter': after,
    });

class RailApi extends FakeClient {
  final requests = <({String session, int? after, RequestScope? scope})>[];
  final pending = <int, Completer<HistoryPage>>{};
  @override
  Future<HistoryPage> history(
    String id, {
    int? before,
    int? after,
    RequestScope? scope,
  }) {
    requests.add((session: id, after: after, scope: scope));
    return after == null
        ? Future.value(railPage(900))
        : pending[after]?.future ??
              Future.value(railPage(after, replies: 22, after: true));
  }
}

void main() {
  testWidgets(
    'scrolling completed output reveals return-to-latest without a server event',
    (tester) async {
      final c = DesktopController(MemoryPreferences())..selectedId = 's';
      c.transcript = List.generate(
        30,
        (i) => TranscriptItem(
          id: 'reply-$i',
          kind: 'assistant',
          text: '正文 $i\n${'内容。' * 100}',
        ),
      );
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byTooltip('回到底部'), findsNothing);
      final list = find.byType(ListView).first;
      await tester.drag(list, const Offset(0, 350));
      await tester.pumpAndSettle();
      expect(find.byTooltip('回到底部'), findsOneWidget);
      expect(
        tester.getSize(
          find.descendant(
            of: find.byTooltip('回到底部'),
            matching: find.byType(InkWell),
          ),
        ),
        const Size(34, 34),
      );
      final scrollable = tester.state<ScrollableState>(
        find.descendant(of: list, matching: find.byType(Scrollable)).first,
      );
      scrollable.position.jumpTo(0);
      await tester.pumpAndSettle();
      expect(find.byTooltip('回到底部'), findsNothing);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );

  testWidgets(
    'rail follows reading position without stealing a keyboard page',
    (tester) async {
      final focus = FocusNode(), current = ValueNotifier<int?>(999);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 450,
              height: 300,
              child: UserMessageRail(
                entries: List.generate(
                  1000,
                  (i) => MessageRailEntry(i, '消息 $i', 0),
                ),
                focusNode: focus,
                current: current,
                onActivate: (_) async {},
              ),
            ),
          ),
        ),
      );
      current.value = 100;
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('message-rail-100')), findsOneWidget);
      expect(find.byKey(const ValueKey('message-rail-999')), findsNothing);
      focus.requestFocus();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.home);
      await tester.pumpAndSettle();
      current.value = 500;
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('message-rail-0')), findsOneWidget);
      expect(find.byKey(const ValueKey('message-rail-500')), findsNothing);
      await tester.pumpWidget(const SizedBox());
      current.value = 10;
      focus.dispose();
      current.dispose();
      expect(tester.takeException(), isNull);
    },
  );

  test(
    'return latest supersedes an unfinished jump and resumes live appends',
    () async {
      final api = RailApi();
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      await c.select('s');
      api.pending[10] = Completer();
      final pending = c.loadHistory(after: 10, targetSeq: 10, force: true);
      final abandoned = api.requests.last.scope!;
      await c.returnLatest();
      expect(abandoned.cancelled, isTrue);
      api.pending[10]!.complete(railPage(10, after: true));
      await pending;
      expect(c.window.firstSeq, 900);
      expect(c.readingHistory, isFalse);
      api.channels.first.data.add(
        HostFrame.fromJson({
          'type': 'server-request',
          'rpcId': 'fresh',
          'payload': {
            'type': 'session/event',
            'sessionId': 's',
            'event': {
              'seq': 902,
              'type': 'user/message',
              'data': {
                'source': {'kind': 'user'},
                'content': [
                  {'type': 'text', 'text': '新的消息'},
                ],
              },
            },
          },
        }),
      );
      await Future<void>.delayed(const Duration(milliseconds: 100));
      expect(c.window.lastSeq, 902);
      expect(c.transcript.last.text, '新的消息');
      expect(c.window.needsRefresh, isFalse);
      c.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );
  test('index contains only real users and projection budgets still apply', () {
    final projection = ProjectionWindow();
    projection.apply('userMessageRail', [
      {'seq': 10, 'text': '索引摘要', 'images': 2},
    ], 3);
    final entries = messageRailEntries(projection.values['userMessageRail'], [
      TranscriptItem(id: 'system', kind: 'context', text: '不属于用户', seq: 5),
      TranscriptItem(id: 'user', kind: 'user', text: '完整正文', seq: 10),
      TranscriptItem(id: 'new', kind: 'user', text: '', seq: 20, images: [{}]),
    ]);
    expect(entries.map((e) => e.seq), [10, 20]);
    expect(entries.first.text, '索引摘要');
    expect(entries.last.label, '（仅图片）');
    expect(
      messageRailEntries([
        {'seq': -1},
        {'seq': '12'},
        {'seq': 10},
      ], []).length,
      1,
    );
    projection.apply('userMessageRail', [
      {'seq': 1, 'text': 'a' * (2 * 1024 * 1024)},
    ], 4);
    expect(projection.oversized, contains('userMessageRail'));
    expect(projection.retainedBytes, lessThanOrEqualTo(projection.maxBytes));
    expect(railSnippet('字' * 500).runes.length, lessThanOrEqualTo(201));
  });

  test('newest navigation wins, stale errors and live events cannot replace a reading window', () async {
    final api = RailApi();
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    await c.select('s');
    api.pending[10] = Completer();
    api.pending[20] = Completer();
    final first = c.loadHistory(after: 10, targetSeq: 10, force: true);
    final oldScope = api.requests.last.scope!;
    final second = c.loadHistory(after: 20, targetSeq: 20, force: true);
    expect(oldScope.cancelled, isTrue);
    api.pending[20]!.complete(railPage(20, after: true));
    await second;
    api.pending[10]!.completeError(StateError('stale read failed'));
    await first;
    expect(c.historyTargetSeq, 20);
    expect(c.window.firstSeq, 20);
    final retained = c.window.retainedBytes, count = c.window.eventCount;
    for (var i = 1000; i < 1200; i++) {
      api.channels.first.data.add(
        HostFrame.fromJson({
          'type': 'server-request',
          'rpcId': '$i',
          'payload': {
            'type': 'session/event',
            'sessionId': 's',
            'event': {
              'seq': i,
              'type': 'assistant/chunk',
              'data': {
                'chunk': {'type': 'text-delta', 'text': 'live'},
              },
            },
          },
        }),
      );
    }
    await Future<void>.delayed(const Duration(milliseconds: 100));
    expect(c.window.retainedBytes, retained);
    expect(c.window.eventCount, count);
    expect(c.window.needsRefresh, isTrue);
    await c.returnLatest();
    expect(c.historyTargetSeq, isNull);
    expect(c.window.firstSeq, 900);
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });

  test(
    'bad targets keep existing data, session switch discards pending jump',
    () async {
      final api = RailApi();
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      await c.select('s');
      api.pending[10] = Completer()..complete(railPage(11));
      await expectLater(
        c.loadHistory(after: 10, targetSeq: 10, force: true),
        throwsStateError,
      );
      expect(c.window.firstSeq, 900);
      expect(c.historyTargetSeq, isNull);
      api.pending[30] = Completer();
      final jump = c.loadHistory(after: 30, targetSeq: 30, force: true);
      await c.select('other');
      api.pending[30]!.complete(railPage(30));
      await jump;
      expect(c.selectedId, 'other');
      expect(c.window.firstSeq, 900);
      expect(c.historyTargetSeq, isNull);
      c.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );

  testWidgets(
    'rail limits ticks to viewport and keyboard reaches older pages',
    (tester) async {
      final focus = FocusNode(), current = ValueNotifier<int?>(999);
      int? selected;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 450,
              height: 300,
              child: UserMessageRail(
                entries: List.generate(
                  1000,
                  (i) => MessageRailEntry(i, '消息 $i', 0),
                ),
                focusNode: focus,
                current: current,
                onActivate: (entry) async => selected = entry.seq,
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('message-rail-999')), findsOneWidget);
      expect(find.byKey(const ValueKey('message-rail-0')), findsNothing);
      focus.requestFocus();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.home);
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('message-rail-0')), findsOneWidget);
      expect(find.byKey(const ValueKey('message-rail-999')), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(selected, 0);
      await tester.sendKeyEvent(LogicalKeyboardKey.pageDown);
      await tester.pumpAndSettle();
      expect(
        find.byKey(
          ValueKey('message-rail-${UserMessageRail.capacity(300) - 1}'),
        ),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      focus.dispose();
      current.dispose();
    },
  );

  testWidgets(
    'indexed jump loads one page and positions an initially unmounted message',
    (tester) async {
      final api = RailApi();
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await tester.pump();
      await c.select('s');
      c.projectionWindow.apply('userMessageRail', [
        {'seq': 10, 'text': '较早消息'},
        {'seq': 900, 'text': '当前消息'},
      ], 900);
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      final cachedIndex = tester
          .widget<UserMessageRail>(find.byType(UserMessageRail))
          .entries;
      c.messageChanges.value++;
      await tester.pumpAndSettle();
      expect(
        tester.widget<UserMessageRail>(find.byType(UserMessageRail)).entries,
        same(cachedIndex),
      );
      final area = tester.getRect(find.byType(Conversation));
      final oldTick = tester.getCenter(
        find.byKey(const ValueKey('message-rail-10')),
      );
      final newTick = tester.getCenter(
        find.byKey(const ValueKey('message-rail-900')),
      );
      expect(newTick.dy - oldTick.dy, closeTo(10, .1));
      expect(
        (oldTick.dy + newTick.dy) / 2,
        closeTo(area.top + (area.height + 76) / 2, .1),
      );
      final line = find.descendant(
        of: find.byKey(const ValueKey('message-rail-10')),
        matching: find.byType(AnimatedContainer),
      );
      expect(tester.getSize(line), const Size(12, 2));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: oldTick);
      await mouse.moveTo(oldTick);
      await tester.pumpAndSettle();
      expect(tester.getSize(line), const Size(26, 2));
      await mouse.removePointer();
      await tester.pumpAndSettle();
      final count = api.requests.length;
      await tester.tap(find.byKey(const ValueKey('message-rail-10')));
      await tester.pumpAndSettle();
      expect(api.requests.length, count + 1);
      expect(api.requests.last.after, 10);
      expect(c.historyTargetSeq, 10);
      expect(find.text('定位消息 10').hitTestable(), findsOneWidget);
      expect(find.byTooltip('回到底部'), findsOneWidget);
      await tester.tap(find.byTooltip('回到底部'));
      await tester.pumpAndSettle();
      expect(c.historyTargetSeq, isNull);
      expect(find.text('定位消息 900').hitTestable(), findsOneWidget);
      expect(find.text('较早消息'), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.pump();
    },
  );
}
