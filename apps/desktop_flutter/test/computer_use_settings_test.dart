import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart' show ShadApp;

import 'controller_test.dart' show MemoryPreferences;

class SettingsClient extends DshClient {
  SettingsClient() : super('http://127.0.0.1');
  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async => method == 'settings.describe'
      ? {
          'namespaces': [
            {
              'ns': 'computer-use',
              'revision': 1,
              'applies': 'restart',
              'value': {
                'enabled': false,
                'adapter': 'native-browser',
                'command': '',
              },
              'schema': {
                'uid': 1,
                'refs': {
                  '1': {
                    'type': 'object',
                    'dict': {'enabled': 2, 'adapter': 3, 'command': 7},
                  },
                  '2': {'type': 'boolean'},
                  '3': {
                    'type': 'union',
                    'list': [4, 5, 6],
                  },
                  '4': {'type': 'const', 'value': 'auto'},
                  '5': {'type': 'const', 'value': 'native-browser'},
                  '6': {'type': 'const', 'value': 'command'},
                  '7': {'type': 'string'},
                },
              },
            },
          ],
        }
      : {};
}

class SettingsController extends DesktopController {
  SettingsController() : super(MemoryPreferences());
  final api = SettingsClient();
  @override
  DshClient get client => api;
}

void main() {
  testWidgets('environment settings enable Computer Use with its own labels', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1280, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final c = SettingsController();
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SettingsShell(controller: c, initialPage: 'environment'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Computer Use'), findsOneWidget);
    expect(find.text('启用 Computer Use'), findsOneWidget);
    expect(find.text('执行适配器'), findsOneWidget);
    // Option and field names that other namespaces share keep their
    // Computer Use meaning here.
    expect(find.text('内置浏览器'), findsWidgets);
    expect(find.text('外部控制命令'), findsOneWidget);
    expect(find.text('这些设置需要重启服务后生效'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });
}
