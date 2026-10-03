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

/// Current conversation usage and compaction controls for its confirmed model.
/// A gesture retains its original session and model until it is committed.
class ContextQuickSettings extends StatefulWidget {
  const ContextQuickSettings({
    super.key,
    required this.controller,
    this.showUsage = true,
  });
  final DesktopController controller;
  final bool showUsage;
  @override
  State<ContextQuickSettings> createState() => _ContextQuickSettingsState();
}

class _ContextQuickSettingsState extends State<ContextQuickSettings> {
  double? dragging;
  bool busy = false;
  String? notice;
  late Object observedScope;
  Object? dragScope, savingScope;
  double? savingPercent;
  int interactionRevision = 0;
  int? dragRevision;
  bool wasBlocked = false;

  DesktopController get c => widget.controller;

  String? get modelKey {
    final current = c.catalog?.current;
    if (current == null ||
        current['provider'] is! String ||
        current['model'] is! String ||
        (current['provider'] as String).isEmpty ||
        (current['model'] as String).isEmpty) {
      return null;
    }
    return '${current['provider']}/${current['model']}';
  }

  Object get scope =>
      (c, c.client, c.host, c.selectedId, c.selectionRevision, modelKey);

  bool get blocked =>
      c.changingModel || c.modelSelectionUnconfirmed || c.sending;

  bool canWrite(Object target) =>
      mounted &&
      target == scope &&
      !busy &&
      !blocked &&
      c.client != null &&
      c.host != null &&
      c.selectedId != null &&
      modelKey != null;

  @override
  void initState() {
    super.initState();
    observedScope = scope;
    wasBlocked = blocked;
    c.addListener(changed);
  }

  @override
  void didUpdateWidget(ContextQuickSettings oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, c)) {
      oldWidget.controller.removeListener(changed);
      c.addListener(changed);
      changed();
    }
  }

  void changed() {
    if (!mounted) return;
    final next = scope;
    final nowBlocked = blocked;
    if (observedScope != next || !wasBlocked && nowBlocked) {
      // Rebuilding alone does not cancel the render slider's active gesture.
      // Retire its callbacks even if the old model or session is selected again.
      interactionRevision++;
      dragging = null;
      dragScope = null;
      dragRevision = null;
      notice = null;
      savingScope = null;
      savingPercent = null;
    }
    observedScope = next;
    wasBlocked = nowBlocked;
    setState(() {});
  }

  @override
  void dispose() {
    c.removeListener(changed);
    super.dispose();
  }

  void beginDrag(Object target, int revision, double value) {
    if (!canWrite(target) || revision != interactionRevision) return;
    setState(() {
      dragScope = target;
      dragRevision = revision;
      dragging = value;
      notice = null;
    });
  }

  bool activeDrag(Object target, int revision) =>
      canWrite(target) &&
      revision == interactionRevision &&
      dragScope == target &&
      dragRevision == revision;

  void finishDrag(Object target, int revision, String key, double value) {
    if (!activeDrag(target, revision)) return;
    final thresholds = object(c.contextCompaction['thresholds']);
    final effective =
        (thresholds[key] as num?)?.toDouble() ??
        (thresholds['*'] as num?)?.toDouble() ??
        defaultCompactionThreshold;
    setState(() {
      dragging = null;
      dragScope = null;
      dragRevision = null;
    });
    final ratio = value.round() / 100;
    if ((ratio - effective).abs() < .001) return;
    final owner = c;
    run(
      target,
      revision,
      () => owner.setCompactionThreshold(key, ratio),
      percent: value,
    );
  }

  Future<void> run(
    Object target,
    int revision,
    Future<void> Function() action, {
    String? done,
    double? percent,
  }) async {
    if (!canWrite(target) || revision != interactionRevision) return;
    final operationRevision = interactionRevision;
    bool current() =>
        mounted && target == scope && operationRevision == interactionRevision;
    setState(() {
      busy = true;
      notice = null;
      savingScope = target;
      savingPercent = percent;
    });
    try {
      await action();
      if (current() && done != null) {
        setState(() => notice = done);
      }
    } catch (e) {
      if (current()) setState(() => notice = '$e');
    } finally {
      // A section write already accepted by the Host cannot be recalled.
      // Keep this widget serialized across scope changes until it settles.
      if (mounted) {
        setState(() {
          busy = false;
          savingScope = null;
          savingPercent = null;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final pressure = object(c.projections['contextPressure']);
    final used =
        (pressure['projectedTokens'] ?? pressure['pressureTokens']) as num?;
    final window = pressure['contextWindow'] as num?;
    final thresholds = object(c.contextCompaction['thresholds']);
    final key = modelKey;
    final target = scope;
    final revision = interactionRevision;
    final enabled = canWrite(target);
    final stored = key == null ? null : (thresholds[key] as num?)?.toDouble();
    final effective =
        stored ??
        (thresholds['*'] as num?)?.toDouble() ??
        defaultCompactionThreshold;
    final pending = savingScope == target;
    final percent =
        (dragging ?? (pending ? savingPercent : null) ?? effective * 100).clamp(
          30.0,
          98.0,
        );
    final usage = used == null || window == null || window == 0
        ? DshConversationZh.noUsage
        : '${(used / window * 100).clamp(0, 100).round()}% · ${compactTokens(used)} / ${compactTokens(window)}';
    return Column(
      key: const Key('context-quick-settings'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (widget.showUsage) ...[
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
        ],
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
            min: 30,
            max: 98,
            divisions: 68,
            semanticFormatterCallback: (value) => '${value.round()}%',
            onChangeStart: !enabled
                ? null
                : (value) => beginDrag(target, revision, value),
            onChanged: !enabled
                ? null
                : (value) {
                    if (activeDrag(target, revision)) {
                      setState(() => dragging = value);
                    }
                  },
            onChangeEnd: !enabled || key == null
                ? null
                : (value) => finishDrag(target, revision, key, value),
          ),
        ),
        Row(
          children: [
            Expanded(
              child: Text(
                notice ??
                    (pending && savingPercent != null
                        ? DshConversationZh.saving
                        : null) ??
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
                onPressed: !enabled || key == null
                    ? null
                    : () => run(
                        target,
                        revision,
                        () => c.setCompactionThreshold(key, null),
                      ),
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
              onPressed: !enabled || c.compacting
                  ? null
                  : () => run(
                      target,
                      revision,
                      c.compactNow,
                      done: DshConversationZh.compactionStarted,
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
  }
}
