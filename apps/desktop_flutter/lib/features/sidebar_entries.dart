import 'dart:math' as math;

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

/// Global entries sharing one row at equal widths. A label sits beside its
/// icon when there is room and under it when it is not. Large text that fits
/// neither wraps onto further rows; ordinary text keeps icons with tooltips.
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
      final style = DshTypography.body.copyWith(
        fontSize: DshTypography.sizeAuxiliary,
      );
      var widest = 0.0;
      var lineHeight = 0.0;
      for (final entry in entries) {
        final painter = TextPainter(
          text: TextSpan(text: entry.label, style: style),
          textDirection: Directionality.of(context),
          textScaler: scaler,
          maxLines: 1,
        )..layout();
        widest = math.max(widest, painter.width);
        lineHeight = math.max(lineHeight, painter.height);
        painter.dispose();
      }
      final cell = (box.maxWidth - gap * (entries.length - 1)) / entries.length;
      final beside = cell >= math.max(labelWidth, widest + 44);
      final stacked = !beside && cell >= widest + 16;
      final wrap = !beside && !stacked && scaler.scale(1) >= 1.5;
      final colors = DshColors(context);
      final tokens = DshTokens.of(context);
      final height = stacked
          ? math.max(tokens.controlHeight(context), lineHeight + 16 + 4 + 14)
          : 36.0;
      Text label(SidebarEntry entry) => Text(
        entry.label,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        textAlign: TextAlign.center,
        style: const TextStyle(fontSize: DshTypography.sizeAuxiliary),
      );
      Widget button(SidebarEntry entry) => DshTooltip(
        message: entry.fullLabel,
        child: Semantics(
          label: entry.fullLabel,
          button: true,
          selected: entry.active,
          // Borderless navigation tiles: hover gives the surface, the open
          // page reads in the accent colour.
          child: DshButton(
            key: entry.key,
            height: height,
            width: double.infinity,
            active: entry.active,
            activeBackgroundColor: colors.selected,
            padding: const EdgeInsets.symmetric(horizontal: 6),
            icon: stacked ? null : entry.icon,
            iconColor: entry.active ? colors.blue : colors.muted,
            textColor: entry.active ? colors.blue : null,
            onPressed: entry.onPressed,
            child: wrap
                ? Flexible(
                    child: Text(
                      entry.label,
                      style: const TextStyle(
                        fontSize: DshTypography.sizeAuxiliary,
                      ),
                    ),
                  )
                : stacked
                ? Flexible(
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        DshGlyph(
                          entry.icon,
                          size: 16,
                          color: entry.active ? colors.blue : colors.muted,
                        ),
                        const SizedBox(height: 4),
                        label(entry),
                      ],
                    ),
                  )
                : beside
                ? Flexible(child: label(entry))
                : const SizedBox.shrink(),
          ),
        ),
      );
      if (wrap) {
        final columns =
            box.maxWidth >= (widest + 44) * 2 + gap && entries.length > 1
            ? 2
            : 1;
        final width = (box.maxWidth - gap * (columns - 1)) / columns;
        return Wrap(
          spacing: gap,
          runSpacing: 8,
          children: [
            for (final entry in entries)
              SizedBox(width: width, child: button(entry)),
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
