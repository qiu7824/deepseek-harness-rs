import 'package:dsh_desktop/features/workbench/line_index.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'line indexing retains split semantics without preallocating line strings',
    () {
      for (final source in ['', 'a', 'a\n', '\n\n', 'a\r\nb', '😀\n中文']) {
        final expected = source.split('\n');
        final index = LineIndex.fromText(source);
        expect(index.length, expected.length);
        for (var i = 0; i < expected.length; i++) {
          expect(index[i], expected[i]);
        }
      }
      expect(LineIndex.empty().isEmpty, isTrue);
    },
  );

  test('a long source keeps only four bytes of index per line', () {
    final source = 'a\n' * 100000;
    final index = LineIndex.fromText(source);
    expect(index.length, 100001);
    expect(index.retainedBytes, 100001 * 4);
    expect(index[99999], 'a');
    expect(index[100000], isEmpty);
    expect(() => index[100001], throwsRangeError);
  });
}
