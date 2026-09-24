import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

abstract final class DshTypography {
  static const family = 'Segoe UI';
  static const fallback = [
    'Microsoft YaHei',
    'Microsoft YaHei UI',
    'Segoe UI Symbol',
    'Segoe UI Emoji',
    'Noto Sans CJK SC',
    'Arial',
  ];
  static const body = TextStyle(
    fontFamily: family,
    fontFamilyFallback: fallback,
    fontSize: 14,
    fontWeight: FontWeight.w400,
    height: 22 / 14,
    letterSpacing: 0,
  );
  static const caption = TextStyle(
    fontFamily: family,
    fontFamilyFallback: fallback,
    fontSize: 12,
    fontWeight: FontWeight.w400,
    height: 18 / 12,
    letterSpacing: 0,
  );
  static const composer = TextStyle(
    fontFamily: family,
    fontFamilyFallback: fallback,
    fontSize: 16,
    fontWeight: FontWeight.w400,
    height: 24 / 16,
    letterSpacing: 0,
  );
  static const headline = TextStyle(
    fontFamily: family,
    fontFamilyFallback: fallback,
    fontSize: 26,
    fontWeight: FontWeight.w500,
    height: 32 / 26,
    letterSpacing: 0,
  );
  static const code = TextStyle(
    fontFamily: 'Consolas',
    fontFamilyFallback: fallback,
    fontSize: 12,
    fontWeight: FontWeight.w400,
    height: 18 / 12,
    letterSpacing: 0,
  );
  static ShadTextTheme get shad => ShadTextTheme(
    family: family,
    p: body,
    list: body,
    table: body,
    blockquote: body,
    small: body,
    muted: caption,
    large: body.copyWith(fontSize: 16, fontWeight: FontWeight.w500),
    lead: body.copyWith(fontSize: 16),
    h1Large: headline,
    h1: headline,
    h2: body.copyWith(
      fontSize: 22,
      height: 30 / 22,
      fontWeight: FontWeight.w500,
    ),
    h3: body.copyWith(
      fontSize: 18,
      height: 26 / 18,
      fontWeight: FontWeight.w500,
    ),
    h4: body.copyWith(
      fontSize: 16,
      height: 24 / 16,
      fontWeight: FontWeight.w500,
    ),
  );
  static TextTheme material(TextTheme source) => source.copyWith(
    displayLarge: headline,
    displayMedium: headline,
    displaySmall: headline,
    headlineLarge: headline,
    headlineMedium: headline,
    headlineSmall: headline,
    titleLarge: body.copyWith(fontSize: 18, fontWeight: FontWeight.w500),
    titleMedium: body.copyWith(fontSize: 16, fontWeight: FontWeight.w500),
    titleSmall: body.copyWith(fontWeight: FontWeight.w500),
    bodyLarge: body,
    bodyMedium: body,
    bodySmall: caption,
    labelLarge: body,
    labelMedium: body,
    labelSmall: caption,
  );
}
