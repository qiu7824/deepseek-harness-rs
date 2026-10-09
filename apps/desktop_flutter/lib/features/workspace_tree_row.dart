import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../design/primitives.dart';
import '../design/motion.dart';

import 'package:dsh_desktop/design/typography.dart';

class WorkspaceTreeRow extends StatelessWidget {
  const WorkspaceTreeRow({
    super.key,
    required this.title,
    required this.path,
    required this.expanded,
    required this.active,
    required this.onPressed,
    this.onToggle,
    this.toggleKey,
    required this.onMenu,
  });
  final String title, path;
  final bool expanded, active;
  final VoidCallback onPressed;
  final VoidCallback? onToggle;
  final Key? toggleKey;
  final ValueChanged<Offset> onMenu;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Tooltip(
      message: displayPath(path),
      child: InkWell(
        onTap: onPressed,
        onSecondaryTapDown: (event) => onMenu(event.globalPosition),
        borderRadius: BorderRadius.circular(8),
        child: SizedBox(
          height: 34,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            child: Row(
              children: [
                Tooltip(
                  message: '${expanded ? '收起' : '展开'}工作区：$title',
                  child: InkWell(
                    key: toggleKey,
                    onTap: onToggle ?? onPressed,
                    borderRadius: BorderRadius.circular(4),
                    child: SizedBox(
                      width: 24,
                      height: 30,
                      child: Center(
                        child: AnimatedRotation(
                          turns: expanded ? .25 : 0,
                          duration: DshMotion.duration(
                            context,
                            DshMotion.quick,
                          ),
                          curve: DshMotion.curve,
                          child: DshGlyph(
                            DshIcons.chevronRight.data,
                            size: 12,
                            color: colors.muted,
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
                DshGlyph(
                  DshIcons.folder.data,
                  size: 16,
                  color: active ? colors.blue : colors.muted,
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      fontSize: DshTypography.sizeBody,
                      height: 20 / 14,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
