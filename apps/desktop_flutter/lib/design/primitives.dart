import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../l10n/zh.dart';
import 'error.dart';
import 'icon_assets.dart';
import 'icons.dart';
import 'motion.dart';
import 'tokens.dart';
import 'typography.dart';

export 'icons.dart';
export 'tokens.dart';

class DshGlyph extends StatelessWidget {
  const DshGlyph(
    this.data, {
    super.key,
    this.size,
    this.color,
    this.semanticLabel,
    this.asset,
  });
  final IconData? data;
  final double? size;
  final Color? color;
  final String? semanticLabel;
  final String? asset;
  @override
  Widget build(BuildContext context) {
    final dimension = size ?? IconTheme.of(context).size ?? 16;
    final path =
        asset ??
        DshIcons.assetFor(data) ??
        (data?.fontPackage == 'lucide_icons_flutter'
            ? dshIconAssets[data!.codePoint]
            : null);
    if (path == null) {
      return Icon(
        data,
        size: dimension,
        color: color,
        semanticLabel: semanticLabel,
      );
    }
    return Center(
      widthFactor: 1,
      heightFactor: 1,
      child: SizedBox(
        width: dimension,
        height: dimension,
        child: SvgPicture.asset(
          path,
          fit: BoxFit.contain,
          semanticsLabel: semanticLabel,
          excludeFromSemantics: semanticLabel == null,
          theme: SvgTheme(
            currentColor:
                color ?? IconTheme.of(context).color ?? DshColors(context).text,
          ),
        ),
      ),
    );
  }
}

class DshColors {
  DshColors(BuildContext context) : tokens = DshTokens.of(context);
  final DshTokens tokens;
  bool get dark => tokens.isDark;
  Color get base => tokens.base;
  Color get sidebar => tokens.sidebar;
  Color get layer => tokens.layer;
  Color get hover => tokens.hover;
  Color get selected => tokens.selected;
  Color get border => tokens.border;
  Color get text => tokens.text;
  Color get muted => tokens.muted;
  Color get blue => tokens.accent;
  Color get bubble => tokens.bubble;
  Color get success => tokens.success.foreground;
  Color get warning => tokens.warning.foreground;
  Color get error => tokens.error.foreground;
  Color get info => tokens.info.foreground;
  Color get onAccent => tokens.onAccent;
  Color get focus => tokens.focus;
}

/// Desktop hover hints use the same reading style on every control.
class DshTooltip extends StatelessWidget {
  const DshTooltip({
    super.key,
    required this.message,
    required this.child,
    this.excludeFromSemantics = false,
  });

  final String message;
  final Widget child;
  final bool excludeFromSemantics;

  static TooltipThemeData theme(DshTokens tokens) => TooltipThemeData(
    waitDuration: const Duration(milliseconds: 500),
    exitDuration: const Duration(milliseconds: 100),
    showDuration: const Duration(seconds: 4),
    padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
    margin: const EdgeInsets.all(12),
    textStyle: DshTypography.auxiliary.copyWith(color: tokens.base),
    decoration: BoxDecoration(
      color: tokens.text,
      borderRadius: BorderRadius.circular(tokens.radiusControl),
    ),
  );

  @override
  Widget build(BuildContext context) => Tooltip(
    message: message,
    excludeFromSemantics: excludeFromSemantics,
    ignorePointer: true,
    waitDuration: const Duration(milliseconds: 500),
    exitDuration: const Duration(milliseconds: 100),
    child: child,
  );
}

class DshButton extends StatelessWidget {
  const DshButton({
    super.key,
    required this.child,
    this.onPressed,
    this.icon,
    this.primary = false,
    this.outline = false,
    this.height = 36,
    this.width,
    this.padding,
    this.trailing,
    this.pill = false,
    this.fontSize = 14,
    this.destructive = false,
    this.active = false,
    this.activeBorderColor,
    this.activeBackgroundColor,
    this.focusNode,
    this.loading = false,
    this.tooltip,
  });
  final Widget child;
  final VoidCallback? onPressed;
  final IconData? icon;
  final bool primary, outline, active;
  final Color? activeBorderColor, activeBackgroundColor;
  final double height;
  final double? width;
  final EdgeInsets? padding;
  final Widget? trailing;
  final bool pill, destructive;
  final double fontSize;
  final FocusNode? focusNode;
  final bool loading;
  final String? tooltip;
  @override
  Widget build(BuildContext context) {
    final tokens = DshTokens.of(context);
    final enabled = onPressed != null && !loading;
    final actualHeight = tokens.controlHeight(
      context,
      minimum: height,
      primary: primary,
      fontSize: fontSize,
    );
    final foreground = destructive
        ? tokens.error.foreground
        : primary
        ? tokens.onAccent
        : tokens.text;
    final button = ShadButton.raw(
      variant: primary
          ? ShadButtonVariant.primary
          : outline
          ? ShadButtonVariant.outline
          : ShadButtonVariant.ghost,
      onPressed: enabled
          ? () {
              Tooltip.dismissAllToolTips();
              onPressed!();
            }
          : null,
      enabled: enabled,
      focusNode: focusNode,
      cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
      backgroundColor: active
          ? activeBackgroundColor ?? tokens.selected
          : primary
          ? tokens.accent
          : null,
      foregroundColor: foreground,
      hoverBackgroundColor: active
          ? activeBackgroundColor ?? tokens.selected
          : primary
          ? tokens.accent.withValues(alpha: .9)
          : tokens.hover,
      hoverForegroundColor: foreground,
      height: actualHeight,
      width: width,
      padding: padding ?? const EdgeInsets.symmetric(horizontal: 12),
      decoration: active
          ? ShadDecoration(
              border: ShadBorder.all(
                color: activeBorderColor ?? DshColors(context).blue,
                width: outline ? 1 : 0,
                radius: BorderRadius.circular(tokens.radiusControl),
              ),
            )
          : pill
          ? ShadDecoration(
              border: ShadBorder.all(
                radius: BorderRadius.circular(actualHeight / 2),
                color: destructive
                    ? Theme.of(context).colorScheme.error
                    : DshColors(context).border,
                width: outline ? 1 : 0,
              ),
            )
          : null,
      leading: loading
          ? DshMotion.disabled(context)
                ? DshGlyph(
                    DshIcons.loaderCircle.data,
                    size: tokens.iconSize,
                    color: foreground,
                  )
                : SizedBox(
                    width: tokens.iconSize,
                    height: tokens.iconSize,
                    child: CircularProgressIndicator(
                      strokeWidth: 2,
                      color: foreground,
                    ),
                  )
          : icon == null
          ? null
          : DshGlyph(icon, size: tokens.iconSize),
      trailing: trailing,
      textStyle: DshTypography.body.copyWith(
        fontSize: fontSize,
        fontFamily: DshTypography.family,
        fontFamilyFallback: DshTypography.fallback,
        height: 22 / fontSize,
        color: foreground,
      ),
      child: child,
    );
    return tooltip == null
        ? button
        : DshTooltip(message: tooltip!, child: button);
  }
}

class DshIcon extends StatelessWidget {
  const DshIcon(
    this.icon, {
    super.key,
    required this.label,
    this.onPressed,
    this.active = false,
    this.size = 36,
    this.color,
    this.asset,
    this.glyphSize = 16,
    this.focusNode,
    this.shortcut,
  });
  final IconData icon;
  final String label;
  final VoidCallback? onPressed;
  final bool active;
  final double size;
  final double glyphSize;
  final String? asset;
  final Color? color;
  final FocusNode? focusNode;
  final String? shortcut;
  @override
  Widget build(BuildContext context) => DshTooltip(
    message: shortcut == null ? label : '$label ($shortcut)',
    excludeFromSemantics: true,
    child: Semantics(
      label: label,
      button: true,
      enabled: onPressed != null,
      child: ShadButton.ghost(
        onPressed: onPressed == null
            ? null
            : () {
                Tooltip.dismissAllToolTips();
                onPressed!();
              },
        enabled: onPressed != null,
        focusNode: focusNode,
        width: size < DshTokens.of(context).controlMinimum
            ? DshTokens.of(context).controlMinimum
            : size,
        height: size < DshTokens.of(context).controlMinimum
            ? DshTokens.of(context).controlMinimum
            : size,
        padding: EdgeInsets.zero,
        backgroundColor: active ? DshColors(context).selected : null,
        hoverBackgroundColor: active
            ? DshColors(context).selected
            : DshColors(context).hover,
        child: DshGlyph(
          icon,
          asset: asset,
          size: glyphSize,
          color:
              color ??
              (active ? DshColors(context).text : DshColors(context).muted),
        ),
      ),
    ),
  );
}

class DshField extends StatelessWidget {
  const DshField({
    super.key,
    this.controller,
    this.hint,
    this.onChanged,
    this.secret = false,
    this.enabled = true,
    this.maxLines = 1,
    this.autofocus = false,
    this.prefix,
    this.focusNode,
    this.onSubmitted,
  });
  final TextEditingController? controller;
  final String? hint;
  final ValueChanged<String>? onChanged, onSubmitted;
  final bool secret, autofocus, enabled;
  final int maxLines;
  final IconData? prefix;
  final FocusNode? focusNode;
  @override
  Widget build(BuildContext context) => TextField(
    controller: controller,
    enabled: enabled,
    focusNode: focusNode,
    onChanged: onChanged,
    onSubmitted: onSubmitted,
    obscureText: secret,
    autofocus: autofocus,
    maxLines: maxLines,
    style: DshTypography.body.copyWith(color: DshColors(context).text),
    textAlignVertical: TextAlignVertical.center,
    decoration: InputDecoration(
      hintText: hint,
      hintStyle: DshTypography.body.copyWith(color: DshColors(context).muted),
      prefixIcon: prefix == null ? null : DshGlyph(prefix, size: 16),
      contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
      constraints: maxLines == 1
          ? BoxConstraints(
              minHeight: DshTokens.of(context).controlHeight(context),
            )
          : null,
      prefixIconConstraints: BoxConstraints.tightFor(
        width: DshTokens.of(context).controlMinimum,
        height: DshTokens.of(context).controlHeight(context),
      ),
      isDense: true,
      filled: true,
      fillColor: DshColors(context).base,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(
          DshTokens.of(context).radiusControl,
        ),
        borderSide: BorderSide(color: DshColors(context).border),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(
          DshTokens.of(context).radiusControl,
        ),
        borderSide: BorderSide(color: DshColors(context).border),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(
          DshTokens.of(context).radiusControl,
        ),
        borderSide: BorderSide(color: DshColors(context).focus, width: 2),
      ),
    ),
  );
}

class DshSwitch extends StatelessWidget {
  const DshSwitch({super.key, required this.value, required this.onChanged});
  final bool value;
  final ValueChanged<bool>? onChanged;
  @override
  Widget build(BuildContext context) => MergeSemantics(
    child: MouseRegion(
      cursor: onChanged == null
          ? SystemMouseCursors.basic
          : SystemMouseCursors.click,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: onChanged == null ? null : () => onChanged!(!value),
        child: SizedBox.square(
          dimension: DshTokens.of(context).controlMinimum,
          child: Center(
            child: ShadSwitch(
              value: value,
              enabled: onChanged != null,
              onChanged: onChanged,
              width: 36,
              height: 22,
              duration: DshMotion.duration(context, DshMotion.quick),
              margin: 3,
              padding: EdgeInsets.zero,
              thumbColor: Colors.white,
              checkedTrackColor: DshColors(context).blue,
              uncheckedTrackColor: DshTokens.of(context).switchTrack,
            ),
          ),
        ),
      ),
    ),
  );
}

class DshEmpty extends StatelessWidget {
  const DshEmpty(this.text, {super.key, this.icon});
  final String text;
  final IconData? icon;
  @override
  Widget build(BuildContext context) => Center(
    child: Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          DshGlyph(
            icon ?? DshIcons.inbox.data,
            size: 28,
            color: DshColors(context).muted,
          ),
          const SizedBox(height: 12),
          Text(
            text,
            textAlign: TextAlign.center,
            style: DshTypography.body.copyWith(color: DshColors(context).muted),
          ),
        ],
      ),
    ),
  );
}

Future<bool> confirmAction(
  BuildContext context,
  String title,
  String message, {
  String action = '确定',
}) async =>
    await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title, style: DshTypography.sectionTitle),
        content: Text(message),
        actions: [
          DshButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text(DshZh.cancel),
          ),
          DshButton(
            primary: true,
            onPressed: () => Navigator.pop(context, true),
            child: Text(action),
          ),
        ],
      ),
    ) ??
    false;
Future<String?> editTextDialog(
  BuildContext context,
  String title,
  String value, {
  Future<void> Function(String)? onSubmit,
  Future<String> Function()? onRecover,
  bool Function(Object)? recoveryRequired,
}) => showDialog<String>(
  context: context,
  barrierDismissible: onSubmit == null,
  builder: (context) => _EditDialog(
    title: title,
    value: value,
    onSubmit: onSubmit,
    onRecover: onRecover,
    recoveryRequired: recoveryRequired,
  ),
);

class _EditDialog extends StatefulWidget {
  const _EditDialog({
    required this.title,
    required this.value,
    this.onSubmit,
    this.onRecover,
    this.recoveryRequired,
  });
  final String title, value;
  final Future<void> Function(String)? onSubmit;
  final Future<String> Function()? onRecover;
  final bool Function(Object)? recoveryRequired;
  @override
  State<_EditDialog> createState() => _EditDialogState();
}

class _EditDialogState extends State<_EditDialog> {
  late final input = TextEditingController(text: widget.value);
  bool saving = false, mustRecover = false;
  Object? error;
  String? notice;

  Future<void> save() async {
    if (saving || mustRecover) return;
    setState(() {
      saving = true;
      error = null;
    });
    try {
      if (widget.onSubmit != null) await widget.onSubmit!(input.text);
      if (mounted) Navigator.pop(context, input.text);
    } catch (failure) {
      if (mounted) {
        setState(() {
          error = failure;
          mustRecover =
              widget.onRecover != null &&
              (widget.recoveryRequired?.call(failure) ?? true);
        });
      }
    } finally {
      if (mounted) setState(() => saving = false);
    }
  }

  Future<void> recover() async {
    if (saving || widget.onRecover == null) return;
    setState(() => saving = true);
    try {
      final latest = await widget.onRecover!();
      if (mounted) {
        setState(() {
          notice = latest;
          error = null;
          mustRecover = false;
        });
      }
    } catch (failure) {
      if (mounted) setState(() => error = failure);
    } finally {
      if (mounted) setState(() => saving = false);
    }
  }

  @override
  void dispose() {
    input.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !saving,
    child: AlertDialog(
      title: Text(widget.title, style: DshTypography.sectionTitle),
      content: SizedBox(
        width: 420,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            DshField(controller: input, autofocus: true, enabled: !saving),
            if (error != null)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: DshErrorView(
                  error: error!,
                  operation: DshZh.save,
                  onRetry: saving || mustRecover ? null : save,
                ),
              ),
            if (notice != null)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Text(notice!),
              ),
          ],
        ),
      ),
      actions: [
        DshButton(
          onPressed: saving ? null : () => Navigator.pop(context),
          child: const Text(DshZh.cancel),
        ),
        if (mustRecover)
          DshButton(
            onPressed: saving ? null : recover,
            child: const Text('读取最新状态'),
          ),
        DshButton(
          primary: true,
          loading: saving,
          onPressed: saving || mustRecover ? null : save,
          child: Text(saving ? DshZh.saving : DshZh.save),
        ),
      ],
    ),
  );
}
