import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/settings/plugin_inventory_row.dart';
import 'package:dsh_desktop/features/settings/plugin_page.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

class _WorkspaceClient extends DshClient {
  _WorkspaceClient() : super('http://127.0.0.1');

  final mutations = <({String method, Json payload})>[];
  final managerActions = <String>[];
  int inventoryReads = 0, configReads = 0;
  List<Json> entries = [
    {'entryId': 'clock', 'moduleName': 'dsh-time-context', 'enabled': false},
  ];

  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    if (mutation) mutations.add((method: method, payload: {...payload}));
    switch (method) {
      case 'pluginInventory.list':
        inventoryReads++;
        return {'entries': entries};
      case 'pluginInventory.getConfig':
        configReads++;
        return {
          'entryId': 'clock',
          'moduleName': 'dsh-time-context',
          'revision': 'revision-1',
          'config': <String, Object?>{},
        };
      case 'pluginInventory.setEnabled':
        entries = [
          for (final entry in entries)
            if (entry['entryId'] == payload['entryId'])
              {...entry, 'enabled': payload['enabled']}
            else
              entry,
        ];
        return {};
      default:
        fail('Unexpected request: $method');
    }
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
    managerActions.add(body!['action'] as String);
    return {'operation': null, 'configurationError': null};
  }
}

class _WorkspaceController extends DesktopController {
  _WorkspaceController() : super(MemoryPreferences());
  final api = _WorkspaceClient();
  int pluginRefreshes = 0;

  @override
  DshClient get client => api;
  @override
  Future<void> loadPlugins() async {
    pluginRefreshes++;
  }

  void changeHost() {
    host = HostInfo.fromJson({
      'version': 'other',
      'home': 'other-home',
      'cwd': 'other-workspace',
    });
    emit();
  }
}

Future<void> _withPage(
  WidgetTester tester,
  _WorkspaceController controller,
  Future<void> Function() body, {
  Size size = const Size(1000, 900),
  double textScale = 1,
}) async {
  await tester.binding.setSurfaceSize(size);
  try {
    await tester.pumpWidget(
      ShadApp(
        home: MediaQuery(
          data: MediaQueryData(textScaler: TextScaler.linear(textScale)),
          child: Scaffold(body: PluginPage(controller: controller)),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await body();
    expect(tester.takeException(), isNull);
  } finally {
    await tester.pumpWidget(const SizedBox.shrink());
    controller.dispose();
    await tester.binding.setSurfaceSize(null);
  }
}

Finder _search() => find.descendant(
  of: find.byType(DshField).first,
  matching: find.byType(TextField),
);
Finder _clock() => find.byKey(const ValueKey('plugin-row-clock'));
Finder _interval() => find.descendant(
  of: find.byKey(const ValueKey('time-context-interval')),
  matching: find.byType(TextField),
);

void main() {
  testWidgets('workspace uses real metadata and distinct runtime state', (
    tester,
  ) async {
    final controller = _WorkspaceController();
    controller.api.entries = [
      {
        'entryId': 'review',
        'moduleName': '@deepseek-ai/dsh-experimental-auto-review',
        'enabled': true,
        'fiberPhase': null,
      },
      {
        'entryId': 'custom',
        'moduleName': 'example-plugin',
        'description': '自定义服务连接',
        'enabled': false,
        'fiberPhase': 'future-state',
        'experimental': true,
      },
      {'entryId': 'skin', 'moduleName': 'dsh-skin-center', 'enabled': true},
    ];
    await _withPage(tester, controller, () async {
      expect(find.text('插件'), findsOneWidget);
      expect(find.text('添加插件'), findsOneWidget);
      expect(find.text('当前服务'), findsOneWidget);
      expect(find.text('2 个插件'), findsOneWidget);
      expect(find.text('自动审批'), findsOneWidget);
      expect(find.text('example-plugin'), findsOneWidget);
      expect(find.text('自定义服务连接'), findsOneWidget);
      expect(find.text('运行状态：未运行'), findsOneWidget);
      expect(find.text('运行状态：状态未知'), findsOneWidget);
      expect(find.text('运行状态：运行中'), findsNothing);
      expect(find.text('实验性'), findsOneWidget);
      expect(find.text('官方'), findsNothing);
      expect(find.text('dsh-skin-center'), findsNothing);
      expect(find.byType(DshSwitch), findsNWidgets(2));
      expect(controller.api.configReads, 0);
      await tester.enterText(_search(), '自动审批');
      await tester.pumpAndSettle();
      expect(find.text('1 个插件'), findsOneWidget);
      expect(find.text('example-plugin'), findsNothing);
      expect(controller.api.inventoryReads, 1);
    });
  });

  testWidgets(
    'workspace drafts survive row collapse, filter, scroll and reorder',
    (tester) async {
      final controller = _WorkspaceController();
      controller.api.entries.addAll([
        for (var i = 0; i < 60; i++)
          {'entryId': 'other-$i', 'moduleName': 'example-$i', 'enabled': false},
      ]);
      await _withPage(tester, controller, () async {
        expect(find.text('example-59'), findsNothing);
        await tester.tap(_clock());
        await tester.pumpAndSettle();
        await tester.enterText(_interval(), '2.5');
        await tester.tap(_clock());
        await tester.pumpAndSettle();
        expect(_interval(), findsNothing);
        await tester.tap(_clock());
        await tester.pumpAndSettle();
        expect(tester.widget<TextField>(_interval()).controller!.text, '2.5');
        final scroll = tester.state<ScrollableState>(
          find
              .descendant(
                of: find.byType(ListView),
                matching: find.byType(Scrollable),
              )
              .first,
        );
        scroll.position.jumpTo(5000);
        await tester.pumpAndSettle();
        expect(find.text('example-0'), findsNothing);
        await tester.enterText(_search(), 'missing');
        await tester.pumpAndSettle();
        expect(_interval(), findsNothing);
        await tester.enterText(_search(), '时间上下文');
        await tester.pumpAndSettle();
        await tester.ensureVisible(_interval());
        expect(tester.widget<TextField>(_interval()).controller!.text, '2.5');
        controller.api.entries = controller.api.entries.reversed.toList();
        await tester.tap(find.byTooltip('刷新'));
        await tester.pumpAndSettle();
        await tester.ensureVisible(_interval());
        expect(tester.widget<TextField>(_interval()).controller!.text, '2.5');
        expect(controller.api.configReads, 1);
        expect(controller.api.mutations, isEmpty);
      }, size: const Size(820, 700));
    },
  );

  testWidgets('same client Host switch rejects a retained row callback', (
    tester,
  ) async {
    final controller = _WorkspaceController();
    await _withPage(tester, controller, () async {
      final configure = tester
          .widget<PluginInventoryRow>(_clock())
          .onConfigure!;
      controller.changeHost();
      configure();
      await tester.pumpAndSettle();
      expect(tester.widget<PluginInventoryRow>(_clock()).onConfigure, isNull);
      expect(_interval(), findsNothing);
      expect(controller.api.configReads, 0);
      expect(controller.api.mutations, isEmpty);
    });
  });

  testWidgets('disabling schedule only changes plugin enablement', (
    tester,
  ) async {
    final controller = _WorkspaceController();
    controller.api.entries = [
      {'entryId': 'schedule', 'moduleName': 'dsh-schedule', 'enabled': true},
    ];
    await _withPage(tester, controller, () async {
      tester.widget<DshSwitch>(find.byType(DshSwitch)).onChanged!(false);
      await tester.pumpAndSettle();
      expect(controller.api.mutations, hasLength(1));
      expect(
        controller.api.mutations.single.method,
        'pluginInventory.setEnabled',
      );
      expect(controller.api.mutations.single.payload, {
        'entryId': 'schedule',
        'enabled': false,
      });
      expect(controller.pluginRefreshes, 1);
      expect(find.text('未启用'), findsOneWidget);
    });
  });

  testWidgets('short viewport at double text scale scrolls configuration', (
    tester,
  ) async {
    final controller = _WorkspaceController();
    await _withPage(
      tester,
      controller,
      () async {
        await tester.tap(_clock());
        await tester.pumpAndSettle();
        await tester.ensureVisible(_interval());
        expect(_interval(), findsOneWidget);
        await tester.ensureVisible(
          find.byKey(const ValueKey('time-context-save')),
        );
        expect(controller.api.configReads, 1);
        expect(controller.api.mutations, isEmpty);
      },
      size: const Size(720, 520),
      textScale: 2,
    );
  });

  testWidgets('narrow large text keeps details and maintenance scrollable', (
    tester,
  ) async {
    final controller = _WorkspaceController();
    await _withPage(
      tester,
      controller,
      () async {
        await tester.tap(_clock());
        await tester.pumpAndSettle();
        await tester.ensureVisible(_interval());
        expect(_interval(), findsOneWidget);
        expect(tester.takeException(), isNull);
        await tester.tap(find.byKey(const ValueKey('plugin-add')));
        await tester.pumpAndSettle();
        expect(find.text('插件安装与维护'), findsOneWidget);
        expect(controller.api.managerActions, ['status']);
        await tester.tap(find.byTooltip('关闭插件管理'));
        await tester.pumpAndSettle();
        expect(controller.api.inventoryReads, 2);
        expect(controller.api.mutations, isEmpty);
      },
      size: const Size(460, 680),
      textScale: 1.5,
    );
  });
}
