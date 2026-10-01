import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
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
  final popover = ShadPopoverController();
  @override
  void didUpdateWidget(covariant DshSelect<T> oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!setEquals(
      controller.value,
      widget.value == null ? <T>{} : {widget.value!},
    )) {
      controller.value = widget.value == null ? {} : {widget.value!};
    }
    if (widget.onChanged == null ||
        !mapEquals(oldWidget.options, widget.options)) {
      popover.hide();
    }
  }

  @override
  void dispose() {
    controller.dispose();
    popover.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final text = widget.options[widget.value] ?? widget.placeholder;
    final painter = TextPainter(
      text: TextSpan(text: text, style: DshTypography.body),
      textDirection: Directionality.of(context),
      textScaler: MediaQuery.textScalerOf(context),
      maxLines: 1,
    )..layout();
    final width = (painter.width + 54).clamp(76.0, widget.maxWidth).toDouble();
    painter.dispose();
    final colors = DshColors(context);
    return SizedBox(
      height: DshTokens.of(context).controlHeight(context),
      width: width,
      child: ShadSelect<T>(
        controller: controller,
        popoverController: popover,
        enabled: widget.onChanged != null,
        minWidth: width,
        maxWidth: widget.maxWidth,
        maxHeight: 320,
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 5),
        decoration: ShadDecoration(
          color: widget.outline ? colors.base : colors.layer,
          border: ShadBorder.all(
            color: colors.border,
            width: widget.outline ? 1 : 0,
            radius: BorderRadius.circular(DshTokens.of(context).radiusControl),
          ),
        ),
        trailing: DshGlyph(
          DshIcons.chevronDown.data,
          size: 14,
          color: colors.muted,
        ),
        placeholder: Text(widget.placeholder, style: DshTypography.body),
        options: [
          for (final item in widget.options.entries)
            ShadOption(
              value: item.key,
              child: Text(
                item.value,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: DshTypography.body,
              ),
            ),
        ],
        selectedOptionBuilder: (_, value) => Text(
          widget.options[value] ?? '$value',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: DshTypography.body,
        ),
        onChanged: (value) {
          if (value == null || !widget.options.containsKey(value)) return;
          widget.onChanged?.call(value);
          // This is a controlled input. The owner may reject or asynchronously
          // confirm the change, so retain its value until it rebuilds.
          controller.value = widget.value == null ? {} : {widget.value!};
        },
      ),
    );
  }
}
