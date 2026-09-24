import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/tool_message.dart';
import 'package:dsh_desktop/design/text_document.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  testWidgets('file link opens exact path without toggling tool details', (
    tester,
  ) async {
    String? opened;
    final item = TranscriptItem(
      id: 't',
      kind: 'tool',
      title: '已读取文件',
      text: '{}',
      output: 'body',
      filePath: r'\\?\E:\Project\a.txt',
      summary: r'\\?\E:\Project\a.txt',
      iconKind: 'read',
      status: 'complete',
    );
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SizedBox(
            width: 300,
            child: ToolMessage(
              item: item,
              cwd: r'E:\Project',
              onOpenPath: (p) => opened = p,
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('a.txt'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('tool-file-t')));
    await tester.pump();
    expect(opened, item.filePath);
    expect(find.byType(TextDocument), findsNothing);
    await tester.tap(find.text('已读取文件'));
    await tester.pump();
    expect(find.byType(TextDocument), findsOneWidget);
    expect(find.text('body'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
  testWidgets(
    'expanded tool retains state across completion and virtualizes long output',
    (tester) async {
      Future<void> show(String output, String status) => tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 320,
              child: ToolMessage(
                key: const ValueKey('stable'),
                item: TranscriptItem(
                  id: 'q',
                  kind: 'tool',
                  title: '提问',
                  summary: '等待回答',
                  text: '{"questions":[{"id":"a"}]}',
                  output: output,
                  status: status,
                  iconKind: 'question',
                ),
              ),
            ),
          ),
        ),
      );
      await show('', 'pending');
      await tester.pumpAndSettle();
      await tester.tap(find.text('提问'));
      await tester.pump();
      expect(find.text('运行中…'), findsOneWidget);
      await show('x' * 2000000, 'complete');
      await tester.pump();
      expect(find.byKey(const ValueKey('tool-body-q')), findsOneWidget);
      expect(find.byType(SelectableText).evaluate().length, lessThan(8));
      await tester.tap(find.text('提问'));
      await tester.pump();
      expect(find.byType(TextDocument), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );
}
