import 'l10n/runtime_zh.dart';

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'src/controller.dart';
import 'src/preferences.dart';
import 'src/app.dart';
import 'src/desktop_diagnostics.dart';
import 'src/desktop_updates.dart';
import 'src/window_theme.dart';
import 'src/window_lifecycle.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await windowManager.ensureInitialized();
  await windowManager.setMinimumSize(const Size(720, 520));
  PaintingBinding.instance.imageCache.maximumSizeBytes = 48 * 1024 * 1024;
  PaintingBinding.instance.imageCache.maximumSize = 128;
  DesktopPreferences preferences;
  String? startupError;
  try {
    preferences = await DesktopPreferences.load();
  } catch (e) {
    preferences = DesktopPreferences();
    startupError = DshRuntimeZh.preferencesReadFailed(error: e);
  }
  final controller = DesktopController(preferences);
  unawaited(
    installWindowLifecycle(
      controller,
      onError: (message) {
        startupError = [
          if (startupError != null) startupError!,
          message,
        ].join('\n');
      },
    ),
  );
  await WindowThemeBinding(controller).start();
  runApp(DesktopApp(controller: controller));
  if (DesktopUpdateController.installationRoot() != null) {
    unawaited(DesktopUpdateController.instance.check());
  }
  await controller.initialize();
  await DesktopDiagnostics.start(
    controller,
    Platform.environment['DSH_DESKTOP_DIAGNOSTICS'],
  );
  if (startupError != null) {
    controller.error = startupError;
    controller.emit();
  }
}
