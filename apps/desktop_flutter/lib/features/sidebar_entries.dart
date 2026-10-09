import 'package:flutter/material.dart';

import '../design/primitives.dart';
import '../design/motion.dart';

import 'package:dsh_desktop/design/typography.dart';

/// The Harness sidebar's raised, centered new-session action.
class NewSessionButton extends StatefulWidget {
  const NewSessionButton({
    super.key,
    this.buttonKey,
    required this.tooltip,
    this.shortcut,
    this.onPressed,
  });
  final Key? buttonKey;
  final String tooltip;
  final String? shortcut;
  final VoidCallback? onPressed;
  @override
  State<NewSessionButton> createState() => _NewSessionButtonState();
}

class _NewSessionButtonState extends State<NewSessionButton> {
  bool hovered = false, focused = false;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final height =
        38.0 +
        (MediaQuery.textScalerOf(context).scale(14) - 14).clamp(
          0.0,
          double.infinity,
        );
    return DshTooltip(
      message: widget.tooltip,
      child: Semantics(
        button: true,
        enabled: widget.onPressed != null,
        label: '新建会话',
        child: Material(
          color: colors.dark ? const Color(0xff43454a) : colors.base,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
            side: BorderSide(
              color: colors.dark
                  ? Colors.white.withValues(alpha: .16)
                  : Colors.black.withValues(alpha: .12),
              width: .5,
            ),
          ),
          child: InkWell(
            key: widget.buttonKey,
            onTap: widget.onPressed,
            onHover: (value) => setState(() => hovered = value),
            onFocusChange: (value) => setState(() => focused = value),
            borderRadius: BorderRadius.circular(12),
            hoverColor: colors.dark
                ? const Color(0xff353638)
                : const Color(0xfff1f3f5),
            child: Container(
              height: height,
              padding: const EdgeInsets.symmetric(horizontal: 16),
              child: LayoutBuilder(
                builder: (context, box) => Stack(
                  fit: StackFit.expand,
                  children: [
                    Center(
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          DshGlyph(
                            DshIcons.newSession.data,
                            size: 14,
                            color: colors.text,
                          ),
                          const SizedBox(width: 6),
                          Flexible(
                            child: Text(
                              '新会话',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: DshTypography.body.copyWith(
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                          ),
                        ],
                      ),
                    ),
                    if (widget.shortcut != null)
                      Positioned(
                        right: 0,
                        top: 0,
                        bottom: 0,
                        child: ExcludeSemantics(
                          child: IgnorePointer(
                            child: Center(
                              child: AnimatedOpacity(
                                duration: DshMotion.duration(
                                  context,
                                  DshMotion.quick,
                                ),
                                opacity:
                                    (hovered || focused) &&
                                        box.maxWidth >= 210 &&
                                        MediaQuery.textScalerOf(context)
                                                .scale(1) <
                                            1.3
                                    ? 1
                                    : 0,
                                child: SizedBox(
                                  width: 70,
                                  child: Text(
                                    widget.shortcut!,
                                    key: const ValueKey('sidebar-shortcut-new'),
                                    textAlign: TextAlign.right,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: DshTypography.caption.copyWith(
                                      color: colors.muted,
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

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
          child: DecoratedBox(
            decoration: BoxDecoration(
              color: colors.layer,
              borderRadius: BorderRadius.circular(
                DshTokens.of(context).radiusControl,
              ),
            ),
            child: DshButton(
              key: entry.key,
              outline: true,
              height: 36,
              width: double.infinity,
              active: entry.active,
              activeBackgroundColor: colors.selected,
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
