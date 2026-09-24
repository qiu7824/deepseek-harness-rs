import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/design/rich_content.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

void main() {
  testWidgets(
    'active composer matches measured Web card geometry and user bubble bounds',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1144, 862));
      final c = DesktopController(MemoryPreferences())..selectedId = 's';
      c.transcript = [
        TranscriptItem(id: 'user', kind: 'user', text: '请检查项目并保留原始格式。' * 50),
        TranscriptItem(id: 'answer', kind: 'assistant', text: '正文'),
      ];
      c.projectionWindow.apply('sessionStats', {'turns': 1, 'steps': 1}, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      final card = tester.getSize(find.byKey(const ValueKey('composer-card')));
      expect(card.height, closeTo(94, 1));
      expect(card.width, closeTo(1144 * .64 + 32, 1));
      expect(find.byType(ComposerAction), findsNWidgets(3));
      expect(
        tester.getSize(find.byType(ComposerAction).first),
        const Size(34, 34),
      );
      expect(
        tester.getSize(find.byKey(const ValueKey('bubble-user'))).width,
        lessThanOrEqualTo(525),
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets(
    'user prompts remain literal and streaming text keeps a left anchor',
    (tester) async {
      Future<void> show(TranscriptItem item) => tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Align(
              alignment: Alignment.topLeft,
              child: SizedBox(width: 700, child: MessageCard(item: item)),
            ),
          ),
        ),
      );
      await show(
        TranscriptItem(id: 'u', kind: 'user', text: '# 原样显示\n**不是回复排版**'),
      );
      await tester.pumpAndSettle();
      expect(find.byType(DshMarkdown), findsNothing);
      expect(find.text('# 原样显示\n**不是回复排版**'), findsOneWidget);
      await show(
        TranscriptItem(
          id: 'a',
          kind: 'assistant',
          text: '流式正文',
          streaming: true,
        ),
      );
      await tester.pumpAndSettle();
      final left = tester.getTopLeft(find.byType(DshMarkdown)).dx;
      await show(
        TranscriptItem(
          id: 'a',
          kind: 'assistant',
          text: '流式正文',
          streaming: false,
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.getTopLeft(find.byType(DshMarkdown)).dx, left);
    },
  );
  testWidgets(
    'reasoning stays collapsed during streaming and long expansion is paged',
    (tester) async {
      final text = '第一行摘要\n${'思考内容。' * 10000}';
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: MessageCard(
                item: TranscriptItem(
                  id: 'r',
                  kind: 'reasoning',
                  text: text,
                  streaming: true,
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pump(const Duration(milliseconds: 100));
      expect(find.byType(SelectableText), findsNothing);
      await tester.tap(find.text('思考'));
      await tester.pump(const Duration(milliseconds: 100));
      final shown = tester.widget<SelectableText>(find.byType(SelectableText));
      expect(shown.data!.length, lessThanOrEqualTo(16001));
      expect(find.text('下一段'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );
}
