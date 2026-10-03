import 'package:flutter/material.dart';

import '../design/primitives.dart';
import '../design/typography.dart';

/// Opens a sidebar row menu with the same surface as the header menus.
Future<T?> showRowMenu<T>(
  BuildContext context,
  Offset position,
  List<PopupMenuEntry<T>> items,
) {
  final colors = DshColors(context);
  return showMenu<T>(
    context: context,
    position: RelativeRect.fromLTRB(
      position.dx,
      position.dy,
      position.dx,
      position.dy,
    ),
    color: colors.base,
    elevation: 4,
    shadowColor: Colors.black.withValues(alpha: .1),
    shape: RoundedRectangleBorder(
      borderRadius: BorderRadius.circular(12),
      side: BorderSide(color: colors.border.withValues(alpha: .6)),
    ),
    constraints: const BoxConstraints(minWidth: 200, maxWidth: 300),
    items: items,
  );
}

/// Icon and label for a sidebar row menu item; destructive items read red.
Widget rowMenuLabel(
  BuildContext context,
  IconData icon,
  String title, {
  bool destructive = false,
}) {
  final colors = DshColors(context);
  final danger = DshTokens.of(context).error.foreground;
  return Row(
    children: [
      DshGlyph(icon, size: 16, color: destructive ? danger : colors.muted),
      const SizedBox(width: 10),
      Expanded(
        child: Text(
          title,
          style: DshTypography.body.copyWith(
            color: destructive ? danger : colors.text,
          ),
        ),
      ),
    ],
  );
}

/// Tracks whether the pointer or keyboard focus is inside a row, so its
/// "more" button appears where a right-click menu would otherwise hide.
class HoverRowActions extends StatefulWidget {
  const HoverRowActions({super.key, required this.builder});
  final Widget Function(BuildContext context, bool revealed) builder;
  @override
  State<HoverRowActions> createState() => _HoverRowActionsState();
}

class _HoverRowActionsState extends State<HoverRowActions> {
  bool hovered = false, focused = false;
  @override
  Widget build(BuildContext context) => MouseRegion(
    onEnter: (_) => setState(() => hovered = true),
    onExit: (_) => setState(() => hovered = false),
    child: Focus(
      canRequestFocus: false,
      skipTraversal: true,
      onFocusChange: (value) => setState(() => focused = value),
      child: widget.builder(context, hovered || focused),
    ),
  );
}

/// A compact "more" button that opens its row's menu below itself.
class RowMoreButton extends StatelessWidget {
  const RowMoreButton({super.key, required this.label, required this.onMenu});
  final String label;
  final ValueChanged<Offset> onMenu;
  @override
  Widget build(BuildContext context) => DshIcon(
    DshIcons.ellipsis.data,
    label: label,
    size: 24,
    glyphSize: 14,
    onPressed: () {
      final box = context.findRenderObject() as RenderBox?;
      if (box == null || !box.attached) return;
      onMenu(box.localToGlobal(Offset(0, box.size.height)));
    },
  );
}
