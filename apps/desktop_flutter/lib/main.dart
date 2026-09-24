import 'dart:io';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'src/controller.dart';
import 'src/preferences.dart';
import 'src/app.dart';
import 'src/desktop_diagnostics.dart';
import 'src/window_theme.dart';

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
    startupError = '无法读取桌面设置：$e';
  }
  final controller = DesktopController(preferences);
  await WindowThemeBinding(controller).start();
  runApp(DesktopApp(controller: controller));
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
