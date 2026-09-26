import 'package:dsh_desktop/design/text_document.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

List<String> documentSlices(String text) => [
  for (var start = 0; start < text.length; start += TextDocument.sliceLength)
    text.substring(
      TextDocument.boundary(text, start),
      TextDocument.boundary(text, start + TextDocument.sliceLength),
    ),
];

void main() {
  final cases = <String, String>{
    'joined emoji': '👩‍💻',
    'emoji with skin tone': '👩🏽‍💻',
    'combining accent': 'e\u0301',
    'carriage return and line feed': '\r\n',
    'regional indicator pairs': '🇨🇳🇺🇸',
  };

  for (final entry in cases.entries) {
    test('${entry.key} keeps every character whole at a slice boundary', () {
      // Place each interior UTF-16 offset at the nominal slice boundary.
      for (var interior = 1; interior < entry.value.length; interior++) {
        final source =
            '${'a' * (TextDocument.sliceLength - interior)}'
            '${entry.value}${'b' * TextDocument.sliceLength}结束';
        final slices = documentSlices(source);
        expect(slices.join(), source);
        expect(
          slices.expand((slice) => slice.characters),
          orderedEquals(source.characters),
          reason: '${entry.key}, interior offset $interior',
        );
      }
    });
  }

  test('boundary keeps existing character boundaries and clamps offsets', () {
    const source = 'A👩‍💻e\u0301\r\n🇨🇳🇺🇸Z';
    var start = 0;
    for (final character in source.characters) {
      for (var inside = 0; inside < character.length; inside++) {
        expect(TextDocument.boundary(source, start + inside), start);
      }
      start += character.length;
    }
    expect(TextDocument.boundary(source, -1), 0);
    expect(TextDocument.boundary(source, source.length), source.length);
    expect(TextDocument.boundary(source, source.length + 1), source.length);
    expect(TextDocument.boundary('', 20), 0);
  });

  test('a character larger than a slice is retained exactly once', () {
    final character = 'e${'\u0301' * (TextDocument.sliceLength * 2)}';
    final source = '${'a' * 20}$character结束';
    final slices = documentSlices(source);
    expect(slices.join(), source);
    expect(slices.expand((slice) => slice.characters), source.characters);
    expect(slices.where((slice) => slice.contains(character)), hasLength(1));
  });

  testWidgets('large documents still build only viewport slices', (
    tester,
  ) async {
    final source = '${'资料行👩‍💻e\u0301🇨🇳\r\n' * 150000}最终记录';
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: TextDocument(sections: [(title: '结果', text: source)]),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final visible = tester
        .widgetList<SelectableText>(find.byType(SelectableText))
        .toList();
    expect(visible, isNotEmpty);
    expect(visible, hasLength(lessThan(8)));
    for (final text in visible) {
      expect(
        text.data!.length,
        lessThanOrEqualTo(TextDocument.sliceLength + 8),
      );
    }
    expect(
      visible.map((text) => text.data).join().length,
      lessThan(source.length),
    );
    expect(tester.takeException(), isNull);
  });
}
