import 'dart:async';
import 'dart:math' as math;

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

class AnchorHistoryClient extends FakeClient {
  int count = 240;
  int? growAfter;
  Completer<HistoryPage>? pendingHistory;
  final requests = <({String session, int? after, int? before})>[];

  HistoryPage page(String id, {int? before, int? after}) {
    final total = id == 's' ? count : 3;
    final start = (after ?? math.max(0, (before ?? total) - 80)).clamp(
      0,
      total,
    );
    final end = math.min(total, after == null ? before ?? total : start + 80);
    return HistoryPage.fromJson({
      'events': [
        for (var seq = start; seq < end; seq++)
          {
            'event': {
              'seq': seq,
              'type': 'user/message',
              'data': {
                'source': {'kind': 'user'},
                'content': [
                  {
                    'type': 'text',
                    'text':
                        'Message $id/$seq\n${'Readable line.\n' * (growAfter != null && seq > growAfter! ? 12 : (seq % 4 + 1))}',
                  },
                ],
              },
            },
          },
      ],
      'firstSeq': start < end ? start : null,
      'lastSeq': start < end ? end - 1 : null,
      'hasMoreBefore': start > 0,
      'hasMoreAfter': end < total,
    });
  }

  @override
  Future<HistoryPage> history(
    String id, {
    int? before,
    int? after,
    RequestScope? scope,
  }) {
    requests.add((session: id, after: after, before: before));
    if (id == 's' && after != null && pendingHistory != null) {
      return pendingHistory!.future;
    }
    return Future.value(page(id, before: before, after: after));
  }
}

Future<DesktopController> connectedController(AnchorHistoryClient api) async {
  api.liveSessions = [
    SessionSummary.fromJson({'sessionId': 's'}),
    SessionSummary.fromJson({'sessionId': 'other'}),
  ];
  final controller = DesktopController(
    MemoryPreferences(),
    clientFactory: (_) => api,
  );
  await controller.connect('http://127.0.0.1');
  await controller.select('s');
  return controller;
}

class StreamingAnchorClient extends AnchorHistoryClient {
  bool completed = false;
  @override
  HistoryPage page(String id, {int? before, int? after}) {
    if (id != 's') return super.page(id, before: before, after: after);
    final text = 'A paragraph of explanation.\n\n' * (completed ? 180 : 100);
    return HistoryPage.fromJson({
      'events': [
        {
          'event': {
            'seq': 0,
            'type': 'user/message',
            'data': {
              'content': [
                {'type': 'text', 'text': 'Question'},
              ],
            },
          },
        },
        {
          'event': {
            'seq': completed ? 2 : 1,
            'type': completed ? 'assistant/message' : 'assistant/chunk',
            'data': {
              'turn': 'turn',
              'step': 1,
              if (completed)
                'message': {
                  'content': [
                    {'type': 'text', 'text': text},
                  ],
                }
              else
                'chunk': {'type': 'text-delta', 'index': 0, 'text': text},
            },
          },
        },
      ],
      'firstSeq': 0,
      'lastSeq': completed ? 2 : 1,
      'hasMoreBefore': false,
      'hasMoreAfter': false,
    });
  }
}

void main() {
  testWidgets(
    'a long streaming message keeps its anchor when it completes while away',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1200, 850));
      final api = StreamingAnchorClient();
      final c = await connectedController(api);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      tester
          .widget<ListView>(find.byKey(const PageStorageKey('messages-s')))
          .controller!
          .jumpTo(650);
      await tester.pumpAndSettle();
      final bookmark = c.readingPositions['s']!;
      expect(bookmark.itemId, startsWith('assistant:'));
      expect(bookmark.seq, 0);
      Finder row() => find.byWidgetPredicate(
        (widget) => widget is MessageCard && widget.item.id == bookmark.itemId,
      );
      final before = tester.getTopLeft(row()).dy;
      await c.select('other');
      await tester.pumpAndSettle();
      api.completed = true;
      await c.select('s');
      await tester.pumpAndSettle();
      expect(row(), findsOneWidget);
      expect(tester.getTopLeft(row()).dy, closeTo(before, 1));
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'multi-page reading restores its visible message after new and taller replies',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1200, 850));
      final api = AnchorHistoryClient();
      final controller = await connectedController(api);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: controller)),
        ),
      );
      await tester.pumpAndSettle();
      await controller.loadHistory(
        before: controller.window.firstSeq,
        merge: true,
      );
      await tester.pumpAndSettle();
      expect(controller.transcript.length, 160);
      final list = find.byKey(const PageStorageKey('messages-s'));
      tester.widget<ListView>(list).controller!.jumpTo(650);
      await tester.pumpAndSettle();
      final bookmark = controller.readingPositions['s']!;
      expect(bookmark.seq, greaterThan(controller.window.firstSeq!));
      Finder row() => find.byWidgetPredicate(
        (widget) => widget is MessageCard && widget.item.id == bookmark.itemId,
      );
      final before = tester.getTopLeft(row()).dy;
      await controller.select('other');
      await tester.pumpAndSettle();
      api.count += 60;
      api.growAfter = bookmark.seq;
      await controller.select('s');
      await tester.pumpAndSettle();
      expect(
        api.requests.lastWhere((request) => request.session == 's').after,
        bookmark.seq,
      );
      expect(row(), findsOneWidget);
      expect(tester.getTopLeft(row()).dy, closeTo(before, 1));
      expect(controller.readingHistory, isTrue);
      await controller.select('s');
      await tester.pumpAndSettle();
      expect(tester.getTopLeft(row()).dy, closeTo(before, 1));
      await tester.tap(find.byTooltip('回到底部'));
      await tester.pumpAndSettle();
      expect(controller.readingPositions.containsKey('s'), isFalse);
      await controller.select('other');
      await tester.pumpAndSettle();
      await controller.select('s');
      await tester.pumpAndSettle();
      expect(
        api.requests.lastWhere((request) => request.session == 's').after,
        isNull,
      );
      expect(controller.readingHistory, isFalse);
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
      await tester.binding.setSurfaceSize(null);
      expect(tester.takeException(), isNull);
    },
  );

  test('live events arriving during bookmark load do not enter its historical page', () async {
    final api = AnchorHistoryClient();
    final c = await connectedController(api);
    c.rememberReadingPosition(
      's',
      seq: 100,
      itemId: '100',
      viewportOffset: -20,
      follow: false,
    );
    api.pendingHistory = Completer<HistoryPage>();
    final loading = c.select('s');
    api.channels.first.data.add(
      HostFrame.fromJson({
        'type': 'server-request',
        'rpcId': 'live-fixture',
        'payload': {
          'type': 'session/event',
          'sessionId': 's',
          'event': {
            'seq': 300,
            'type': 'user/message',
            'data': {
              'content': [
                {'type': 'text', 'text': 'LIVE_NEW'},
              ],
            },
          },
        },
      }),
    );
    await Future<void>.delayed(Duration.zero);
    api.pendingHistory!.complete(api.page('s', after: 100));
    await loading;
    expect(c.readingHistory, isTrue);
    expect(c.transcript.any((item) => item.text == 'LIVE_NEW'), isFalse);
    expect(c.window.needsRefresh, isTrue);
    expect(c.unreadHistoryEvents, 1);
    c.dispose();
  });

  test('a removed bookmark falls back to current history without keeping a frozen empty page', () async {
    final api = AnchorHistoryClient();
    final c = await connectedController(api);
    c.rememberReadingPosition(
      's',
      seq: 200,
      itemId: '200',
      viewportOffset: -20,
      follow: false,
    );
    api.count = 10;
    await c.select('s');
    expect(c.readingPositions.containsKey('s'), isFalse);
    expect(c.transcript.length, 10);
    expect(c.readingHistory, isFalse);
    expect(api.requests.last.after, isNull);
    c.dispose();
  });
}
