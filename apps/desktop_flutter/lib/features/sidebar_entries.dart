import 'package:flutter/material.dart';

import '../design/primitives.dart';

import 'package:dsh_desktop/design/typography.dart';

/// One global entry in the sidebar row (定时任务, 知识库, 插件, …).
class SidebarEntry {
  const SidebarEntry({
    this.key,
    required this.icon,
    required this.label,
    this.semanticLabel,
    required this.onPressed,
    this.active = false,
  });
  final Key? key;
  final IconData icon;
  final String label;
  final String? semanticLabel;
  String get fullLabel => semanticLabel ?? label;
  final VoidCallback? onPressed;
  final bool active;
}

/// Global entries sharing one row at equal widths. When the sidebar is too
/// narrow for their labels they collapse to icons with tooltips.
class SidebarEntryRow extends StatelessWidget {
  const SidebarEntryRow({super.key, required this.entries});
  final List<SidebarEntry> entries;

  /// Narrowest cell that still fits an icon and a two-character label.
  static const labelWidth = 76.0;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, box) {
      const gap = 6.0;
      if (entries.isEmpty) return const SizedBox.shrink();
      final scaler = MediaQuery.textScalerOf(context);
      final largeText = scaler.scale(1) >= 1.5;
      var requiredWidth = labelWidth;
      for (final entry in entries) {
        final painter = TextPainter(
          text: TextSpan(
            text: entry.label,
            style: DshTypography.body.copyWith(
              fontSize: DshTypography.sizeAuxiliary,
            ),
          ),
          textDirection: Directionality.of(context),
          textScaler: scaler,
          maxLines: 1,
        )..layout();
        final width = painter.width + 44;
        if (width > requiredWidth) requiredWidth = width;
        painter.dispose();
      }
      final columns = largeText
          ? (box.maxWidth >= requiredWidth * 2 + gap && entries.length > 1
                ? 2
                : 1)
          : entries.length;
      final cell = (box.maxWidth - gap * (columns - 1)) / columns;
      final showLabel = largeText || cell >= requiredWidth;
      final colors = DshColors(context);
      Widget button(SidebarEntry entry) => DshTooltip(
        message: entry.fullLabel,
        child: Semantics(
          label: entry.fullLabel,
          button: true,
          selected: entry.active,
          child: DshButton(
            key: entry.key,
            height: 36,
            width: double.infinity,
            outline: true,
            active: entry.active,
            activeBorderColor: colors.blue,
            padding: const EdgeInsets.symmetric(horizontal: 6),
            icon: entry.icon,
            onPressed: entry.onPressed,
            child: showLabel
                ? Flexible(
                    child: Text(
                      entry.label,
                      maxLines: largeText ? null : 1,
                      overflow: largeText
                          ? TextOverflow.visible
                          : TextOverflow.ellipsis,
                      style: const TextStyle(
                        fontSize: DshTypography.sizeAuxiliary,
                      ),
                    ),
                  )
                : const SizedBox.shrink(),
          ),
        ),
      );
      if (largeText) {
        return Wrap(
          spacing: gap,
          runSpacing: 8,
          children: [
            for (final entry in entries)
              SizedBox(width: cell, child: button(entry)),
          ],
        );
      }
      return Row(
        children: [
          for (final (index, entry) in entries.indexed) ...[
            if (index > 0) const SizedBox(width: gap),
            Expanded(child: button(entry)),
          ],
        ],
      );
    },
  );
}
