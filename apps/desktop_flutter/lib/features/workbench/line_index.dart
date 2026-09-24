import 'dart:typed_data';

/// Addresses source lines without allocating a String for every line.
class LineIndex {
  LineIndex._(this.source, this.starts);

  factory LineIndex.empty() => LineIndex._('', Uint32List(0));

  factory LineIndex.fromText(String source) {
    var count = 1;
    for (var i = 0; i < source.length; i++) {
      if (source.codeUnitAt(i) == 10) count++;
    }
    final starts = Uint32List(count);
    var line = 1;
    for (var i = 0; i < source.length; i++) {
      if (source.codeUnitAt(i) == 10) starts[line++] = i + 1;
    }
    return LineIndex._(source, starts);
  }

  final String source;
  final Uint32List starts;
  int get length => starts.length;
  bool get isEmpty => starts.isEmpty;
  bool get isNotEmpty => starts.isNotEmpty;
  int get retainedBytes => starts.length * Uint32List.bytesPerElement;

  String operator [](int index) {
    RangeError.checkValidIndex(index, starts, 'index', starts.length);
    final start = starts[index];
    final end = index + 1 < starts.length
        ? starts[index + 1] - 1
        : source.length;
    return source.substring(start, end);
  }
}
