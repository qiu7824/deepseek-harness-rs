import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'primitives.dart';
import 'typography.dart';

class DshSelect<T extends Object> extends StatefulWidget {
  const DshSelect({
    super.key,
    required this.options,
    required this.value,
    required this.onChanged,
    this.placeholder = '请选择',
    this.maxWidth = 270,
    this.outline = false,
  });
  final Map<T, String> options;
  final T? value;
  final ValueChanged<T>? onChanged;
  final String placeholder;
  final double maxWidth;
  final bool outline;
  @override
  State<DshSelect<T>> createState() => _DshSelectState<T>();
}

class _DshSelectState<T extends Object> extends State<DshSelect<T>> {
  late final controller = ShadSelectController<T>(
    initialValue: widget.value == null ? {} : {widget.value!},
  );
  @override
  void didUpdateWidget(covariant DshSelect<T> oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.value != widget.value) {
      controller.value = widget.value == null ? {} : {widget.value!};
    }
  }

  @override
  void dispose() {
    controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final text = widget.options[widget.value] ?? widget.placeholder;
    final painter = TextPainter(
      text: TextSpan(text: text, style: DshTypography.body),
      textDirection: Directionality.of(context),
      maxLines: 1,
    )..layout();
    final width = (painter.width + 54).clamp(76.0, widget.maxWidth).toDouble();
    painter.dispose();
    final colors = DshColors(context);
    return SizedBox(
      height: 36,
      width: width,
      child: ShadSelect<T>(
        controller: controller,
        enabled: widget.onChanged != null,
        minWidth: width,
        maxWidth: width,
        maxHeight: 320,
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 5),
        decoration: ShadDecoration(
          color: widget.outline
              ? colors.base
              : colors.dark
              ? colors.layer
              : const Color(0xfff5f6f7),
          border: ShadBorder.all(
            color: colors.border,
            width: widget.outline ? 1 : 0,
            radius: BorderRadius.circular(widget.outline ? 8 : 18),
          ),
        ),
        trailing: DshGlyph(
          LucideIcons.chevronDown,
          size: 14,
          color: colors.muted,
        ),
        placeholder: Text(widget.placeholder, style: DshTypography.body),
        options: [
          for (final item in widget.options.entries)
            ShadOption(
              value: item.key,
              child: Text(item.value, style: DshTypography.body),
            ),
        ],
        selectedOptionBuilder: (_, value) => Text(
          widget.options[value] ?? '$value',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: DshTypography.body,
        ),
        onChanged: (value) {
          if (value != null) widget.onChanged?.call(value);
        },
      ),
    );
  }
}
