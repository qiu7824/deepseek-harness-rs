import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/conversation/streaming_presentation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  testWidgets(
    'touch switch preserves its track and accepts taps in the larger target',
    (tester) async {
      var value = false;
      await tester.pumpWidget(
        ShadApp(
          home: Theme(
            data: ThemeData(
              extensions: [DshTokens.light.copyWith(touchMode: true)],
            ),
            child: Scaffold(
              body: Center(
                child: StatefulBuilder(
                  builder: (_, setState) => DshSwitch(
                    value: value,
                    onChanged: (next) => setState(() => value = next),
                  ),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.getSize(find.byType(DshSwitch)), const Size(48, 48));
      expect(tester.getSize(find.byType(ShadSwitch)), const Size(36, 22));
      await tester.tapAt(
        tester.getTopLeft(find.byType(DshSwitch)) + const Offset(2, 2),
      );
      await tester.pumpAndSettle();
      expect(value, isTrue);
    },
  );

  testWidgets(
    'accessible navigation shows the complete received text immediately',
    (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: MediaQuery(
            data: const MediaQueryData(accessibleNavigation: true),
            child: ProgressiveText(
              text: '所有已收到的正文立即呈现',
              streaming: true,
              builder: (text) => Text(text),
            ),
          ),
        ),
      );
      expect(find.text('所有已收到的正文立即呈现'), findsOneWidget);
      expect(tester.binding.transientCallbackCount, 0);
    },
  );

  testWidgets(
    'hidden and reduced-motion thinking indicators stop their ticker and mask',
    (tester) async {
      Widget build({bool visible = true, bool reduced = false}) => MaterialApp(
        home: MediaQuery(
          data: MediaQueryData(accessibleNavigation: reduced),
          child: TickerMode(
            enabled: visible,
            child: const ThinkingSweep(running: true, child: Text('正在执行')),
          ),
        ),
      );
      final mask = find.descendant(
        of: find.byType(ThinkingSweep),
        matching: find.byType(CustomPaint),
      );
      await tester.pumpWidget(build());
      expect(mask, findsOneWidget);
      expect(tester.binding.transientCallbackCount, greaterThan(0));
      await tester.pumpWidget(build(visible: false));
      await tester.pump();
      expect(mask, findsNothing);
      expect(tester.binding.transientCallbackCount, 0);
      await tester.pumpWidget(build());
      expect(mask, findsOneWidget);
      await tester.pumpWidget(build(reduced: true));
      await tester.pump();
      expect(mask, findsNothing);
      expect(tester.binding.transientCallbackCount, 0);
      await tester.pumpWidget(const SizedBox());
    },
  );
}
