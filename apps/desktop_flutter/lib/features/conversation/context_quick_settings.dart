import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';
import '../../src/controller.dart';

import 'package:dsh_desktop/l10n/conversation_zh.dart';

/// Default automatic compaction threshold (compaction-basic `thresholdRatio`).
const defaultCompactionThreshold = 0.8;

String compactTokens(num value) {
  if (value >= 10000) {
    final wan = value / 10000;
    return DshConversationZh.tenThousands(
      amount: wan >= 100 ? wan.round() : (wan * 10).round() / 10,
    );
  }
  return '${value.round()}';
}

/// Usage of the current conversation, the current model's automatic
/// compaction threshold and a compact-now action, shown in the model menu.
class ContextQuickSettings extends StatefulWidget {
  const ContextQuickSettings({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<ContextQuickSettings> createState() => _ContextQuickSettingsState();
}

class _ContextQuickSettingsState extends State<ContextQuickSettings> {
  double? dragging;
  bool busy = false;
  String? notice;

  DesktopController get c => widget.controller;

  String? get modelKey {
    final current = c.catalog?.current;
    if (current == null || current['provider'] == null) return null;
    return '${current['provider']}/${current['model']}';
  }

  Future<void> run(Future<void> Function() action, [String? done]) async {
    setState(() {
      busy = true;
      notice = null;
    });
    try {
      await action();
      if (mounted && done != null) setState(() => notice = done);
    } catch (e) {
      if (mounted) setState(() => notice = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: c,
    builder: (context, _) {
      final colors = DshColors(context);
      final pressure = object(c.projections['contextPressure']);
      final used =
          (pressure['projectedTokens'] ?? pressure['pressureTokens']) as num?;
      final window = pressure['contextWindow'] as num?;
      final thresholds = object(c.contextCompaction['thresholds']);
      final key = modelKey;
      final stored = key == null ? null : (thresholds[key] as num?)?.toDouble();
      final effective =
          stored ??
          (thresholds['*'] as num?)?.toDouble() ??
          defaultCompactionThreshold;
      final percent = (dragging ?? effective * 100).clamp(50.0, 95.0);
      final usage = used == null || window == null || window == 0
          ? DshConversationZh.noUsage
          : '${(used / window * 100).clamp(0, 100).round()}% · ${compactTokens(used)} / ${compactTokens(window)}';
      return Column(
        key: const Key('context-quick-settings'),
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Text(
                DshConversationZh.context,
                style: DshTypography.caption.copyWith(color: colors.muted),
              ),
              const Spacer(),
              Text(
                usage,
                style: const TextStyle(
                  fontSize: DshTypography.sizeAuxiliary,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
          if (used != null && window != null && window > 0) ...[
            const SizedBox(height: 6),
            ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: LinearProgressIndicator(
                value: (used / window).clamp(0, 1).toDouble(),
                minHeight: 4,
                color: colors.blue,
                backgroundColor: colors.border,
              ),
            ),
          ],
          const SizedBox(height: 8),
          Row(
            children: [
              Text(
                DshConversationZh.autoCompactionThreshold,
                style: DshTypography.caption.copyWith(color: colors.muted),
              ),
              const Spacer(),
              Text(
                '${percent.round()}%',
                style: const TextStyle(
                  fontSize: DshTypography.sizeAuxiliary,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
          SliderTheme(
            data: SliderTheme.of(context).copyWith(
              trackHeight: 4,
              activeTrackColor: colors.blue,
              inactiveTrackColor: colors.border,
              thumbColor: colors.blue,
              overlayShape: const RoundSliderOverlayShape(overlayRadius: 14),
              thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 8),
              showValueIndicator: ShowValueIndicator.never,
            ),
            child: Slider(
              key: const Key('compaction-threshold'),
              value: percent.toDouble(),
              min: 50,
              max: 95,
              divisions: 9,
              semanticFormatterCallback: (value) => '${value.round()}%',
              onChanged: key == null || busy
                  ? null
                  : (value) => setState(() => dragging = value),
              onChangeEnd: key == null || busy
                  ? null
                  : (value) {
                      setState(() => dragging = null);
                      final ratio = value.round() / 100;
                      if ((ratio - effective).abs() < .001) return;
                      run(() => c.setCompactionThreshold(key, ratio));
                    },
            ),
          ),
          Row(
            children: [
              Expanded(
                child: Text(
                  notice ??
                      (stored == null
                          ? DshConversationZh.useDefaultThreshold
                          : DshConversationZh.customThreshold),
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
                ),
              ),
              if (stored != null)
                DshButton(
                  key: const Key('compaction-threshold-reset'),
                  height: 28,
                  onPressed: busy || key == null
                      ? null
                      : () => run(() => c.setCompactionThreshold(key, null)),
                  child: const Text(
                    DshConversationZh.restoreDefaults,
                    style: TextStyle(fontSize: DshTypography.sizeCaption),
                  ),
                ),
              const SizedBox(width: 6),
              DshButton(
                key: const Key('compact-now'),
                height: 28,
                outline: true,
                onPressed: busy || c.compacting || c.selectedId == null
                    ? null
                    : () => run(
                        c.compactNow,
                        DshConversationZh.compactionStarted,
                      ),
                child: Text(
                  c.compacting
                      ? DshConversationZh.compacting
                      : DshConversationZh.compactNow,
                  style: const TextStyle(fontSize: DshTypography.sizeCaption),
                ),
              ),
            ],
          ),
        ],
      );
    },
  );
}
