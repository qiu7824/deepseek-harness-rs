import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';

import 'package:dsh_desktop/l10n/conversation_zh.dart';

/// Same level names as the Web model menu (`effort.level.*`).
const reasoningLevelNames = {
  'none': DshConversationZh.reasoningNone,
  'off': DshConversationZh.close,
  'minimal': DshConversationZh.reasoningMinimal,
  'low': DshConversationZh.reasoningLow,
  'medium': DshConversationZh.reasoningMedium,
  'high': DshConversationZh.reasoningHigh,
  'xhigh': DshConversationZh.reasoningExtraHigh,
  'max': DshConversationZh.reasoningMaximum,
};

const _aliases = {
  'none': ['none'],
  'off': ['off', 'disabled'],
  'minimal': ['minimal'],
  'low': ['low'],
  'medium': ['medium'],
  'high': ['high'],
  'xhigh': ['xhigh', 'extrahigh'],
  'max': ['max', 'maximum'],
};

/// Localized label for one provider-declared effort; a custom name that is
/// not one of the standard levels is shown as declared.
String reasoningLevelLabel(Json effort) {
  final id = '${effort['id'] ?? ''}';
  final name = '${effort['name'] ?? id}';
  final normalized = name.toLowerCase().replaceAll(RegExp(r'[ -]'), '');
  return (_aliases[id]?.contains(normalized) ?? false)
      ? reasoningLevelNames[id]!
      : name;
}

/// Discrete slider over the current model's reasoning levels. The value is
/// committed once when the drag ends or a level label is clicked.
class ReasoningSlider extends StatefulWidget {
  const ReasoningSlider({
    super.key,
    required this.levels,
    required this.value,
    required this.onChanged,
    this.enabled = true,
  });
  final List<Json> levels;
  final String? value;
  final Future<void> Function(String id) onChanged;
  final bool enabled;
  @override
  State<ReasoningSlider> createState() => _ReasoningSliderState();
}

class _ReasoningSliderState extends State<ReasoningSlider> {
  double? dragging;
  bool committing = false;
  String? failure;
  int revision = 0;

  @override
  void didUpdateWidget(ReasoningSlider oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.value != widget.value ||
        oldWidget.enabled && !widget.enabled ||
        oldWidget.levels.map((level) => level['id']).join('\u0000') !=
            widget.levels.map((level) => level['id']).join('\u0000')) {
      dragging = null;
      failure = null;
    }
  }

  int get selected {
    final index = widget.levels.indexWhere(
      (level) => '${level['id']}' == widget.value,
    );
    return index;
  }

  Future<void> commit(int index) async {
    if (!widget.enabled ||
        committing ||
        index < 0 ||
        index >= widget.levels.length) {
      return;
    }
    if (index == selected) {
      setState(() => dragging = null);
      return;
    }
    final action = ++revision;
    final levels = widget.levels.map((level) => level['id']).join('\u0000');
    setState(() {
      dragging = null;
      failure = null;
      committing = true;
    });
    try {
      await widget.onChanged('${widget.levels[index]['id']}');
    } catch (error) {
      if (mounted &&
          action == revision &&
          levels == widget.levels.map((level) => level['id']).join('\u0000')) {
        setState(() => failure = '无法更新推理等级：$error');
      }
    } finally {
      if (mounted && action == revision) setState(() => committing = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final levels = widget.levels;
    if (levels.isEmpty) return const SizedBox.shrink();
    final shown = dragging?.round() ?? selected;
    final enabled = widget.enabled && !committing;
    return Column(
      key: const Key('reasoning-slider'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Text(
              DshConversationZh.reasoningStrength,
              style: DshTypography.caption.copyWith(color: colors.muted),
            ),
            const Spacer(),
            Text(
              dragging != null
                  ? '预览：${reasoningLevelLabel(levels[shown])}'
                  : selected >= 0
                  ? reasoningLevelLabel(levels[selected])
                  : widget.value == null
                  ? '模型默认'
                  : '${reasoningLevelNames[widget.value] ?? widget.value}（未列出）',
              style: const TextStyle(
                fontSize: DshTypography.sizeAuxiliary,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
        if (levels.length > 1 && selected >= 0)
          SliderTheme(
            data: SliderTheme.of(context).copyWith(
              trackHeight: 4,
              activeTrackColor: colors.blue,
              inactiveTrackColor: colors.border,
              thumbColor: colors.blue,
              overlayShape: const RoundSliderOverlayShape(overlayRadius: 14),
              thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 8),
              tickMarkShape: const RoundSliderTickMarkShape(tickMarkRadius: 2),
              activeTickMarkColor: Colors.white,
              inactiveTickMarkColor: colors.muted,
              showValueIndicator: ShowValueIndicator.never,
            ),
            child: Slider(
              value: (dragging ?? selected.toDouble()).clamp(
                0,
                levels.length - 1.0,
              ),
              min: 0,
              max: levels.length - 1.0,
              divisions: levels.length - 1,
              semanticFormatterCallback: (value) =>
                  reasoningLevelLabel(levels[value.round()]),
              onChanged: enabled
                  ? (value) => setState(() => dragging = value)
                  : null,
              onChangeEnd: enabled ? (value) => commit(value.round()) : null,
            ),
          ),
        Row(
          children: [
            for (final (index, level) in levels.indexed)
              Expanded(
                child: InkWell(
                  key: ValueKey('reasoning-level-${level['id']}'),
                  borderRadius: BorderRadius.circular(6),
                  onTap: enabled ? () => commit(index) : null,
                  child: Padding(
                    padding: const EdgeInsets.symmetric(vertical: 4),
                    child: Text(
                      reasoningLevelLabel(level),
                      textAlign: levels.length == 1
                          ? TextAlign.start
                          : index == 0
                          ? TextAlign.start
                          : index == levels.length - 1
                          ? TextAlign.end
                          : TextAlign.center,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        fontWeight: index == shown ? FontWeight.w600 : null,
                        color: index == shown ? colors.text : colors.muted,
                      ),
                    ),
                  ),
                ),
              ),
          ],
        ),
        if (failure != null)
          Text(
            failure!,
            key: const ValueKey('reasoning-update-error'),
            style: DshTypography.caption.copyWith(
              color: Theme.of(context).colorScheme.error,
            ),
          ),
      ],
    );
  }
}
