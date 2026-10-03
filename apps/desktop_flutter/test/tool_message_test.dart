import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/tool_message.dart';
import 'package:dsh_desktop/design/text_document.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

Future<String> displayedOutput(WidgetTester tester, TranscriptItem item) async {
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: SizedBox(
          width: 320,
          child: ToolMessage(key: ValueKey(item.id), item: item),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  await tester.tap(find.byKey(ValueKey('tool-row-${item.id}')));
  await tester.pump();
  final document = tester.widget<TextDocument>(find.byType(TextDocument));
  return document.sections.singleWhere((s) => s.title == '输出').text;
}

void main() {
  testWidgets('generated image details retain pretty JSON and both captions', (
    tester,
  ) async {
    final images = [
      for (final name in ['first.png', 'second.png'])
        {
          'type': 'image',
          'attachment': {
            'attachmentId': 'sha256:${name == 'first.png' ? 'a' * 64 : 'b' * 64}',
            'mediaType': 'image/png',
            'bytes': 80,
            'width': 1,
            'height': 1,
            'name': name,
          },
        },
    ];
    final value = {'images': images, 'edited': false};
    final raw = jsonEncode(value);
    final item = projectTranscript([
      HistoryEvent.fromJson(
        {
          'seq': 1,
          'type': 'tool/call',
          'data': {
            'turn': 1,
            'callId': 'generated',
            'name': 'generate_image',
            'arguments': '{}',
          },
        },
        view: {'view': {'title': '生成图片'}},
      ),
      HistoryEvent.fromJson({
        'seq': 2,
        'type': 'tool/result',
        'data': {
          'turn': 1,
          'meta': {...value, 'kind': 'image-generation'},
          'message': {
            'role': 'tool',
            'source': {'kind': 'tool', 'callId': 'generated'},
            'isError': false,
            'content': [
              {'type': 'text', 'text': raw},
              ...images,
            ],
          },
        },
      }),
    ]).single;
    const captions = '\n\n[图片：first.png]\n\n[图片：second.png]';
    expect(item.output, '$raw$captions');
    expect(item.images, isEmpty);
    expect(
      await displayedOutput(tester, item),
      '${const JsonEncoder.withIndent('  ').convert(value)}$captions',
    );
    expect(find.byType(Image), findsNothing);
    expect(tester.takeException(), isNull);
  });
  testWidgets('tool details preserve JSON, ordinary text and error tails', (
    tester,
  ) async {
    const cases = [
      (source: '{"ok":true}', expected: '{\n  "ok": true\n}'),
      (
        source: '{\n "ok": true,\n "count": 2\n}',
        expected: '{\n  "ok": true,\n  "count": 2\n}',
      ),
      (source: 'first line\nsecond line', expected: 'first line\nsecond line'),
      (
        source: '{"error":"failure"}\n\nKeep the exact error details',
        expected: '{"error":"failure"}\n\nKeep the exact error details',
      ),
      (
        source: 'ordinary text\n\n[图片：first.png]',
        expected: 'ordinary text\n\n[图片：first.png]',
      ),
    ];
    for (var index = 0; index < cases.length; index++) {
      final item = TranscriptItem(
        id: 'format-$index',
        kind: 'tool',
        title: '工具详情',
        text: '{}',
        output: cases[index].source,
        status: 'complete',
      );
      expect(await displayedOutput(tester, item), cases[index].expected);
      expect(tester.takeException(), isNull);
    }
  });
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
