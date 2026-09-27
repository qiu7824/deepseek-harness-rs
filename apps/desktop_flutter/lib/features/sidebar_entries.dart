import 'package:flutter/material.dart';

import '../design/primitives.dart';

/// One global entry in the sidebar row (定时任务, 知识库, 插件, …).
class SidebarEntry {
  const SidebarEntry({
    this.key,
    required this.icon,
    required this.label,
    required this.onPressed,
    this.active = false,
  });
  final Key? key;
  final IconData icon;
  final String label;
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
      final cell = (box.maxWidth - gap * (entries.length - 1)) / entries.length;
      final showLabel = cell >= labelWidth;
      final colors = DshColors(context);
      return Row(
        children: [
          for (final (index, entry) in entries.indexed) ...[
            if (index > 0) const SizedBox(width: gap),
            Expanded(
              child: Tooltip(
                message: showLabel ? '' : entry.label,
                child: Semantics(
                  label: entry.label,
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
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: const TextStyle(fontSize: 13),
                            ),
                          )
                        : const SizedBox.shrink(),
                  ),
                ),
              ),
            ),
          ],
        ],
      );
    },
  );
}
