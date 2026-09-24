import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/rich_content.dart';
import 'package:dsh_desktop/design/text_document.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter_math_fork/flutter_math.dart';
import 'package:flutter_mermaid/flutter_mermaid.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

void main() {
  testWidgets('assistant output uses Web typography and heading margins', (
    tester,
  ) async {
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: MessageCard(
              item: TranscriptItem(
                id: 'reply',
                kind: 'assistant',
                text:
                    '正文\n\n# 一级标题\n\n## 二级标题\n\n### 三级标题\n\n#### 四级标题\n\n- 列表',
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final blocks = tester
        .widgetList<DshMarkdownBlock>(find.byType(DshMarkdownBlock))
        .toList();
    final style = blocks.first.style;
    expect(style.p!.fontSize, 14);
    expect(style.p!.height! * style.p!.fontSize!, 24);
    expect(style.h1!.fontSize, 21);
    expect(style.h2!.fontSize, 19);
    expect(style.h3!.fontSize, 18);
    expect(style.h4!.fontSize, 14);
    expect(style.h2!.fontWeight, FontWeight.w700);
    expect(conversationBlockSpacing(blocks[0].node, blocks[1].node), 32);
    expect(conversationBlockSpacing(blocks[4].node, blocks[5].node), 8);
    expect(tester.takeException(), isNull);
  });

  testWidgets('math and Mermaid remain native blocks after renderer reuse', (
    tester,
  ) async {
    const source =
        r'公式 $x^2$'
        '\n\n'
        r'$$'
        '\ny=x+1\n'
        r'$$'
        '\n\n```mermaid\ngraph LR\nA-->B\n```';
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: SizedBox(width: 700, child: DshMarkdown(data: source)),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(Math), findsNWidgets(2));
    expect(find.byType(MermaidDiagram), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
  testWidgets(
    'streamed tail reuses completed blocks and late reference definitions update links',
    (tester) async {
      var text = '```dart\nfinal stable = true;\n```\n\n[说明][doc]\n\n末尾';
      late StateSetter change;
      String? opened;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: StatefulBuilder(
              builder: (context, setState) {
                change = setState;
                return DshMarkdown(
                  data: text,
                  onTapLink: (_, url, _) => opened = url,
                );
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final code = tester.state(find.byType(NativeCodeBlock));
      final rendered = tester.widget(
        find
            .descendant(
              of: find.byType(DshMarkdownBlock).first,
              matching: find.byType(Column),
            )
            .first,
      );
      change(() => text += '正在追加内容\n\n[doc]: https://example.org/document');
      await tester.pumpAndSettle();
      expect(tester.state(find.byType(NativeCodeBlock)), same(code));
      expect(
        tester.widget(
          find
              .descendant(
                of: find.byType(DshMarkdownBlock).first,
                matching: find.byType(Column),
              )
              .first,
        ),
        same(rendered),
      );
      Iterable<TextSpan> spans(InlineSpan span) sync* {
        if (span is TextSpan) {
          yield span;
          for (final child in span.children ?? <InlineSpan>[]) {
            yield* spans(child);
          }
        }
      }

      final link = tester
          .widgetList<SelectableText>(find.byType(SelectableText))
          .expand((w) => spans(w.textSpan ?? TextSpan(text: w.data)))
          .where((s) => s.recognizer is TapGestureRecognizer)
          .first;
      (link.recognizer as TapGestureRecognizer).onTap!();
      expect(opened, 'https://example.org/document');
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets('Markdown formatting and renderer survive streaming completion', (
    tester,
  ) async {
    const text =
        '# 标题\n\n**重点** 和 [链接](https://example.org)\n\n'
        '- 第一项\n- 第二项\n\n'
        '```dart\nfinal value = 1;\n```\n\n'
        '| 列 A | 列 B |\n| --- | --- |\n| 甲 | 乙 |';
    Future<void> show(bool streaming) => tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: SizedBox(
              width: 700,
              child: MessageCard(
                item: TranscriptItem(
                  id: 'reply',
                  kind: 'assistant',
                  text: text,
                  streaming: streaming,
                ),
              ),
            ),
          ),
        ),
      ),
    );
    await show(true);
    await tester.pumpAndSettle();
    expect(find.byType(NativeCodeBlock), findsOneWidget);
    expect(find.byType(Table), findsWidgets);
    final body = tester.state(find.byType(DshMarkdownBlock).first);
    final rect = tester.getRect(find.byType(DshMarkdown));
    final code = tester.state(find.byType(NativeCodeBlock));
    await show(false);
    await tester.pumpAndSettle();
    expect(tester.state(find.byType(DshMarkdownBlock).first), same(body));
    expect(tester.state(find.byType(NativeCodeBlock)), same(code));
    expect(tester.getRect(find.byType(DshMarkdown)), rect);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'unchanged Markdown is reused and links use the latest callback',
    (tester) async {
      late StateSetter rebuild;
      var version = 0;
      final calls = <int>[];
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: StatefulBuilder(
              builder: (context, setState) {
                rebuild = setState;
                final current = version;
                return DshMarkdown(
                  data: '[链接](https://example.org)',
                  onTapLink: (_, _, _) => calls.add(current),
                );
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final body = tester.widget(find.byType(DshMarkdownBlock).first);
      for (var i = 0; i < 25; i++) {
        rebuild(() => version++);
        await tester.pump();
        expect(tester.widget(find.byType(DshMarkdownBlock).first), same(body));
      }
      // Callback is read from the current parent even when the body is cached.
      tester
          .widget<DshMarkdownBlock>(find.byType(DshMarkdownBlock).first)
          .onTapLink('链接', 'https://example.org', '');
      expect(calls, [25]);
    },
  );

  testWidgets(
    'new transcript rows preserve expanded reasoning and Markdown state',
    (tester) async {
      final c = DesktopController(MemoryPreferences())..selectedId = 's';
      c.transcript = [
        TranscriptItem(id: 'reason', kind: 'reasoning', text: '摘要\n保留展开的思考'),
        TranscriptItem(
          id: 'reply',
          kind: 'assistant',
          text: '**回复**',
          streaming: true,
        ),
      ];
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('思考'));
      await tester.pumpAndSettle();
      final state = tester.state(find.byType(ReasoningMessage));
      final markdown = tester.state(find.byType(DshMarkdownBlock).first);
      c.transcript = [
        ...c.transcript,
        TranscriptItem(id: 'tool', kind: 'tool', title: '读取文件', text: '{}'),
      ];
      c.messageChanges.value++;
      await tester.pumpAndSettle();
      expect(tester.state(find.byType(ReasoningMessage)), same(state));
      expect(tester.state(find.byType(DshMarkdownBlock).first), same(markdown));
      expect(find.text('摘要\n保留展开的思考'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );

  testWidgets(
    'multi-megabyte detail lays out bounded slices and can reach the final text',
    (tester) async {
      final long = '${'资料行\n' * 400000}最终记录';
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: TextDocument(sections: [(title: '结果', text: long)]),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(SelectableText).evaluate().length, lessThan(5));
      for (final e in find.byType(SelectableText).evaluate()) {
        expect(
          (e.widget as SelectableText).data!.length,
          lessThanOrEqualTo(TextDocument.sliceLength + 1),
        );
      }
      final scroll = tester.state<ScrollableState>(
        find.byType(Scrollable).first,
      );
      scroll.position.jumpTo(scroll.position.maxScrollExtent);
      await tester.pumpAndSettle();
      // Extent estimates converge after the last virtual child is measured.
      for (
        var i = 0;
        i < 3 && find.textContaining('最终记录').evaluate().isEmpty;
        i++
      ) {
        scroll.position.jumpTo(scroll.position.maxScrollExtent);
        await tester.pumpAndSettle();
      }
      expect(find.textContaining('最终记录'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  test(
    'detail boundaries preserve emoji without gaps or lone UTF-16 surrogates',
    () {
      final text = '${'a' * 4095}😀${'中' * 4095}😀结束';
      final chunks = <String>[];
      for (
        var start = 0;
        start < text.length;
        start += TextDocument.sliceLength
      ) {
        final a = TextDocument.boundary(text, start),
            b = TextDocument.boundary(text, start + TextDocument.sliceLength);
        chunks.add(text.substring(a, b));
      }
      expect(chunks.join(), text);
      expect(chunks.first.length, 4095);
      expect(chunks[1].startsWith('😀'), isTrue);
    },
  );
}
