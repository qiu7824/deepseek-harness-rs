import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../design/primitives.dart';
import '../l10n/zh.dart';
import 'row_menu.dart';

import 'package:dsh_desktop/design/typography.dart';

class WorkspaceTreeRow extends StatefulWidget {
  const WorkspaceTreeRow({
    super.key,
    required this.title,
    required this.path,
    required this.expanded,
    required this.active,
    required this.onPressed,
    this.onToggle,
    this.toggleKey,
    this.menuKey,
    required this.onMenu,
  });
  final String title, path;
  final bool expanded, active;
  final VoidCallback onPressed;
  final VoidCallback? onToggle;
  final Key? toggleKey, menuKey;
  final ValueChanged<Offset> onMenu;
  @override
  State<WorkspaceTreeRow> createState() => _WorkspaceTreeRowState();
}

class _WorkspaceTreeRowState extends State<WorkspaceTreeRow> {
  bool hovered = false, focused = false;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return MouseRegion(
      onEnter: (_) => setState(() => hovered = true),
      onExit: (_) => setState(() => hovered = false),
      child: Focus(
        canRequestFocus: false,
        skipTraversal: true,
        onFocusChange: (value) => setState(() => focused = value),
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
                    Tooltip(
                      message:
                          '${widget.expanded ? '收起' : '展开'}工作区：${widget.title}',
                      child: InkWell(
                        key: widget.toggleKey,
                        onTap: widget.onToggle ?? widget.onPressed,
                        borderRadius: BorderRadius.circular(4),
                        child: SizedBox(
                          width: 24,
                          height: 30,
                          child: Center(
                            child: DshGlyph(
                              hovered
                                  ? (widget.expanded
                                        ? DshIcons.chevronDown.data
                                        : DshIcons.chevronRight.data)
                                  : DshIcons.folder.data,
                              size: 16,
                              color: !hovered && widget.active
                                  ? colors.blue
                                  : colors.muted,
                            ),
                          ),
                        ),
                      ),
                    ),
                    const SizedBox(width: 6),
                    Expanded(
                      child: Text(
                        widget.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          fontSize: DshTypography.sizeBody,
                          height: 20 / 14,
                        ),
                      ),
                    ),
                    if (hovered || focused)
                      RowMoreButton(
                        key: widget.menuKey,
                        label: DshShellZh.workspaceActions,
                        onMenu: widget.onMenu,
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
