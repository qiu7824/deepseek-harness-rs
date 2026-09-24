import 'package:flutter/material.dart';

import 'typography.dart';

/// A source document is retained once. Only viewport slices become layout
/// objects, including when a tool returns a single multi-megabyte line.
class TextDocument extends StatelessWidget {
  const TextDocument({super.key, required this.sections});
  final List<({String title, String text})> sections;
  static const sliceLength = 4096;

  static int boundary(String text, int offset) {
    final at = offset.clamp(0, text.length);
    if (at > 0 &&
        at < text.length &&
        text.codeUnitAt(at) >= 0xdc00 &&
        text.codeUnitAt(at) <= 0xdfff &&
        text.codeUnitAt(at - 1) >= 0xd800 &&
        text.codeUnitAt(at - 1) <= 0xdbff) {
      return at - 1;
    }
    return at;
  }

  @override
  Widget build(BuildContext context) {
    final counts = [
      for (final s in sections)
        1 + (s.text.length + sliceLength - 1) ~/ sliceLength,
    ];
    return Scrollbar(
      child: ListView.builder(
        padding: const EdgeInsets.all(20),
        itemCount: counts.fold<int>(0, (sum, value) => sum + value),
        itemBuilder: (context, index) {
          var section = 0, row = index;
          while (row >= counts[section]) {
            row -= counts[section++];
          }
          final s = sections[section];
          if (row == 0) {
            return s.title.isEmpty
                ? const SizedBox.shrink()
                : Padding(
                    padding: const EdgeInsets.symmetric(vertical: 8),
                    child: Text(s.title),
                  );
          }
          final start = boundary(s.text, (row - 1) * sliceLength);
          final end = boundary(s.text, row * sliceLength);
          return SelectableText(
            s.text.substring(start, end),
            key: ValueKey('document-$section-$row'),
            style: DshTypography.code.copyWith(height: 1.6),
          );
        },
      ),
    );
  }
}
