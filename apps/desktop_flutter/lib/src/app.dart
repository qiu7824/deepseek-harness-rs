import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller.dart';
import '../features/shell.dart';
import '../design/tokens.dart';
import '../design/typography.dart';
import '../design/primitives.dart' show DshTooltip;
export '../features/shell.dart';

class DesktopApp extends StatelessWidget {
  const DesktopApp({super.key, required this.controller});
  final DesktopController controller;

  ShadThemeData _theme(DshTokens tokens) => ShadThemeData(
    brightness: tokens.brightness,
    colorScheme:
        (tokens.isDark
                ? const ShadZincColorScheme.dark()
                : const ShadZincColorScheme.light())
            .copyWith(
              background: tokens.base,
              foreground: tokens.text,
              card: tokens.sidebar,
              cardForeground: tokens.text,
              popover: tokens.layer,
              popoverForeground: tokens.text,
              primary: tokens.accent,
              primaryForeground: tokens.onAccent,
              secondary: tokens.layer,
              secondaryForeground: tokens.text,
              muted: tokens.layer,
              mutedForeground: tokens.muted,
              accent: tokens.hover,
              accentForeground: tokens.text,
              destructive: tokens.error.foreground,
              border: tokens.border,
              input: tokens.border,
              ring: tokens.focus,
            ),
    radius: BorderRadius.circular(tokens.radiusControl),
    textTheme: DshTypography.shad,
  );

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller.themeChanges,
    builder: (_, _) => ShadApp(
      title: 'DeepSeek Harness',
      debugShowCheckedModeBanner: false,
      themeMode: controller.preferences.dark ? ThemeMode.dark : ThemeMode.light,
      theme: _theme(DshTokens.light),
      darkTheme: _theme(DshTokens.dark),
      materialThemeBuilder: (context, theme) {
        final tokens = theme.brightness == Brightness.dark
            ? DshTokens.dark
            : DshTokens.light;
        return theme.copyWith(
          extensions: [
            ...theme.extensions.values.where((e) => e is! DshTokens),
            tokens,
          ],
          colorScheme: tokens.colorScheme,
          scaffoldBackgroundColor: tokens.base,
          textTheme: DshTypography.material(theme.textTheme)
              .apply(bodyColor: tokens.text, displayColor: tokens.text),
          dividerColor: tokens.border,
          tooltipTheme: DshTooltip.theme(tokens),
          dialogTheme: DialogThemeData(
            backgroundColor: tokens.base,
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(tokens.radiusDialog),
            ),
          ),
          popupMenuTheme: PopupMenuThemeData(
            color: tokens.layer,
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(tokens.radiusCard),
            ),
          ),
        );
      },
      home: Workbench(controller: controller),
    ),
  );
}
