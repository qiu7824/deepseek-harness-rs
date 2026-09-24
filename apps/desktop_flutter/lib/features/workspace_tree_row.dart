import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../design/primitives.dart';

class WorkspaceTreeRow extends StatefulWidget {
  const WorkspaceTreeRow({
    super.key,
    required this.title,
    required this.path,
    required this.expanded,
    required this.active,
    required this.onPressed,
    required this.onMenu,
  });
  final String title, path;
  final bool expanded, active;
  final VoidCallback onPressed;
  final ValueChanged<Offset> onMenu;
  @override
  State<WorkspaceTreeRow> createState() => _WorkspaceTreeRowState();
}

class _WorkspaceTreeRowState extends State<WorkspaceTreeRow> {
  bool hovered = false;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return MouseRegion(
      onEnter: (_) => setState(() => hovered = true),
      onExit: (_) => setState(() => hovered = false),
      child: Tooltip(
        message: displayPath(widget.path),
        child: InkWell(
          onTap: widget.onPressed,
          onSecondaryTapDown: (event) => widget.onMenu(event.globalPosition),
          borderRadius: BorderRadius.circular(8),
          child: SizedBox(
            height: 34,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: Row(
                children: [
                  DshGlyph(
                    hovered
                        ? (widget.expanded
                              ? LucideIcons.chevronDown
                              : LucideIcons.chevronRight)
                        : LucideIcons.folder,
                    size: 16,
                    color: !hovered && widget.active
                        ? colors.blue
                        : colors.muted,
                  ),
                  const SizedBox(width: 6),
                  Expanded(
                    child: Text(
                      widget.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: 14, height: 20 / 14),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
