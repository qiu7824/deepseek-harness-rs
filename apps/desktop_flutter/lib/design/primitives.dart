import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'icon_assets.dart';
import 'typography.dart';

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
  DshColors(BuildContext context)
    : dark = Theme.of(context).brightness == Brightness.dark;
  final bool dark;
  Color get base => dark ? const Color(0xff151517) : Colors.white;
  Color get sidebar => dark ? const Color(0xff1b1b1c) : const Color(0xfff9fafb);
  Color get layer => dark ? const Color(0xff353638) : const Color(0xfff5f6f7);
  Color get hover => dark ? const Color(0xff43454a) : const Color(0xffebeef2);
  Color get border => dark ? const Color(0x1fffffff) : const Color(0x1a000000);
  Color get text => dark ? const Color(0xfff9fafb) : const Color(0xff0f1115);
  Color get muted => dark ? const Color(0xffadb2b8) : const Color(0xff81858c);
  Color get blue => dark ? const Color(0xff679efe) : const Color(0xff4176e6);
  Color get bubble => dark ? const Color(0xff2c2c2e) : const Color(0xffedf3fe);
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
  @override
  Widget build(BuildContext context) => ShadButton.raw(
    variant: primary
        ? ShadButtonVariant.primary
        : outline
        ? ShadButtonVariant.outline
        : ShadButtonVariant.ghost,
    onPressed: onPressed,
    enabled: onPressed != null,
    backgroundColor: active
        ? activeBackgroundColor ?? DshColors(context).hover
        : null,
    foregroundColor: destructive ? Theme.of(context).colorScheme.error : null,
    height: height,
    width: width,
    padding: padding ?? const EdgeInsets.symmetric(horizontal: 12),
    decoration: active
        ? ShadDecoration(
            border: ShadBorder.all(
              color: activeBorderColor ?? DshColors(context).blue,
              width: outline ? 1 : 0,
              radius: BorderRadius.circular(8),
            ),
          )
        : pill
        ? ShadDecoration(
            border: ShadBorder.all(
              radius: BorderRadius.circular(height / 2),
              color: destructive
                  ? Theme.of(context).colorScheme.error
                  : DshColors(context).border,
              width: outline ? 1 : 0,
            ),
          )
        : null,
    leading: icon == null ? null : DshGlyph(icon, size: 16),
    trailing: trailing,
    textStyle: TextStyle(
      fontSize: fontSize,
      fontFamily: DshTypography.family,
      fontFamilyFallback: DshTypography.fallback,
      height: 22 / fontSize,
      color: destructive
          ? Theme.of(context).colorScheme.error
          : primary
          ? DshColors(context).base
          : DshColors(context).text,
    ),
    child: child,
  );
}

class DshIcon extends StatelessWidget {
  const DshIcon(
    this.icon, {
    super.key,
    required this.label,
    this.onPressed,
    this.active = false,
    this.size = 30,
    this.color,
    this.asset,
    this.glyphSize = 16,
  });
  final IconData icon;
  final String label;
  final VoidCallback? onPressed;
  final bool active;
  final double size;
  final double glyphSize;
  final String? asset;
  final Color? color;
  @override
  Widget build(BuildContext context) => Tooltip(
    message: label,
    waitDuration: const Duration(milliseconds: 500),
    child: Semantics(
      label: label,
      button: true,
      child: ShadButton.ghost(
        onPressed: onPressed,
        enabled: onPressed != null,
        width: size,
        height: size,
        padding: EdgeInsets.zero,
        backgroundColor: active ? DshColors(context).hover : null,
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
  });
  final TextEditingController? controller;
  final String? hint;
  final ValueChanged<String>? onChanged;
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
    obscureText: secret,
    autofocus: autofocus,
    maxLines: maxLines,
    style: DshTypography.body.copyWith(color: DshColors(context).text),
    textAlignVertical: TextAlignVertical.center,
    decoration: InputDecoration(
      hintText: hint,
      prefixIcon: prefix == null ? null : DshGlyph(prefix, size: 16),
      contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
      constraints: maxLines == 1
          ? const BoxConstraints.tightFor(height: 36)
          : null,
      prefixIconConstraints: const BoxConstraints.tightFor(
        width: 36,
        height: 36,
      ),
      isDense: true,
      filled: true,
      fillColor: DshColors(context).base,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: BorderSide(color: DshColors(context).border),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: BorderSide(color: DshColors(context).border),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(8),
        borderSide: BorderSide(color: DshColors(context).blue),
      ),
    ),
  );
}

class DshSwitch extends StatelessWidget {
  const DshSwitch({super.key, required this.value, required this.onChanged});
  final bool value;
  final ValueChanged<bool>? onChanged;
  @override
  Widget build(BuildContext context) => ShadSwitch(
    value: value,
    enabled: onChanged != null,
    onChanged: onChanged,
    width: 36,
    height: 22,
    margin: 3,
    padding: EdgeInsets.zero,
    thumbColor: Colors.white,
    checkedTrackColor: DshColors(context).blue,
    uncheckedTrackColor: DshColors(context).dark
        ? const Color(0xff55575d)
        : const Color(0xffc9cdd4),
  );
}

class DshEmpty extends StatelessWidget {
  const DshEmpty(this.text, {super.key, this.icon = LucideIcons.inbox});
  final String text;
  final IconData icon;
  @override
  Widget build(BuildContext context) => Center(
    child: Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          DshGlyph(icon, size: 28, color: DshColors(context).muted),
          const SizedBox(height: 12),
          Text(
            text,
            textAlign: TextAlign.center,
            style: TextStyle(fontSize: 14, color: DshColors(context).muted),
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
        title: Text(title, style: const TextStyle(fontSize: 17)),
        content: Text(message),
        actions: [
          DshButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('取消'),
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
  String? error, notice;

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
          error = failure.toString();
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
      if (mounted) setState(() => error = failure.toString());
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
      title: Text(widget.title, style: const TextStyle(fontSize: 17)),
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
                child: Text(
                  error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
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
          child: const Text('取消'),
        ),
        if (mustRecover)
          DshButton(
            onPressed: saving ? null : recover,
            child: const Text('读取最新状态'),
          ),
        DshButton(
          primary: true,
          onPressed: saving || mustRecover ? null : save,
          child: const Text('保存'),
        ),
      ],
    ),
  );
}
