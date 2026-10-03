import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/design/select.dart';
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
  int configReads = 0;
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
    if (method == 'pluginInventory.list') {
      inventoryReads++;
      return {'entries': entries};
    }
    expect(method, 'pluginInventory.getConfig');
    configReads++;
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
  _EntryClient? replacement;
  @override
  DshClient get client => replacement ?? api;

  void changeConnection({required bool reuseApi}) {
    if (!reuseApi) replacement = _EntryClient();
    host = HostInfo.fromJson({
      'version': 'other-host',
      'home': 'other-home',
      'cwd': 'other-workspace',
    });
    emit();
  }
}

void main() {
  for (final reuseApi in [false, true]) {
    testWidgets(
      'an unvisited editor cannot open after Host change (reuse API: $reuseApi)',
      (tester) async {
        final controller = _EntryController();
        await tester.binding.setSurfaceSize(const Size(720, 700));
        try {
          await tester.pumpWidget(
            ShadApp(
              home: Scaffold(
                body: SettingsResourcePage(
                  controller: controller,
                  page: 'plugins',
                ),
              ),
            ),
          );
          await tester.pumpAndSettle();
          final toggle = find.byKey(
            const ValueKey('plugin-config-toggle-clock'),
          );
          final previousCallback = tester.widget<DshButton>(toggle).onPressed!;
          controller.changeConnection(reuseApi: reuseApi);
          previousCallback();
          await tester.pumpAndSettle();
          expect(tester.widget<DshButton>(toggle).onPressed, isNull);
          expect(
            find.byKey(const ValueKey('time-context-interval')),
            findsNothing,
          );
          expect(controller.api.configReads, 0);
          expect(controller.replacement?.configReads ?? 0, 0);
          expect(controller.api.actions, isEmpty);
          expect(controller.replacement?.actions ?? [], isEmpty);
          expect(tester.takeException(), isNull);
        } finally {
          await tester.pumpWidget(const SizedBox.shrink());
          controller.dispose();
          await tester.binding.setSurfaceSize(null);
        }
      },
    );
  }

  testWidgets('plugin names, runtime state and retired entries stay distinct', (
    tester,
  ) async {
    final controller = _EntryController();
    controller.api.entries = [
      {
        'entryId': 'noop',
        'moduleName': 'cordis:noop',
        'enabled': true,
        'fiberPhase': 'active',
      },
      {
        'entryId': 'review',
        'moduleName': '@deepseek-ai/dsh-experimental-auto-review',
        'enabled': false,
        'fiberPhase': 'failed',
      },
      {'entryId': 'skin', 'moduleName': 'dsh-skin-center', 'enabled': true},
      {
        'entryId': 'skin-scoped',
        'moduleName': '@deepseek-ai/dsh-skin-center',
        'enabled': true,
      },
    ];
    await tester.binding.setSurfaceSize(const Size(720, 900));
    try {
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SettingsResourcePage(controller: controller, page: 'plugins'),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('空操作插件'), findsOneWidget);
      expect(find.text('自动审批'), findsOneWidget);
      expect(find.text('运行状态：运行中'), findsOneWidget);
      expect(find.text('运行状态：启动失败'), findsOneWidget);
      expect(find.text('已启用'), findsOneWidget);
      expect(find.text('未启用'), findsOneWidget);
      expect(find.text('active'), findsNothing);
      expect(find.text('dsh-skin-center'), findsNothing);
      expect(find.text('@deepseek-ai/dsh-skin-center'), findsNothing);
      expect(find.byType(DshSwitch), findsNWidgets(2));
      final search = find.descendant(
        of: find.byType(DshField).first,
        matching: find.byType(TextField),
      );
      await tester.enterText(search, '自动审批');
      await tester.pumpAndSettle();
      expect(
        find.byWidgetPredicate(
          (widget) => widget is Text && widget.data == '自动审批',
        ),
        findsOneWidget,
      );
      expect(find.text('空操作插件'), findsNothing);
      await tester.enterText(search, 'cordis:noop');
      await tester.pumpAndSettle();
      expect(find.text('空操作插件'), findsOneWidget);
      expect(find.text('自动审批'), findsNothing);
      expect(controller.api.inventoryReads, 1);
      expect(controller.api.configReads, 0);
      expect(controller.api.actions, isEmpty);
      expect(tester.takeException(), isNull);
    } finally {
      await tester.pumpWidget(const SizedBox.shrink());
      controller.dispose();
      await tester.binding.setSurfaceSize(null);
    }
  });

  testWidgets(
    'configuration draft survives collapse, filtering, lazy scroll and reorder',
    (tester) async {
      final controller = _EntryController();
      controller.api.entries.addAll([
        for (var i = 0; i < 60; i++)
          {
            'entryId': 'other-$i',
            'moduleName': 'example-plugin-$i',
            'enabled': false,
          },
      ]);
      await tester.binding.setSurfaceSize(const Size(720, 700));
      try {
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: SettingsResourcePage(
                controller: controller,
                page: 'plugins',
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(find.text('example-plugin-59'), findsNothing);
        final toggle = find.byKey(const ValueKey('plugin-config-toggle-clock'));
        await tester.tap(toggle);
        await tester.pumpAndSettle();
        final interval = find.descendant(
          of: find.byKey(const ValueKey('time-context-interval')),
          matching: find.byType(TextField),
        );
        await tester.enterText(interval, '2.5');
        await tester.tap(toggle);
        await tester.pumpAndSettle();
        expect(interval, findsNothing);
        await tester.tap(toggle);
        await tester.pumpAndSettle();
        expect(tester.widget<TextField>(interval).controller!.text, '2.5');

        final scrollable = find
            .descendant(
              of: find.byType(ListView),
              matching: find.byType(Scrollable),
            )
            .first;
        tester.state<ScrollableState>(scrollable).position.jumpTo(5000);
        await tester.pumpAndSettle();
        expect(find.text('example-plugin-0'), findsNothing);
        final search = find.descendant(
          of: find.byType(DshField).first,
          matching: find.byType(TextField),
        );
        await tester.enterText(search, 'does-not-match');
        await tester.pumpAndSettle();
        expect(interval, findsNothing);
        await tester.enterText(search, '时间上下文');
        await tester.pumpAndSettle();
        await tester.ensureVisible(interval);
        expect(tester.widget<TextField>(interval).controller!.text, '2.5');
        expect(
          tester
              .widget<DshSelect<int>>(
                find.byKey(const ValueKey('time-context-unit')),
              )
              .value,
          60000,
        );

        controller.api.entries = controller.api.entries.reversed.toList();
        await tester.tap(find.byTooltip('刷新'));
        await tester.pumpAndSettle();
        await tester.ensureVisible(interval);
        expect(tester.widget<TextField>(interval).controller!.text, '2.5');
        expect(controller.api.configReads, 1);
        expect(controller.api.actions, isEmpty);
        expect(tester.takeException(), isNull);
      } finally {
        await tester.pumpWidget(const SizedBox.shrink());
        controller.dispose();
        await tester.binding.setSurfaceSize(null);
      }
    },
  );

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
        expect(controller.api.configReads, 0);
        await tester.tap(
          find.byKey(const ValueKey('plugin-config-toggle-clock')),
        );
        await tester.pumpAndSettle();
        expect(
          find.byKey(const ValueKey('time-context-interval')),
          findsOneWidget,
        );
        expect(find.textContaining('插件未启用'), findsOneWidget);
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
