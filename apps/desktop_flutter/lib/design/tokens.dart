import 'dart:math' as math;

import 'package:flutter/material.dart';

@immutable
class DshStatusColors {
  const DshStatusColors(this.foreground, this.background, this.border);

  final Color foreground;
  final Color background;
  final Color border;

  static DshStatusColors lerp(DshStatusColors a, DshStatusColors b, double t) =>
      DshStatusColors(
        Color.lerp(a.foreground, b.foreground, t)!,
        Color.lerp(a.background, b.background, t)!,
        Color.lerp(a.border, b.border, t)!,
      );
}

/// The shared source for Material, Shad and desktop component styling.
@immutable
class DshTokens extends ThemeExtension<DshTokens> {
  const DshTokens({
    required this.brightness,
    required this.base,
    required this.sidebar,
    required this.layer,
    required this.hover,
    required this.selected,
    required this.border,
    required this.text,
    required this.muted,
    required this.accent,
    required this.onAccent,
    required this.bubble,
    required this.focus,
    required this.disabled,
    required this.switchTrack,
    required this.success,
    required this.warning,
    required this.error,
    required this.info,
    this.touchMode = false,
  });

  static const light = DshTokens(
    brightness: Brightness.light,
    base: Color(0xffffffff),
    sidebar: Color(0xfff9fafb),
    layer: Color(0xffffffff),
    hover: Color.fromRGBO(38, 49, 72, .06),
    selected: Color.fromRGBO(38, 49, 72, .1),
    border: Color.fromRGBO(0, 0, 0, .1),
    text: Color(0xff0f1115),
    muted: Color(0xff61666b),
    accent: Color(0xff315fbd),
    onAccent: Color(0xffffffff),
    bubble: Color(0xffedf3fe),
    focus: Color(0xff315fbd),
    disabled: Color(0xff9096a0),
    switchTrack: Color(0xffe0e0e0),
    success: DshStatusColors(
      Color(0xff20643b),
      Color(0xffedf8f0),
      Color(0xff9ecfaf),
    ),
    warning: DshStatusColors(
      Color(0xff825600),
      Color(0xfffff7df),
      Color(0xffd9bb63),
    ),
    error: DshStatusColors(
      Color(0xffb42335),
      Color(0xfffff0f1),
      Color(0xffeab0b8),
    ),
    info: DshStatusColors(
      Color(0xff315fbd),
      Color(0xffedf3fe),
      Color(0xffa9c3f4),
    ),
  );

  static const dark = DshTokens(
    brightness: Brightness.dark,
    base: Color(0xff151517),
    sidebar: Color(0xff1b1b1c),
    layer: Color(0xff232324),
    hover: Color.fromRGBO(255, 255, 255, .08),
    selected: Color.fromRGBO(255, 255, 255, .14),
    border: Color.fromRGBO(255, 255, 255, .12),
    text: Color(0xfff9fafb),
    muted: Color(0xffcfd3d6),
    accent: Color(0xff91b7ff),
    onAccent: Color(0xff12203a),
    bubble: Color(0xff26344e),
    focus: Color(0xffa9c8ff),
    disabled: Color(0xff777e89),
    switchTrack: Color(0xff666c76),
    success: DshStatusColors(
      Color(0xff91dcad),
      Color(0xff193324),
      Color(0xff3b7951),
    ),
    warning: DshStatusColors(
      Color(0xfff3d47e),
      Color(0xff392e14),
      Color(0xff8b7137),
    ),
    error: DshStatusColors(
      Color(0xffffa9b5),
      Color(0xff401d26),
      Color(0xffa75566),
    ),
    info: DshStatusColors(
      Color(0xffa9c8ff),
      Color(0xff1e2f4c),
      Color(0xff4b6d9f),
    ),
  );

  static DshTokens of(BuildContext context) {
    final theme = Theme.of(context);
    return theme.extension<DshTokens>() ??
        (theme.brightness == Brightness.dark ? dark : light);
  }

  final Brightness brightness;
  final Color base, sidebar, layer, hover, selected, border, text, muted;
  final Color accent, onAccent, bubble, focus, disabled, switchTrack;
  final DshStatusColors success, warning, error, info;
  final bool touchMode;

  bool get isDark => brightness == Brightness.dark;
  double get spaceXs => 4;
  double get spaceSm => 8;
  double get spaceMd => 12;
  double get spaceLg => 16;
  double get spaceXl => 24;
  double get space2xl => 32;
  double get radiusControl => 8;
  double get radiusCard => 12;
  double get radiusDialog => 16;
  double get controlMinimum => touchMode ? 48 : 36;
  double get primaryMinimum => touchMode ? 48 : 40;
  double get iconSize => 16;
  double get largeIconSize => 20;

  /// The requested height is a minimum, never a clip for scaled text.
  double controlHeight(
    BuildContext context, {
    double minimum = 36,
    double fontSize = 14,
    double lineHeight = 22,
    double verticalPadding = 14,
    bool primary = false,
  }) {
    final scaledLineHeight =
        MediaQuery.textScalerOf(context).scale(fontSize) *
        lineHeight /
        fontSize;
    return math.max(
      math.max(minimum, primary ? primaryMinimum : controlMinimum),
      scaledLineHeight + verticalPadding,
    );
  }

  ColorScheme get colorScheme =>
      ColorScheme.fromSeed(
        seedColor: accent,
        brightness: brightness,
        surface: base,
      ).copyWith(
        primary: accent,
        onPrimary: onAccent,
        secondary: accent,
        onSecondary: onAccent,
        surfaceContainer: layer,
        surfaceContainerLow: sidebar,
        onSurface: text,
        onSurfaceVariant: muted,
        outline: border,
        error: error.foreground,
        errorContainer: error.background,
        onErrorContainer: error.foreground,
      );

  @override
  DshTokens copyWith({
    Brightness? brightness,
    Color? base,
    Color? sidebar,
    Color? layer,
    Color? hover,
    Color? selected,
    Color? border,
    Color? text,
    Color? muted,
    Color? accent,
    Color? onAccent,
    Color? bubble,
    Color? focus,
    Color? disabled,
    Color? switchTrack,
    DshStatusColors? success,
    DshStatusColors? warning,
    DshStatusColors? error,
    DshStatusColors? info,
    bool? touchMode,
  }) => DshTokens(
    brightness: brightness ?? this.brightness,
    base: base ?? this.base,
    sidebar: sidebar ?? this.sidebar,
    layer: layer ?? this.layer,
    hover: hover ?? this.hover,
    selected: selected ?? this.selected,
    border: border ?? this.border,
    text: text ?? this.text,
    muted: muted ?? this.muted,
    accent: accent ?? this.accent,
    onAccent: onAccent ?? this.onAccent,
    bubble: bubble ?? this.bubble,
    focus: focus ?? this.focus,
    disabled: disabled ?? this.disabled,
    switchTrack: switchTrack ?? this.switchTrack,
    success: success ?? this.success,
    warning: warning ?? this.warning,
    error: error ?? this.error,
    info: info ?? this.info,
    touchMode: touchMode ?? this.touchMode,
  );

  @override
  DshTokens lerp(covariant DshTokens? other, double t) {
    if (other == null) return this;
    return DshTokens(
      brightness: t < .5 ? brightness : other.brightness,
      base: Color.lerp(base, other.base, t)!,
      sidebar: Color.lerp(sidebar, other.sidebar, t)!,
      layer: Color.lerp(layer, other.layer, t)!,
      hover: Color.lerp(hover, other.hover, t)!,
      selected: Color.lerp(selected, other.selected, t)!,
      border: Color.lerp(border, other.border, t)!,
      text: Color.lerp(text, other.text, t)!,
      muted: Color.lerp(muted, other.muted, t)!,
      accent: Color.lerp(accent, other.accent, t)!,
      onAccent: Color.lerp(onAccent, other.onAccent, t)!,
      bubble: Color.lerp(bubble, other.bubble, t)!,
      focus: Color.lerp(focus, other.focus, t)!,
      disabled: Color.lerp(disabled, other.disabled, t)!,
      switchTrack: Color.lerp(switchTrack, other.switchTrack, t)!,
      success: DshStatusColors.lerp(success, other.success, t),
      warning: DshStatusColors.lerp(warning, other.warning, t),
      error: DshStatusColors.lerp(error, other.error, t),
      info: DshStatusColors.lerp(info, other.info, t),
      touchMode: t < .5 ? touchMode : other.touchMode,
    );
  }
}
