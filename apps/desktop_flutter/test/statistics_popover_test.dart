import 'package:dsh_desktop/design/statistics_popover.dart';
import 'package:dsh_desktop/features/conversation/session_status.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

void main() {
  testWidgets(
    'Escape closes the current dialog before the underlying statistics',
    (tester) async {
      await tester.pumpWidget(
        const ShadApp(
          home: Scaffold(
            body: Center(
              child: StatisticsPopover(
                label: '打开统计',
                title: '底层统计',
                rows: [('模型用时', '1秒')],
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('打开统计'));
      await tester.pumpAndSettle();
      final context = tester.element(find.byType(Scaffold));
      showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (dialogContext) => CallbackShortcuts(
          bindings: {
            const SingleActivator(LogicalKeyboardKey.escape): () =>
                Navigator.pop(dialogContext),
          },
          child: const Focus(
            autofocus: true,
            child: AlertDialog(title: Text('当前设置窗口')),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(find.text('当前设置窗口'), findsNothing);
      expect(find.text('底层统计'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(find.text('底层统计'), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets(
    'session statistics use distinct timing usage and context popovers',
    (tester) async {
      final c = DesktopController(MemoryPreferences())..selectedId = 'first';
      c.projectionWindow.apply('sessionStats', {
        'turns': 3,
        'steps': 9,
        'llmMs': 10000,
        'toolMs': 2000,
        'ttftMs': 4000,
        'ttftSteps': 2,
        'requestMs': 2000,
        'requestSamples': 1,
        'requestOutputTokens': 200,
        'decodeMs': 1,
        'decodeTokens': 9999,
      }, 1);
      c.projectionWindow.apply('tokenUsage', {
        'uncachedInputTokens': 1000,
        'cacheReadTokens': 80,
        'cacheWriteTokens': 20,
        'outputTokens': 200,
        'cacheStatistics': {
          'reportedSamples': 1,
          'unreportedSamples': 2,
          'reportedInputTokens': 100,
        },
      }, 1);
      c.projectionWindow.apply('contextPressure', {
        'projectedTokens': 2500,
        'contextWindow': 10000,
      }, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.bottomCenter,
              child: SessionStatsLine(controller: c),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(StatisticsPopover), findsNWidgets(3));
      await tester.tap(find.byKey(const ValueKey('session-statistics-button')));
      await tester.pumpAndSettle();
      expect(find.text('100 tok/s'), findsOneWidget);
      expect(find.text('2秒'), findsNWidgets(2));
      expect(find.textContaining('9999'), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(find.text('模型用时'), findsNothing);
      await tester.tap(find.byKey(const ValueKey('session-usage-button')));
      await tester.pumpAndSettle();
      expect(find.text('80%（部分请求）'), findsOneWidget);
      expect(find.text('1300 tok'), findsOneWidget);
      c.selectedId = 'second';
      c.emit();
      await tester.pumpAndSettle();
      expect(find.text('会话用量'), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );

  testWidgets(
    'missing measurements are unavailable while an explicit zero is retained',
    (tester) async {
      final c = DesktopController(MemoryPreferences());
      c.projectionWindow.apply('sessionStats', {'turns': 0, 'steps': 0}, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: SessionStatsLine(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(StatisticsPopover), findsNWidgets(3));
      await tester.tap(find.byKey(const ValueKey('session-statistics-button')));
      await tester.pumpAndSettle();
      expect(find.text('0'), findsNWidgets(2));
      expect(find.text('暂不可用'), findsNWidgets(4));
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('session-usage-button')));
      await tester.pumpAndSettle();
      expect(find.text('0 tok'), findsNothing);
      expect(find.text('暂不可用'), findsWidgets);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );

  testWidgets(
    'statistics detail fits a narrow viewport and restores trigger focus on Escape',
    (tester) async {
      tester.view.physicalSize = const Size(360, 520);
      tester.view.devicePixelRatio = 1;
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.bottomCenter,
              child: StatisticsPopover(
                label: '用量',
                title: '本轮用量',
                rows: [
                  for (var i = 0; i < 12; i++)
                    (
                      '很长的统计名称 $i',
                      '123456789 tok / provider-with-long-name/model-name',
                    ),
                ],
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('用量'));
      await tester.pumpAndSettle();
      expect(find.text('本轮用量'), findsOneWidget);
      expect(find.byType(SingleChildScrollView), findsWidgets);
      expect(tester.takeException(), isNull);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(find.text('本轮用量'), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(find.text('本轮用量'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );
}
