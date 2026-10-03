import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';
import '../../l10n/plugin_settings_zh.dart';

/// A workspace inventory row; enablement and runtime status remain independent.
class PluginInventoryRow extends StatelessWidget {
  const PluginInventoryRow({
    super.key,
    required this.entry,
    required this.onEnabledChanged,
    this.onConfigure,
    this.onOpen,
    this.expanded = false,
  });

  final Json entry;
  final ValueChanged<bool>? onEnabledChanged;
  final VoidCallback? onConfigure, onOpen;
  final bool expanded;

  DshIcons iconFor(String name) => switch (name) {
    'dsh-auto-review' || 'dsh-experimental-auto-review' => DshIcons.shieldCheck,
    'dsh-time-context' => DshIcons.clock,
    'dsh-schedule' => DshIcons.calendarClock,
    'dsh-artifacts' => DshIcons.files,
    'dsh-context-jump' => DshIcons.listChecks,
    'dsh-better-sidebar' => DshIcons.panelRight,
    'dsh-sidebar-workbench-suite' => DshIcons.layoutGrid,
    'dsh-voice-input' => DshIcons.mic,
    _ => DshIcons.puzzle,
  };

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final moduleName =
        '${entry['moduleName'] ?? entry['id'] ?? entry['entryId'] ?? ''}';
    final canonical = DshPluginSettingsZh.canonical(moduleName);
    final fallback =
        '${entry['name'] ?? entry['title'] ?? entry['moduleName'] ?? entry['id'] ?? entry['entryId'] ?? ''}';
    final title = DshPluginSettingsZh.title(moduleName, fallback);
    final description = DshPluginSettingsZh.description(
      moduleName,
      '${entry['description'] ?? ''}',
    );
    final phase = entry['fiberPhase'];
    final enabled = entry['enabled'] == true;
    final onTap = onConfigure ?? onOpen;
    final radius = BorderRadius.circular(10);
    final tone = switch (canonical) {
      'dsh-auto-review' || 'dsh-experimental-auto-review' => colors.success,
      'dsh-time-context' || 'dsh-schedule' => colors.info,
      _ => colors.muted,
    };
    final row = Material(
      color: Colors.transparent,
      borderRadius: radius,
      child: InkWell(
        onTap: onTap,
        borderRadius: radius,
        hoverColor: colors.hover,
        focusColor: colors.hover,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 14),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  color: tone.withValues(alpha: colors.dark ? .13 : .06),
                  borderRadius: BorderRadius.circular(12),
                  border: Border.all(color: colors.border),
                ),
                child: DshGlyph(iconFor(canonical).data, size: 20, color: tone),
              ),
              const SizedBox(width: 16),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Wrap(
                      spacing: 8,
                      runSpacing: 4,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        Text(
                          title,
                          style: DshTypography.composer.copyWith(
                            fontWeight: FontWeight.w500,
                            color: colors.text,
                          ),
                        ),
                        if (entry['experimental'] == true)
                          Container(
                            padding: const EdgeInsets.symmetric(
                              horizontal: 6,
                              vertical: 1,
                            ),
                            decoration: BoxDecoration(
                              color: colors.layer,
                              borderRadius: BorderRadius.circular(4),
                              border: Border.all(color: colors.border),
                            ),
                            child: Text(
                              DshPluginSettingsZh.experimental,
                              style: DshTypography.caption.copyWith(
                                color: colors.muted,
                              ),
                            ),
                          ),
                      ],
                    ),
                    if (description.isNotEmpty) ...[
                      const SizedBox(height: 4),
                      Text(
                        description,
                        maxLines: 3,
                        overflow: TextOverflow.ellipsis,
                        style: DshTypography.auxiliary.copyWith(
                          color: colors.muted,
                        ),
                      ),
                    ],
                    const SizedBox(height: 6),
                    _PluginState(
                      key: ValueKey('plugin-state-$canonical'),
                      label: DshPluginSettingsZh.state(enabled, phase),
                      color: !enabled
                          ? colors.muted
                          : switch (phase) {
                              'active' => colors.success,
                              'failed' => DshTokens.of(
                                context,
                              ).error.foreground,
                              'pending' ||
                              'loading' ||
                              'unloading' => colors.warning,
                              _ => colors.muted,
                            },
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 12),
              if (onTap != null) ...[
                DshTooltip(
                  message: onConfigure != null
                      ? (expanded
                            ? DshPluginSettingsZh.collapseConfiguration
                            : DshPluginSettingsZh.expandConfiguration)
                      : DshPluginSettingsZh.open(title),
                  child: DshGlyph(
                    onConfigure == null
                        ? DshIcons.chevronRight.data
                        : expanded
                        ? DshIcons.chevronUp.data
                        : DshIcons.chevronDown.data,
                    color: colors.muted,
                    size: 14,
                  ),
                ),
                const SizedBox(width: 12),
              ],
              Semantics(
                label: DshPluginSettingsZh.toggle(title),
                child: DshSwitch(value: enabled, onChanged: onEnabledChanged),
              ),
            ],
          ),
        ),
      ),
    );
    return Semantics(
      hint: onConfigure != null
          ? DshPluginSettingsZh.configuration
          : onOpen != null
          ? DshPluginSettingsZh.open(title)
          : null,
      // Hairline separators keep the rows reading as one list.
      child: DecoratedBox(
        decoration: BoxDecoration(
          border: Border(
            bottom: BorderSide(color: colors.border.withValues(alpha: .6)),
          ),
        ),
        child: Padding(padding: const EdgeInsets.only(bottom: 1), child: row),
      ),
    );
  }
}

class _PluginState extends StatelessWidget {
  const _PluginState({super.key, required this.label, required this.color});
  final String label;
  final Color color;
  @override
  Widget build(BuildContext context) => Row(
    mainAxisSize: MainAxisSize.min,
    children: [
      Container(
        width: 6,
        height: 6,
        decoration: BoxDecoration(color: color, shape: BoxShape.circle),
      ),
      const SizedBox(width: 6),
      Flexible(
        child: Text(
          label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: DshTypography.caption.copyWith(color: color),
        ),
      ),
    ],
  );
}
