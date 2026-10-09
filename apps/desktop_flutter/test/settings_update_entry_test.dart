import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/features/settings/update_panel.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/desktop_updates.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class _PendingSettingsClient extends DshClient {
  _PendingSettingsClient() : super('http://127.0.0.1:9');

  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) {
    final pending = Completer<Json>();
    scope?.register(() {
      if (!pending.isCompleted) {
        pending.completeError(DshException('cancelled', 'closed'));
      }
    });
    return pending.future;
  }
}

class _SettingsController extends DesktopController {
  _SettingsController(this.api)
    : super(DesktopPreferences(writer: (_) async {}));
  final DshClient? api;
  @override
  DshClient? get client => api;
}

void main() {
  for (final pendingHost in [false, true]) {
    testWidgets(
      'local updates remain available with pending Host: $pendingHost',
      (tester) async {
        await tester.binding.setSurfaceSize(const Size(1280, 800));
        final api = pendingHost ? _PendingSettingsClient() : null;
        final c = _SettingsController(api);
        var checks = 0;
        final updates = DesktopUpdateController(
          runner: (arguments, progress) async {
            expect(arguments, ['check']);
            checks++;
            return {'phase': 'current', 'currentVersion': '1.2.3'};
          },
        );
        addTearDown(() async {
          await tester.pumpWidget(const SizedBox());
          c.dispose();
          updates.dispose();
          await api?.close();
          await tester.binding.setSurfaceSize(null);
        });
        await tester.pumpWidget(
          ShadApp(
            home: SettingsShell(
              controller: c,
              initialPage: 'updates',
              updates: updates,
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(find.byType(SettingsUpdatePanel), findsOneWidget);
        expect(find.text('检查更新'), findsOneWidget);
        expect(find.textContaining('1.2.3'), findsOneWidget);
        expect(checks, 1);
        expect(tester.takeException(), isNull);
      },
    );
  }
}
