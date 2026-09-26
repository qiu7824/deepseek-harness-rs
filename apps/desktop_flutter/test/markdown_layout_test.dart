import 'package:dsh_desktop/design/rich_content.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  const samples = {
    'identifier':
        'long_inline_identifier_with_multiple_parts_and_more_characters_'
        'that_must_wrap_inside_a_narrow_conversation',
    'Chinese path': r'E:\工程资料\项目设计文件\施工组织方案\结构计算和图纸\现场照片与资料\验收说明.md',
  };
  for (final sample in samples.entries) {
    for (final scale in [1.0, 2.0]) {
      testWidgets(
        'inline ${sample.key} wraps and remains selectable at text scale $scale',
        (tester) async {
          final code = sample.value;
          final paragraph = '前文 $code 后文';
          await tester.pumpWidget(
            ShadApp(
              home: Scaffold(
                body: MediaQuery(
                  data: MediaQueryData(textScaler: TextScaler.linear(scale)),
                  child: SingleChildScrollView(
                    child: Align(
                      alignment: Alignment.topLeft,
                      child: SizedBox(
                        width: 180,
                        child: DshMarkdown(data: '前文 `$code` 后文\n\n后继段落'),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          );
          await tester.pumpAndSettle();
          expect(tester.takeException(), isNull);
          // Match selectable text by content without requiring an inline widget.
          // Selecting the whole paragraph must include its code.
          final editable = find.byWidgetPredicate(
            (widget) =>
                widget is EditableText && widget.controller.text.contains(code),
          );
          expect(editable, findsOneWidget);
          final text = tester.widget<EditableText>(editable).controller.text;
          expect(text, paragraph);
          final renderer = tester
              .state<EditableTextState>(editable)
              .renderEditable;
          expect(
            renderer.textScaler.scale(14),
            closeTo(14 * scale, .001),
            reason: 'The rendered text must use the requested system scaling.',
          );
          final start = text.indexOf(code);
          final boxes = renderer.getBoxesForSelection(
            TextSelection(baseOffset: start, extentOffset: start + code.length),
          );
          expect(boxes, isNotEmpty);
          final codeRects = [
            for (final box in boxes)
              Rect.fromPoints(
                renderer.localToGlobal(box.toRect().topLeft),
                renderer.localToGlobal(box.toRect().bottomRight),
              ),
          ];
          final codeRect = codeRects.reduce((a, b) => a.expandToInclude(b));
          final paragraphRect = tester.getRect(
            find.byType(DshMarkdownBlock).first,
          );
          final nextRect = tester.getRect(find.byType(DshMarkdownBlock).last);
          expect(
            boxes.map((box) => box.top).toSet().length,
            greaterThan(1),
            reason: 'The long inline text must wrap within the 180px column.',
          );
          expect(
            codeRect.top,
            greaterThanOrEqualTo(paragraphRect.top - .5),
            reason:
                'The first wrapped lines must not paint above the paragraph.',
          );
          expect(
            codeRect.bottom,
            lessThanOrEqualTo(paragraphRect.bottom + .5),
            reason: 'The paragraph must reserve the full inline code height.',
          );
          expect(
            codeRect.bottom,
            lessThanOrEqualTo(nextRect.top),
            reason: 'Wrapped code must not cover the following paragraph.',
          );
          for (final rect in codeRects) {
            expect(rect.left, greaterThanOrEqualTo(paragraphRect.left - .5));
            expect(rect.right, lessThanOrEqualTo(paragraphRect.right + .5));
          }
        },
      );
    }
  }

  testWidgets('cached Markdown follows text scaling changes in place', (
    tester,
  ) async {
    final scale = ValueNotifier(1.0);
    addTearDown(scale.dispose);
    const paragraph = '正文 inlineCode 后文';
    const markdown = DshMarkdown(data: '正文 `inlineCode` 后文\n\n后继段落');
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: ValueListenableBuilder<double>(
            valueListenable: scale,
            child: markdown,
            builder: (context, value, child) => MediaQuery(
              data: MediaQueryData(textScaler: TextScaler.linear(value)),
              child: SingleChildScrollView(
                child: Align(
                  alignment: Alignment.topLeft,
                  child: SizedBox(width: 180, child: child),
                ),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final markdownState = tester.state(find.byType(DshMarkdown));
    final editable = find.byWidgetPredicate(
      (widget) => widget is EditableText && widget.controller.text == paragraph,
    );
    expect(editable, findsOneWidget);
    expect(
      tester
          .state<EditableTextState>(editable)
          .renderEditable
          .textScaler
          .scale(14),
      closeTo(14, .001),
    );
    final initialHeight = tester
        .getSize(find.byType(DshMarkdownBlock).first)
        .height;

    scale.value = 2;
    await tester.pumpAndSettle();
    expect(tester.state(find.byType(DshMarkdown)), same(markdownState));
    expect(
      tester
          .state<EditableTextState>(editable)
          .renderEditable
          .textScaler
          .scale(14),
      closeTo(28, .001),
    );
    final paragraphRect = tester.getRect(find.byType(DshMarkdownBlock).first);
    expect(paragraphRect.height, greaterThan(initialHeight));
    expect(
      paragraphRect.bottom,
      lessThanOrEqualTo(tester.getRect(find.byType(DshMarkdownBlock).last).top),
    );
    expect(tester.takeException(), isNull);
  });
}
