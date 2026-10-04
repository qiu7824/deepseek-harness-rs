import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

abstract final class DshTypography {
  static const sizeCaption = 12.0;
  static const sizeAuxiliary = 13.0;
  static const sizeBody = 14.0;
  static const sizeConversation = 15.0;
  static const sizeComposer = 16.0;
  static const sizeSectionTitle = 18.0;
  static const sizeTitle = 22.0;
  static const sizeHeadline = 26.0;

  static String familyFor(TargetPlatform platform) => switch (platform) {
    TargetPlatform.windows => 'Segoe UI',
    TargetPlatform.macOS || TargetPlatform.iOS => '.AppleSystemUIFont',
    TargetPlatform.linux => 'Noto Sans',
    _ => 'Roboto',
  };

  static List<String> fallbackFor(TargetPlatform platform) =>
      switch (platform) {
        TargetPlatform.windows => const [
          'Microsoft YaHei',
          'Microsoft YaHei UI',
          'Segoe UI Symbol',
          'Segoe UI Emoji',
          'Noto Sans CJK SC',
          'sans-serif',
        ],
        TargetPlatform.macOS || TargetPlatform.iOS => const [
          'PingFang SC',
          'Hiragino Sans GB',
          'Apple Color Emoji',
          'sans-serif',
        ],
        _ => const [
          'Noto Sans CJK SC',
          'Noto Sans SC',
          'WenQuanYi Micro Hei',
          'Noto Color Emoji',
          'sans-serif',
        ],
      };

  static String monospaceFor(TargetPlatform platform) => switch (platform) {
    TargetPlatform.windows => 'Consolas',
    TargetPlatform.macOS || TargetPlatform.iOS => 'SF Mono',
    _ => 'DejaVu Sans Mono',
  };

  static List<String> monospaceFallbackFor(TargetPlatform platform) => [
    if (platform == TargetPlatform.windows) 'Cascadia Mono',
    if (platform == TargetPlatform.macOS || platform == TargetPlatform.iOS)
      'Menlo',
    'DejaVu Sans Mono',
    'monospace',
    ...fallbackFor(platform),
  ];

  static String get family => familyFor(defaultTargetPlatform);
  static List<String> get fallback => fallbackFor(defaultTargetPlatform);
  static String get monospaceFamily => monospaceFor(defaultTargetPlatform);
  static List<String> get monospaceFallback =>
      monospaceFallbackFor(defaultTargetPlatform);

  static TextStyle _role(
    double size,
    double lineHeight, {
    FontWeight weight = FontWeight.w400,
  }) => TextStyle(
    fontFamily: family,
    fontFamilyFallback: fallback,
    fontSize: size,
    fontWeight: weight,
    height: lineHeight / size,
    letterSpacing: 0,
  );

  static TextStyle get caption => _role(sizeCaption, 18);
  static TextStyle get auxiliary => _role(sizeAuxiliary, 20);
  static TextStyle get body => _role(sizeBody, 22);
  static TextStyle get conversation => _role(sizeConversation, 24);
  static TextStyle get composer => _role(sizeComposer, 24);
  static TextStyle get sectionTitle =>
      _role(sizeSectionTitle, 26, weight: FontWeight.w500);
  static TextStyle get title => _role(sizeTitle, 30, weight: FontWeight.w500);
  static TextStyle get headline =>
      _role(sizeHeadline, 32, weight: FontWeight.w500);
  static TextStyle get code => _role(sizeAuxiliary, 20).copyWith(
    fontFamily: monospaceFamily,
    fontFamilyFallback: monospaceFallback,
  );
  static TextStyle get numeric =>
      body.copyWith(fontFeatures: const [FontFeature.tabularFigures()]);

  static ShadTextTheme get shad => ShadTextTheme(
    family: family,
    p: body,
    list: body,
    table: body,
    blockquote: body,
    small: caption,
    muted: auxiliary,
    large: composer.copyWith(fontWeight: FontWeight.w500),
    lead: composer,
    h1Large: headline,
    h1: headline,
    h2: title,
    h3: sectionTitle,
    h4: composer.copyWith(fontWeight: FontWeight.w500),
  );

  static TextTheme material(TextTheme source) => source.copyWith(
    displayLarge: headline,
    displayMedium: headline,
    displaySmall: headline,
    headlineLarge: headline,
    headlineMedium: title,
    headlineSmall: title,
    titleLarge: sectionTitle,
    titleMedium: composer.copyWith(fontWeight: FontWeight.w500),
    titleSmall: body.copyWith(fontWeight: FontWeight.w500),
    bodyLarge: conversation,
    bodyMedium: body,
    bodySmall: auxiliary,
    labelLarge: body,
    labelMedium: auxiliary,
    labelSmall: caption,
  );
}
