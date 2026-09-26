import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';

/// Same level names as the Web model menu (`effort.level.*`).
const reasoningLevelNames = {
  'none': '不推理',
  'off': '关闭',
  'minimal': '极低',
  'low': '低',
  'medium': '中',
  'high': '高',
  'xhigh': '极高',
  'max': '最高',
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

  int get selected {
    final index = widget.levels.indexWhere(
      (level) => '${level['id']}' == widget.value,
    );
    return index < 0 ? 0 : index;
  }

  void commit(int index) {
    setState(() => dragging = null);
    if (index == selected && widget.value != null) return;
    widget.onChanged('${widget.levels[index]['id']}');
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final levels = widget.levels;
    if (levels.isEmpty) return const SizedBox.shrink();
    final shown = dragging?.round() ?? selected;
    return Column(
      key: const Key('reasoning-slider'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Text(
              '推理强度',
              style: DshTypography.caption.copyWith(color: colors.muted),
            ),
            const Spacer(),
            Text(
              reasoningLevelLabel(levels[shown]),
              style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
            ),
          ],
        ),
        if (levels.length > 1)
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
              onChanged: widget.enabled
                  ? (value) => setState(() => dragging = value)
                  : null,
              onChangeEnd: widget.enabled
                  ? (value) => commit(value.round())
                  : null,
            ),
          ),
        Row(
          children: [
            for (final (index, level) in levels.indexed)
              Expanded(
                child: InkWell(
                  key: ValueKey('reasoning-level-${level['id']}'),
                  borderRadius: BorderRadius.circular(6),
                  onTap: widget.enabled ? () => commit(index) : null,
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
                        fontSize: 12,
                        fontWeight: index == shown ? FontWeight.w600 : null,
                        color: index == shown ? colors.text : colors.muted,
                      ),
                    ),
                  ),
                ),
              ),
          ],
        ),
      ],
    );
  }
}
