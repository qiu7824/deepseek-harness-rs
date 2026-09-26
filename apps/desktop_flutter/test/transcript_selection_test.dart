import 'package:dsh_desktop/design/rich_content.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  testWidgets(
    'one drag selects across paragraphs, lists and code, and copies',
    (tester) async {
      String? copied;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied = (call.arguments as Map)['text'] as String?;
          }
          return null;
        },
      );
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 600,
              child: DshMarkdown(
                data: '第一段说明。\n\n- 要点甲\n- 要点乙\n\n```\nprint("代码")\n```\n\n最后一段结论。',
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(SelectionArea), findsOneWidget);
      expect(find.byType(SelectableText), findsNothing);

      final start =
          tester.getTopLeft(find.textContaining('第一段说明')) + const Offset(1, 6);
      final end =
          tester.getBottomRight(find.textContaining('最后一段结论')) -
          const Offset(1, 6);
      final gesture = await tester.startGesture(
        start,
        kind: PointerDeviceKind.mouse,
      );
      await tester.pump();
      await gesture.moveTo(end);
      await tester.pump();
      await gesture.up();
      await tester.pumpAndSettle();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tester.pump();

      expect(copied, isNotNull);
      for (final part in ['第一段说明', '要点甲', '要点乙', 'print("代码")', '最后一段结论']) {
        expect(copied, contains(part));
      }
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('markdown inside an existing selection area joins it', (
    tester,
  ) async {
    await tester.pumpWidget(
      const ShadApp(
        home: Scaffold(
          body: SelectionArea(
            child: Column(
              children: [
                Text('用户消息'),
                DshMarkdown(data: '助手回复'),
              ],
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(SelectionArea), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });
}
