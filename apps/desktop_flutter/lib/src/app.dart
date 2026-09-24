import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller.dart';
import '../features/shell.dart';
import '../design/typography.dart';
export '../features/shell.dart';

class DesktopApp extends StatelessWidget {
  const DesktopApp({super.key, required this.controller});
  final DesktopController controller;
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller,
    builder: (_, _) => ShadApp(
      title: 'DeepSeek Harness',
      debugShowCheckedModeBanner: false,
      themeMode: controller.preferences.dark ? ThemeMode.dark : ThemeMode.light,
      theme: ShadThemeData(
        brightness: Brightness.light,
        colorScheme: const ShadZincColorScheme.light().copyWith(
          foreground: const Color(0xff0f1115),
          mutedForeground: const Color(0xff81858c),
          border: const Color(0x1a000000),
          accent: const Color(0xfff1f3f5),
          ring: const Color(0xff4176e6),
        ),
        radius: BorderRadius.circular(8),
        textTheme: DshTypography.shad,
      ),
      darkTheme: ShadThemeData(
        brightness: Brightness.dark,
        colorScheme: const ShadZincColorScheme.dark().copyWith(
          background: const Color(0xff151517),
          foreground: const Color(0xfff9fafb),
          card: const Color(0xff1b1b1c),
          popover: const Color(0xff353638),
          mutedForeground: const Color(0xffadb2b8),
          border: const Color(0x1fffffff),
          accent: const Color(0xff43454a),
          ring: const Color(0xff679efe),
        ),
        radius: BorderRadius.circular(8),
        textTheme: DshTypography.shad,
      ),
      materialThemeBuilder: (context, theme) => theme.copyWith(
        scaffoldBackgroundColor: theme.brightness == Brightness.dark
            ? const Color(0xff151517)
            : Colors.white,
        textTheme: DshTypography.material(theme.textTheme).apply(
          bodyColor: theme.colorScheme.onSurface,
          displayColor: theme.colorScheme.onSurface,
        ),
        dividerColor: theme.brightness == Brightness.dark
            ? const Color(0x1fffffff)
            : const Color(0x1a000000),
        dialogTheme: DialogThemeData(
          backgroundColor: theme.brightness == Brightness.dark
              ? const Color(0xff2c2c2e)
              : Colors.white,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(24),
          ),
        ),
        popupMenuTheme: PopupMenuThemeData(
          color: theme.brightness == Brightness.dark
              ? const Color(0xff353638)
              : Colors.white,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
          ),
        ),
      ),
      home: Workbench(controller: controller),
    ),
  );
}
