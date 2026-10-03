import 'dart:async';

import 'package:dsh_desktop/src/preferences.dart';

/// Applies to every test file: preferences without an explicit writer are
/// discarded instead of overwriting the person's real desktop preferences.
Future<void> testExecutable(FutureOr<void> Function() testMain) async {
  DesktopPreferences.debugDefaultWriter = (_) async {};
  await testMain();
}
