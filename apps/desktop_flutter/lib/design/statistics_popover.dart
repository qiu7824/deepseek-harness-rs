import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'primitives.dart';
import 'typography.dart';

/// Shared read-only presentation for session and individual-turn measurements.
class StatisticsPopover extends StatefulWidget {
  const StatisticsPopover({
    super.key,
    required this.label,
    required this.title,
    required this.rows,
    this.icon,
    this.scope,
    this.details,
  });
  final String label, title;
  final List<(String, String)> rows;
  final IconData? icon;
  final Object? scope;
  final Widget? details;
  @override
  State<StatisticsPopover> createState() => _StatisticsPopoverState();
}

class _StatisticsPopoverState extends State<StatisticsPopover> {
  final popover = ShadPopoverController();
  final trigger = FocusNode();
  final panelFocus = FocusNode();
  bool keyboardActive = false;

  @override
  void initState() {
    super.initState();
    popover.addListener(syncKeyboard);
  }

  void syncKeyboard() {
    if (popover.isOpen == keyboardActive) return;
    keyboardActive = popover.isOpen;
    if (keyboardActive) {
      FocusManager.instance.addEarlyKeyEventHandler(handleKey);
    } else {
      FocusManager.instance.removeEarlyKeyEventHandler(handleKey);
    }
  }

  KeyEventResult handleKey(KeyEvent event) {
    if (!mounted || ModalRoute.of(context)?.isCurrent == false) {
      return KeyEventResult.ignored;
    }
    if (popover.isOpen &&
        event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      close();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  void close() {
    popover.hide();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && ModalRoute.of(context)?.isCurrent != false) {
        trigger.requestFocus();
      }
    });
  }

  @override
  void didUpdateWidget(StatisticsPopover oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.scope != widget.scope && popover.isOpen) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) popover.hide();
      });
    }
  }

  @override
  void dispose() {
    popover.removeListener(syncKeyboard);
    if (keyboardActive) {
      FocusManager.instance.removeEarlyKeyEventHandler(handleKey);
    }
    popover.dispose();
    trigger.dispose();
    panelFocus.dispose();
    super.dispose();
  }

  Widget content(BuildContext context) {
    final size = MediaQuery.sizeOf(context);
    final colors = DshColors(context);
    return DefaultTextStyle(
      style: DshTypography.body.copyWith(color: colors.text),
      textAlign: TextAlign.start,
      child: CallbackShortcuts(
        bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
        child: Focus(
          focusNode: panelFocus,
          autofocus: true,
          child: ConstrainedBox(
            constraints: BoxConstraints(
              maxWidth: math.max(120, math.min(360, size.width - 56)),
              maxHeight: math.max(80, math.min(480, size.height - 96)),
            ),
            child: SingleChildScrollView(
              child: SizedBox(
                width: math.max(120, math.min(360, size.width - 56)),
                child: Padding(
                  padding: const EdgeInsets.only(right: 16),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Text(
                        widget.title,
                        style: TextStyle(
                          fontSize: DshTypography.sizeBody,
                          fontWeight: FontWeight.w600,
                          color: colors.text,
                        ),
                      ),
                      const SizedBox(height: 10),
                      Divider(height: 1, color: colors.border),
                      const SizedBox(height: 10),
                      for (final row in widget.rows)
                        Padding(
                          padding: const EdgeInsets.symmetric(vertical: 5),
                          child: LayoutBuilder(
                            builder: (context, constraints) {
                              final label = Text(
                                row.$1,
                                style: TextStyle(
                                  fontSize: DshTypography.sizeCaption,
                                  height: 1.5,
                                  color: colors.muted,
                                ),
                              );
                              final stacked =
                                  constraints.maxWidth < 250 ||
                                  MediaQuery.textScalerOf(context).scale(1) >
                                      1.5;
                              final value = SelectableText(
                                row.$2,
                                textAlign: stacked
                                    ? TextAlign.start
                                    : TextAlign.end,
                                style: TextStyle(
                                  fontSize: DshTypography.sizeCaption,
                                  height: 1.5,
                                  color: colors.text,
                                  fontFeatures: const [
                                    FontFeature.tabularFigures(),
                                  ],
                                ),
                              );
                              return stacked
                                  ? Column(
                                      crossAxisAlignment:
                                          CrossAxisAlignment.start,
                                      children: [
                                        label,
                                        const SizedBox(height: 3),
                                        value,
                                      ],
                                    )
                                  : Row(
                                      crossAxisAlignment:
                                          CrossAxisAlignment.start,
                                      children: [
                                        Expanded(flex: 4, child: label),
                                        const SizedBox(width: 16),
                                        Expanded(flex: 6, child: value),
                                      ],
                                    );
                            },
                          ),
                        ),
                      if (widget.details != null) widget.details!,
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) => CallbackShortcuts(
    bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
    child: ShadPopover(
      controller: popover,
      padding: const EdgeInsets.all(14),
      anchor: const ShadAnchorAuto(
        targetAnchor: Alignment.topRight,
        followerAnchor: Alignment.topLeft,
        offset: Offset(0, -8),
        fallback: ShadAnchorAuto(
          targetAnchor: Alignment.bottomRight,
          followerAnchor: Alignment.bottomLeft,
          offset: Offset(0, 8),
        ),
      ),
      popover: content,
      child: ShadButton.ghost(
        focusNode: trigger,
        height: DshTokens.of(context).controlHeight(context),
        padding: const EdgeInsets.symmetric(horizontal: 8),
        onPressed: () => popover.isOpen ? close() : popover.show(),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (widget.icon != null) ...[
              DshGlyph(widget.icon!, size: 14, color: DshColors(context).muted),
              const SizedBox(width: 6),
            ],
            Flexible(
              child: Text(
                widget.label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: DshColors(context).muted,
                ),
              ),
            ),
          ],
        ),
      ),
    ),
  );
}
