import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/sidebar_entries.dart';
import 'package:dsh_desktop/features/settings/plugin_page.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

class NavigationRepairClient extends DshClient {
  NavigationRepairClient() : super('http://127.0.0.1:9');
  final mutations = <String>[];

  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    if (mutation) mutations.add(method);
    return {'entries': [], 'providers': []};
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    if (path == '/__dsh-schedule/wait') {
      final pending = Completer<Json>();
      scope?.register(() {
        if (!pending.isCompleted) {
          pending.completeError(DshException('cancelled', 'closed'));
        }
      });
      return pending.future;
    }
    return {'entries': [], 'providers': [], 'tasks': [], 'bases': []};
  }
}

class NavigationRepairController extends DesktopController {
  NavigationRepairController()
    : super(DesktopPreferences(writer: (_) async {}));
  final api = NavigationRepairClient();
  final selections = <String>[];
  @override
  DshClient get client => api;
  @override
  bool get connected => true;
  @override
  Future<void> select(String id, {bool adoptDraft = false}) async {
    selections.add(id);
    selectedId = id;
    emit();
  }
}

Future<NavigationRepairController> mountRepair(
  WidgetTester tester, {
  Size size = const Size(1280, 800),
}) async {
  await tester.binding.setSurfaceSize(size);
  final controller = NavigationRepairController()
    ..workspaceId = 'a'
    ..selectedId = 'a-session'
    ..host = HostInfo.fromJson({'home': 'a', 'cwd': 'a', 'version': 'test'})
    ..workspaces = [
      {
        'workspaceId': 'a',
        'path': r'E:\a',
        'title': '工作区 A',
        'sessionIds': ['a-session'],
      },
      {
        'workspaceId': 'b',
        'path': r'E:\b',
        'title': '工作区 B',
        'sessionIds': ['b-session'],
      },
    ]
    ..sessions = [
      SessionSummary.fromJson({
        'sessionId': 'a-session',
        'cwd': r'E:\a',
        'displayTitle': 'Alpha',
      }),
      SessionSummary.fromJson({
        'sessionId': 'b-session',
        'cwd': r'E:\b',
        'displayTitle': 'Beta',
      }),
    ];
  controller.preferences.layout['groupExpansion'] = {'a': true, 'b': false};
  addTearDown(() async {
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
    await controller.api.close();
    await tester.binding.setSurfaceSize(null);
  });
  await tester.pumpWidget(DesktopApp(controller: controller));
  await tester.pumpAndSettle();
  return controller;
}

Future<void> control(WidgetTester tester, LogicalKeyboardKey key) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  await tester.sendKeyEvent(key);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
  await tester.pumpAndSettle();
}

Finder get sidebarSearch => find.byWidgetPredicate(
  (widget) => widget is TextField && widget.decoration?.hintText == '搜索会话',
);

void main() {
  testWidgets('original three global entries share a row and open directly', (
    tester,
  ) async {
    await mountRepair(tester);
    final row = find.byType(SidebarEntryRow);
    expect(row, findsOneWidget);
    for (final key in [
      'open-schedule-direct',
      'open-knowledge',
      'open-plugins',
    ]) {
      expect(
        find.descendant(of: row, matching: find.byKey(Key(key))),
        findsOneWidget,
      );
    }
    final schedule = tester.getRect(
      find.byKey(const Key('open-schedule-direct')),
    );
    final knowledge = tester.getRect(find.byKey(const Key('open-knowledge')));
    final plugins = tester.getRect(find.byKey(const Key('open-plugins')));
    expect(schedule.top, knowledge.top);
    expect(knowledge.top, plugins.top);
    expect(schedule.right, lessThan(knowledge.left));
    expect(knowledge.right, lessThan(plugins.left));
    expect(find.byKey(const Key('open-tools')), findsNothing);
    await tester.tap(find.byKey(const Key('open-plugins')));
    await tester.pumpAndSettle();
    expect(find.byType(PluginPage), findsOneWidget);
    expect(find.byKey(const Key('open-settings-direct')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'folder disclosure never changes the new-conversation workspace',
    (tester) async {
      final controller = await mountRepair(tester);
      await tester.tap(find.byKey(const ValueKey('workspace-toggle-b')));
      await tester.pumpAndSettle();
      expect(controller.workspaceId, 'a');
      expect(controller.selectedId, 'a-session');
      expect(find.byKey(const ValueKey('session-b-session')), findsOneWidget);
      await tester.tap(find.text('工作区 B'));
      await tester.pumpAndSettle();
      expect(controller.workspaceId, 'b');
      expect(find.byKey(const ValueKey('session-a-session')), findsOneWidget);
      expect(find.byKey(const ValueKey('session-b-session')), findsOneWidget);
      // Clicking an already selected title leaves its conversations visible.
      await tester.tap(find.text('工作区 B'));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('session-b-session')), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'search reveals collapsed matches and shortcuts follow that view',
    (tester) async {
      final controller = await mountRepair(tester);
      expect(find.byKey(const ValueKey('session-b-session')), findsNothing);
      await tester.tap(find.byTooltip('筛选侧栏会话'));
      await tester.pumpAndSettle();
      await tester.enterText(sidebarSearch, 'Beta');
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('session-b-session')), findsOneWidget);
      expect(find.byKey(const ValueKey('session-a-session')), findsNothing);
      expect(
        object(controller.preferences.layout['groupExpansion'])['b'],
        false,
      );
      await control(tester, LogicalKeyboardKey.digit1);
      expect(controller.selections, ['b-session']);
      await tester.tap(find.byTooltip('筛选侧栏会话'));
      await tester.pumpAndSettle();
      expect(sidebarSearch, findsNothing);
      expect(find.byKey(const ValueKey('session-a-session')), findsOneWidget);
      expect(find.byKey(const ValueKey('session-b-session')), findsNothing);
      expect(
        object(controller.preferences.layout['groupExpansion'])['b'],
        false,
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('search group disclosure is temporary and can still collapse', (
    tester,
  ) async {
    final controller = await mountRepair(tester);
    await tester.tap(find.byTooltip('筛选侧栏会话'));
    await tester.pumpAndSettle();
    await tester.enterText(sidebarSearch, 'Beta');
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('workspace-toggle-b')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('session-b-session')), findsNothing);
    await control(tester, LogicalKeyboardKey.digit1);
    expect(controller.selections, isEmpty);
    await tester.enterText(sidebarSearch, 'Bet');
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('session-b-session')), findsOneWidget);
    expect(object(controller.preferences.layout['groupExpansion'])['b'], false);
    expect(tester.takeException(), isNull);
  });

  testWidgets('drawer collapse closes the narrow-window sidebar immediately', (
    tester,
  ) async {
    final controller = await mountRepair(tester, size: const Size(720, 640));
    await tester.tap(find.byTooltip('展开侧边栏'));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('new-task')).hitTestable(), findsOneWidget);
    await tester.tap(find.byTooltip('收起侧边栏 · Ctrl+B'));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('new-task')).hitTestable(), findsNothing);
    // Closing a temporary drawer does not persist the desktop sidebar closed.
    expect(controller.preferences.layout['sideOpen'], isNot(false));
    await control(tester, LogicalKeyboardKey.keyB);
    expect(find.byKey(const Key('new-task')).hitTestable(), findsOneWidget);
    await control(tester, LogicalKeyboardKey.keyB);
    expect(find.byKey(const Key('new-task')).hitTestable(), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('global page has only its entry selected and restores chat', (
    tester,
  ) async {
    await mountRepair(tester);
    Material sessionMaterial() => tester.widget<Material>(
      find
          .ancestor(
            of: find.byKey(const ValueKey('session-a-session')),
            matching: find.byType(Material),
          )
          .first,
    );
    expect(sessionMaterial().color, isNot(Colors.transparent));
    await tester.tap(find.byKey(const Key('open-plugins')));
    await tester.pumpAndSettle();
    expect(sessionMaterial().color, Colors.transparent);
    await tester.tap(find.byKey(const ValueKey('session-a-session')));
    await tester.pumpAndSettle();
    expect(find.byType(PluginPage), findsNothing);
    expect(sessionMaterial().color, isNot(Colors.transparent));
    expect(find.byKey(const Key('prompt-input')).hitTestable(), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('stale folder context menu cannot act on a changed Host', (
    tester,
  ) async {
    final controller = await mountRepair(tester);
    final pointer = await tester.startGesture(
      tester.getCenter(find.byKey(const ValueKey('workspace-a'))),
      kind: PointerDeviceKind.mouse,
      buttons: kSecondaryMouseButton,
    );
    await pointer.up();
    await tester.pumpAndSettle();
    controller.host = HostInfo.fromJson({
      'home': 'b',
      'cwd': 'b',
      'version': 'test',
    });
    controller.emit();
    await tester.pumpAndSettle();
    await tester.tap(find.text('在文件管理器中打开'));
    await tester.pumpAndSettle();
    expect(controller.api.mutations, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('new-chat tooltip covers the full control and its shortcut', (
    tester,
  ) async {
    await mountRepair(tester);
    final button = find.byKey(const Key('new-task'));
    final tooltip = find.ancestor(
      of: button,
      matching: find.byType(DshTooltip),
    );
    expect(tooltip, findsOneWidget);
    expect(tester.widget<DshTooltip>(tooltip).message, '新建会话 · Ctrl+N');
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(1200, 780));
    await mouse.moveTo(tester.getCenter(button));
    await tester.pump(const Duration(milliseconds: 700));
    expect(find.text('新建会话 · Ctrl+N'), findsOneWidget);
    await mouse.removePointer();
    await tester.pumpAndSettle();
  });
}
