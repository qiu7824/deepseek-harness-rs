import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';
import '../../l10n/workbench_zh.dart';

/// The workbench landing page contains only deliberate entry points. Merely
/// opening this page never reads workspace files or starts a process.
class WorkbenchStartPanel extends StatelessWidget {
  const WorkbenchStartPanel({
    super.key,
    required this.connected,
    required this.hasWorkspace,
    required this.onOpen,
    this.busy = false,
    this.onSelectWorkspace,
  });

  final bool connected, hasWorkspace, busy;
  final ValueChanged<String> onOpen;
  final VoidCallback? onSelectWorkspace;

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final enabled = connected && hasWorkspace && !busy;
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(24, 28, 24, 24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            DshWorkbenchZh.title,
            style: DshTypography.sectionTitle.copyWith(color: colors.text),
          ),
          const SizedBox(height: 8),
          Text(
            DshWorkbenchZh.hint,
            style: DshTypography.auxiliary.copyWith(color: colors.muted),
          ),
          const SizedBox(height: 24),
          for (final entry in [
            (
              'files',
              DshIcons.folder.data,
              DshWorkbenchZh.files,
              DshWorkbenchZh.filesHint,
            ),
            (
              'terminal',
              DshIcons.terminal.data,
              DshWorkbenchZh.newTerminal,
              DshWorkbenchZh.terminalHint,
            ),
            (
              'computer-use',
              DshIcons.browser.data,
              DshWorkbenchZh.browser,
              DshWorkbenchZh.browserHint,
            ),
          ])
            Padding(
              padding: const EdgeInsets.only(bottom: 12),
              child: _StartCard(
                key: ValueKey('workbench-start-${entry.$1}'),
                icon: entry.$2,
                title: entry.$3,
                description: entry.$4,
                onPressed: enabled ? () => onOpen(entry.$1) : null,
              ),
            ),
          if (!connected || !hasWorkspace) ...[
            const SizedBox(height: 8),
            Text(
              connected
                  ? DshWorkbenchZh.workspaceHint
                  : DshWorkbenchZh.connectionHint,
              style: DshTypography.auxiliary.copyWith(color: colors.muted),
            ),
            if (connected && onSelectWorkspace != null) ...[
              const SizedBox(height: 12),
              DshButton(
                key: const Key('workbench-select-workspace'),
                icon: DshIcons.folderPlus.data,
                onPressed: busy ? null : onSelectWorkspace,
                child: const Text(DshWorkbenchZh.chooseWorkspace),
              ),
            ],
          ],
        ],
      ),
    );
  }
}

class _StartCard extends StatelessWidget {
  const _StartCard({
    super.key,
    required this.icon,
    required this.title,
    required this.description,
    this.onPressed,
  });
  final IconData icon;
  final String title, description;
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Semantics(
      button: true,
      enabled: onPressed != null,
      child: Material(
        color: colors.base,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(12),
          side: BorderSide(color: colors.border.withValues(alpha: .6)),
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onPressed,
          borderRadius: BorderRadius.circular(12),
          hoverColor: colors.layer,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 18),
            child: Row(
              children: [
                DshGlyph(icon, size: 21, color: colors.text),
                const SizedBox(width: 16),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        title,
                        style: DshTypography.body.copyWith(
                          color: colors.text,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                      const SizedBox(height: 5),
                      Text(
                        description,
                        style: DshTypography.auxiliary.copyWith(
                          color: colors.muted,
                          height: 1.5,
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),
                DshGlyph(
                  DshIcons.chevronRight.data,
                  size: 15,
                  color: colors.muted,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
