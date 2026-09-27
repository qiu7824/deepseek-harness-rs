import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/resource_page.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

class _EntryClient extends DshClient {
  _EntryClient() : super('http://127.0.0.1');
  final actions = <String>[];
  int inventoryReads = 0;

  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    if (method == 'pluginInventory.list') {
      inventoryReads++;
      return {
        'entries': [
          {
            'entryId': 'clock',
            'moduleName': 'dsh-time-context',
            'enabled': false,
          },
        ],
      };
    }
    expect(method, 'pluginInventory.getConfig');
    return {
      'entryId': 'clock',
      'moduleName': 'dsh-time-context',
      'revision': 'revision-1',
      'config': <String, Object?>{},
    };
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    expect(path, '/__dsh-plugin-manager');
    actions.add(body!['action'] as String);
    return {
      'operation': {
        'version': 1,
        'operationId': 'running-operation',
        'action': 'add',
        'spec': 'example-plugin',
        'phase': 'running',
        'log': '正在安装',
        'error': null,
        'restartRequired': false,
      },
      'configurationError': null,
    };
  }
}

class _EntryController extends DesktopController {
  _EntryController() : super(MemoryPreferences());
  final api = _EntryClient();
  @override
  DshClient get client => api;
}

void main() {
  testWidgets(
    'plugin settings exposes disabled clock config and safely closes a running operation',
    (tester) async {
      final controller = _EntryController();
      await tester.binding.setSurfaceSize(const Size(520, 700));
      try {
        await tester.pumpWidget(
          ShadApp(
            home: MediaQuery(
              data: const MediaQueryData(textScaler: TextScaler.linear(1.5)),
              child: Scaffold(
                body: Padding(
                  padding: const EdgeInsets.all(16),
                  child: SettingsResourcePage(
                    controller: controller,
                    page: 'plugins',
                  ),
                ),
              ),
            ),
          ),
        );
        await tester.pump();
        await tester.pump();
        expect(
          find.byKey(const ValueKey('time-context-interval')),
          findsOneWidget,
        );
        await tester.tap(find.text('安装与维护'));
        await tester.pump(const Duration(milliseconds: 250));
        await tester.pump();
        expect(find.text('插件安装与维护'), findsOneWidget);
        expect(controller.api.actions, ['status']);
        expect(tester.takeException(), isNull);
        await tester.tap(find.byTooltip('关闭插件管理'));
        await tester.pump(const Duration(milliseconds: 250));
        await tester.pump();
        expect(find.text('插件安装与维护'), findsNothing);
        expect(controller.api.actions, ['status']);
        expect(controller.api.inventoryReads, 2);
        expect(tester.takeException(), isNull);
      } finally {
        await tester.pumpWidget(const SizedBox.shrink());
        controller.dispose();
        await tester.binding.setSurfaceSize(null);
      }
    },
  );
}
