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
}
