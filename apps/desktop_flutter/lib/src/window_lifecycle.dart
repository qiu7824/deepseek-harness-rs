import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:window_manager/window_manager.dart';

import '../design/error.dart';
import '../l10n/zh.dart';
import 'controller.dart';

/// Installs close interception without making a failed native plugin call a
/// fatal application startup error. The listener retains its successful binding.
Future<void> installWindowLifecycle(
  DesktopController controller, {
  void Function(String message)? onError,
}) async {
  final binding = WindowLifecycleBinding(controller);
  try {
    await binding.start();
  } catch (error) {
    binding.dispose();
    final message = DshWindowZh.protectionFailed(
      detail: DshError.describe(error).message,
      code: error is PlatformException ? error.code : '${error.runtimeType}',
    );
    onError?.call(message);
    controller.error = message;
    controller.emit();
  }
}

/// Saves the final local state before closing the desktop client. The detached
/// Host and its tasks are intentionally outside this binding's ownership.
class WindowLifecycleBinding with WindowListener {
  WindowLifecycleBinding(this.controller);

  final DesktopController controller;
  Future<void>? _starting, _closeAttempt;
  bool _installed = false, _disposed = false, _closed = false;

  Future<void> start() => _starting ??= _install();

  Future<void> _install() async {
    if (_disposed) return;
    windowManager.addListener(this);
    _installed = true;
    try {
      await windowManager.setPreventClose(true);
      if (_disposed) await windowManager.setPreventClose(false);
    } catch (_) {
      windowManager.removeListener(this);
      _installed = false;
      rethrow;
    }
  }

  @override
  void onWindowClose() {
    unawaited(requestClose());
  }

  /// Repeated native close events share one attempt, including while the
  /// native destroy call is still pending. A failed attempt can be retried.
  Future<void> requestClose() {
    final existing = _closeAttempt;
    if (existing != null) return existing;
    if (_disposed || _closed) return Future.value();
    final completion = Completer<void>();
    _closeAttempt = completion.future;
    unawaited(
      _saveAndClose().whenComplete(() {
        _closeAttempt = null;
        completion.complete();
      }),
    );
    return completion.future;
  }

  String _draftState() => jsonEncode(controller.preferences.drafts);

  Future<void> _saveAndClose() async {
    var saved = false;
    try {
      while (!_disposed) {
        final before = _draftState();
        // save() joins the preferences write queue and snapshots at write time;
        // it does not wait for the composer's 500 ms debounce timer.
        await controller.preferences.save();
        if (_disposed) return;
        // Background transcript/reading-position changes must not postpone
        // closing a client whose Host is intentionally continuing to run.
        if (before == _draftState()) break;
      }
      if (_disposed) return;
      saved = true;
      // Hand the close back to Windows so the window is destroyed inside the
      // message loop like any closing window. destroy() only posts a quit
      // message and leaves teardown to process exit, where flutter_windows.dll
      // faulted and Windows Error Reporting held the window for seconds.
      _closed = true;
      try {
        await windowManager.setPreventClose(false);
        await windowManager.close();
      } catch (_) {
        _closed = false;
        // The next close attempt must still save first.
        try {
          await windowManager.setPreventClose(true);
        } catch (_) {}
        rethrow;
      }
    } catch (error) {
      if (!_disposed) {
        final detail = DshError.describe(error).message;
        controller.error = saved
            ? DshWindowZh.closeFailed(detail: detail)
            : DshWindowZh.saveFailed(detail: detail);
        controller.emit();
      }
    }
  }

  /// The production binding lives until process exit; tests and alternate
  /// embedders may detach it explicitly without allowing a late close result.
  void dispose() {
    _disposed = true;
    if (_installed) windowManager.removeListener(this);
    _installed = false;
  }
}
