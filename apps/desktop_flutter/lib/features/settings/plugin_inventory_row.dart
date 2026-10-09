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
    _ => DshIcons.plugins,
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
    final active = phase == 'active';
    final onTap = onConfigure ?? onOpen;
    final radius = BorderRadius.circular(12);
    return Material(
      color: Colors.transparent,
      borderRadius: radius,
      child: InkWell(
        onTap: onTap,
        borderRadius: radius,
        hoverColor: colors.hover,
        focusColor: colors.hover,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 18),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              Container(
                width: 48,
                height: 48,
                decoration: BoxDecoration(
                  color: colors.layer,
                  borderRadius: BorderRadius.circular(12),
                ),
                child: DshGlyph(
                  iconFor(canonical).data,
                  size: 22,
                  color: colors.text,
                ),
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
                    Wrap(
                      spacing: 10,
                      runSpacing: 3,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        Text(
                          DshPluginSettingsZh.configured(
                            entry['enabled'] == true,
                          ),
                          style: DshTypography.caption.copyWith(
                            color: colors.muted,
                          ),
                        ),
                        Text(
                          DshPluginSettingsZh.runtimeStatus(phase),
                          style: DshTypography.caption.copyWith(
                            color: active ? colors.success : colors.muted,
                          ),
                        ),
                      ],
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 12),
              if (onTap != null) ...[
                DshGlyph(
                  onConfigure != null && expanded
                      ? DshIcons.chevronUp.data
                      : DshIcons.chevronRight.data,
                  color: colors.muted,
                  size: 14,
                ),
                const SizedBox(width: 12),
              ],
              Semantics(
                label: DshPluginSettingsZh.toggle(title),
                child: DshSwitch(
                  value: entry['enabled'] == true,
                  onChanged: onEnabledChanged,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
