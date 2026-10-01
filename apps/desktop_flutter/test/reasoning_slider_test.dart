import 'dart:async';

import 'package:dsh_desktop/features/conversation/reasoning_slider.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  test('standard levels use the Web names; custom names stay as declared', () {
    expect(reasoningLevelLabel({'id': 'high', 'name': 'High'}), '高');
    expect(reasoningLevelLabel({'id': 'xhigh', 'name': 'Extra high'}), '极高');
    expect(reasoningLevelLabel({'id': 'custom', 'name': 'Turbo'}), 'Turbo');
    expect(
      reasoningLevelLabel({'id': 'high', 'name': 'Deep think'}),
      'Deep think',
    );
  });

  testWidgets('labels commit on tap and a drag commits once on release', (
    tester,
  ) async {
    final commits = <String>[];
    var value = 'high';
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) => SizedBox(
              width: 360,
              child: ReasoningSlider(
                levels: const [
                  {'id': 'low', 'name': 'Low'},
                  {'id': 'medium', 'name': 'Medium'},
                  {'id': 'high', 'name': 'High'},
                  {'id': 'max', 'name': 'Max'},
                ],
                value: value,
                onChanged: (id) async {
                  commits.add(id);
                  setState(() => value = id);
                },
              ),
            ),
          ),
        ),
      ),
    );
    expect(find.text('推理强度'), findsOneWidget);
    expect(tester.widget<Slider>(find.byType(Slider)).value, 2);

    await tester.tap(find.byKey(const ValueKey('reasoning-level-low')));
    await tester.pump();
    expect(commits, ['low']);
    expect(tester.widget<Slider>(find.byType(Slider)).value, 0);

    final slider = find.byType(Slider);
    final start =
        tester.getCenter(slider) -
        Offset(tester.getSize(slider).width / 2 - 24, 0);
    final gesture = await tester.startGesture(start);
    await gesture.moveBy(Offset(tester.getSize(slider).width - 48, 0));
    await tester.pump();
    expect(commits, ['low'], reason: 'dragging alone does not commit');
    await gesture.up();
    await tester.pump();
    expect(commits, ['low', 'max']);

    await tester.tap(find.byKey(const ValueKey('reasoning-level-max')));
    await tester.pump();
    expect(commits, [
      'low',
      'max',
    ], reason: 'reselecting the current level is a no-op');
  });

  testWidgets('unknown and default efforts do not appear as the lowest level', (
    tester,
  ) async {
    Future<void> show(String? value) => tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SizedBox(
            width: 360,
            child: ReasoningSlider(
              levels: const [
                {'id': 'low', 'name': 'Low'},
                {'id': 'high', 'name': 'High'},
              ],
              value: value,
              onChanged: (_) async {},
            ),
          ),
        ),
      ),
    );
    await show(null);
    expect(find.text('模型默认'), findsOneWidget);
    expect(find.byType(Slider), findsNothing);
    await show('custom');
    expect(find.text('custom（未列出）'), findsOneWidget);
    expect(find.byType(Slider), findsNothing);
  });

  testWidgets(
    'pending effort retains the confirmed value and admits one request',
    (tester) async {
      final reply = Completer<void>();
      var calls = 0;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 360,
              child: ReasoningSlider(
                levels: const [
                  {'id': 'low', 'name': 'Low'},
                  {'id': 'high', 'name': 'High'},
                ],
                value: 'high',
                onChanged: (_) {
                  calls++;
                  return reply.future;
                },
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.byKey(const ValueKey('reasoning-level-low')));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('reasoning-level-low')));
      await tester.pump();
      expect(calls, 1);
      expect(tester.widget<Slider>(find.byType(Slider)).value, 1);
      expect(tester.widget<Slider>(find.byType(Slider)).onChanged, isNull);
      reply.completeError(StateError('rejected'));
      await tester.pumpAndSettle();
      expect(tester.widget<Slider>(find.byType(Slider)).value, 1);
      expect(
        find.byKey(const ValueKey('reasoning-update-error')),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    },
  );
}
