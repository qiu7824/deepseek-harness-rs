import 'package:dsh_desktop/features/conversation/turn_stats.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  testWidgets('closed turn time opens measured duration and speed details', (
    tester,
  ) async {
    await tester.pumpWidget(
      const ShadApp(
        home: Scaffold(
          body: Center(
            child: TurnTimeButton(
              runMs: 383601,
              ttftMs: 6673,
              tokensPerSecond: 43.70049,
            ),
          ),
        ),
      ),
    );
    expect(find.text('用时 6分23秒'), findsOneWidget);
    await tester.tap(find.text('用时 6分23秒'));
    await tester.pumpAndSettle();
    expect(find.text('本轮用时和速度'), findsOneWidget);
    expect(find.text('44 tok/s'), findsOneWidget);
    expect(find.text('6.7秒'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('本轮用时和速度'), findsNothing);
    await tester.pumpWidget(const SizedBox());
    expect(tester.takeException(), isNull);
  });

  testWidgets('exact token panel reports disjoint input and output buckets', (
    tester,
  ) async {
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: Center(
            child: TurnUsageButton(
              usage: {
                'uncachedInputTokens': 1000,
                'cacheReadTokens': 2000,
                'cacheWriteTokens': 300,
                'outputTokens': 100,
                'reasoningTokens': 25,
                'totalTokens': 3400,
                'routes': [
                  {'provider': 'p', 'model': 'm'},
                ],
              },
            ),
          ),
        ),
      ),
    );
    expect(find.text('用量 3.4K'), findsOneWidget);
    await tester.tap(find.text('用量 3.4K'));
    await tester.pumpAndSettle();
    expect(find.text('本轮用量'), findsOneWidget);
    expect(find.text('1,000'), findsOneWidget);
    expect(find.text('2,000'), findsOneWidget);
    expect(find.text('300'), findsOneWidget);
    expect(find.text('100（其中推理 25）'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    expect(tester.takeException(), isNull);
  });
}
