import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'controller.dart';

class WindowThemeBinding {
  WindowThemeBinding(this.controller);
  static const channel = MethodChannel('dsh/window-theme');
  final DesktopController controller;
  bool? applied;
  bool disposed = false, applying = false;

  Future<void> start() async {
    controller.addListener(changed);
    await apply();
  }

  void changed() {
    unawaited(apply());
  }

  Future<void> apply() async {
    if (disposed ||
        applying ||
        defaultTargetPlatform != TargetPlatform.windows) {
      return;
    }
    applying = true;
    try {
      while (!disposed && applied != controller.preferences.dark) {
        final desired = controller.preferences.dark;
        await channel.invokeMethod<void>('setDarkMode', desired);
        applied = desired;
      }
    } on MissingPluginException {
      // Test embedders and older runners may not provide native window chrome.
    } finally {
      applying = false;
    }
  }

  void dispose() {
    disposed = true;
    controller.removeListener(changed);
  }
}
