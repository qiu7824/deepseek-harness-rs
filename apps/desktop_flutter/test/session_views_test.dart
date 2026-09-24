import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:dsh_desktop/features/conversation/session_views.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show MemoryPreferences, historyPage;

void main() {
  testWidgets('queue arrival preserves the expanded progress dock', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences())..selectedId = 's';
    c.sessions = [
      SessionSummary.fromJson({'sessionId': 's'}),
    ];
    c.transcript = [TranscriptItem(id: '1', kind: 'user', text: '检查状态')];
    c.projectionWindow.apply('todos', [
      {'content': '保持展开的任务', 'status': 'pending'},
    ], 1);
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('todo-disclosure')));
    await tester.pumpAndSettle();
    expect(find.text('保持展开的任务'), findsOneWidget);
    c.queued = [
      {
        'id': 'queue-test',
        'message': {
          'content': [
            {'type': 'text', 'text': '待发送'},
          ],
        },
      },
    ];
    c.emit();
    await tester.pumpAndSettle();
    expect(find.text('保持展开的任务'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });
  testWidgets(
    'narrow chat pane keeps long model, stop, context and send controls bounded',
    (tester) async {
      final c = DesktopController(MemoryPreferences())..selectedId = 's';
      c.sessions = [
        SessionSummary.fromJson({'sessionId': 's', 'running': true}),
      ];
      c.catalog = ModelCatalog.fromJson({
        'current': {
          'provider': 'p',
          'model': 'very-long-model-route-abcdefghijklmnopqrstuvwxyz',
          'reasoningEffort': 'high',
        },
        'groups': [],
      });
      c.transcript = [TranscriptItem(id: '1', kind: 'user', text: '检查控件布局')];
      c.projectionWindow.apply('contextPressure', {
        'projectedTokens': 50,
        'contextWindow': 100,
      }, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(
              child: SizedBox(
                width: 360,
                height: 650,
                child: Conversation(controller: c),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('send-message')), findsOneWidget);
      expect(find.byTooltip('上下文已用 50%'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
  testWidgets(
    'session tabs expose real context and trace, composer focus returns to conversation',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1280, 900));
      final c = DesktopController(MemoryPreferences())..selectedId = 'session';
      c.window.replace(historyPage('实际消息'));
      c.transcript = c.window.project();
      c.projectionWindow.apply('contextInsights', {
        'userMessages': 12,
        'assistantMessages': 18,
        'roleTokens': {'user': 30, 'assistant': 70},
        'reasoningTokens': null,
      }, 1);
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      await tester.tap(find.text('上下文'));
      await tester.pumpAndSettle();
      expect(find.byType(ContextView), findsOneWidget);
      expect(find.byKey(const Key('prompt-input')), findsOneWidget);
      expect(find.text('30'), findsOneWidget);
      expect(find.text('上下文细分'), findsOneWidget);
      c.composerFocus.value++;
      await tester.pumpAndSettle();
      expect(find.byType(ContextView), findsNothing);
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('prompt-input')))
            .focusNode!
            .hasFocus,
        isTrue,
      );
      await tester.tap(find.text('轨迹'));
      await tester.pumpAndSettle();
      expect(find.text('实际消息'), findsOneWidget);
      expect(find.byKey(const ValueKey('trajectory-timeline')), findsOneWidget);
      expect(find.byKey(const Key('prompt-input')), findsOneWidget);
      c.newConversation();
      await tester.pumpAndSettle();
      expect(find.byType(TraceView), findsNothing);
      expect(find.text('轨迹'), findsNothing);
      expect(find.byTooltip('显示工作台'), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );
}
