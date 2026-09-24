import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/design/text_document.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class MemoryPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('profile native Markdown stream and bounded source details', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences())
      ..selectedId = 'render-fixture';
    c.sessions = [
      SessionSummary.fromJson({'sessionId': 'render-fixture', 'running': true}),
    ];
    final history = [
      for (var i = 0; i < 800; i++)
        TranscriptItem(
          id: 'history-$i',
          kind: i.isEven ? 'user' : 'assistant',
          text: i.isEven
              ? '分析第 $i 项结果'
              : '**已完成**\n\n- 保留既有布局\n- 检查文件\n\n编号：$i',
        ),
    ];
    c.transcript = history;
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    final sample =
        '# 分析结果\n\n**重点**与行内代码 `value`。\n\n'
        '- 保留第一项\n- 更新第二项\n\n'
        '```dart\nfinal value = 42;\nprint(value);\n```\n\n'
        '| 名称 | 状态 |\n| --- | --- |\n| 客户端 | 正常 |\n\n';
    final source = List.filled(24, sample).join();
    await binding.watchPerformance(() async {
      for (var i = 1; i <= 100; i++) {
        c.transcript = [
          ...history,
          TranscriptItem(
            id: 'stream-fixture',
            kind: 'assistant',
            text: source.substring(0, (source.length * i / 100).floor()),
            streaming: i < 100,
          ),
        ];
        c.messageChanges.value++;
        await tester.pump(const Duration(milliseconds: 72));
      }
      await tester.pumpAndSettle();
    }, reportKey: 'markdown_stream');
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    final large = '${'原始工具输出\n' * 250000}末尾标识';
    await binding.watchPerformance(() async {
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: TextDocument(sections: [(title: '工具结果', text: large)]),
          ),
        ),
      );
      await tester.pumpAndSettle();
      for (var i = 0; i < 8; i++) {
        await tester.drag(find.byType(ListView), const Offset(0, -480));
        await tester.pumpAndSettle();
      }
      expect(find.byType(SelectableText).evaluate().length, lessThan(8));
      expect(tester.takeException(), isNull);
    }, reportKey: 'source_details');
    final output = Platform.environment['DSH_PERF_OUTPUT'];
    if (output != null) {
      await File(output).writeAsString(
        jsonEncode({
          'fixture': '800 history rows; 100 stream updates at 72ms; code, headings, lists, tables; 2M code-unit details',
          'processId': pid,
          'reports': binding.reportData,
        }),
      );
    }
    await tester.pumpWidget(const SizedBox());
  });
}
