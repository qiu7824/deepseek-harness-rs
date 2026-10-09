import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/plugin_page.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/features/schedule/schedule_page.dart' as schedule;
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;
import 'workbench_tabs_test.dart' as tabs show TabApi, TabController;

class StartClient extends FakeClient {
  final requests = <({String path, Json? body, bool mutation})>[];
  final methods = <String>[];
  final terminals = <Json>[];
  final scheduledTasks = <Json>[];
  Completer<Json>? creation;
  Completer<Json>? terminalListing;
  Completer<HistoryPage>? selectionHistory;
  static const workspace = {
    'workspaceId': 'workspace',
    'path': r'E:\project',
    'title': '示例工作区',
  };

  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    methods.add(method);
    if (method == 'session.create') {
      final result =
          await (creation?.future ??
              Future.value(<String, dynamic>{'sessionId': 'created'}));
      liveSessions = [
        SessionSummary.fromJson({
          'sessionId': result['sessionId'],
          'cwd': payload['cwd'],
          'blank': true,
        }),
      ];
      histories[result['sessionId'] as String] =
          selectionHistory?.future ??
          Future.value(HistoryPage.fromJson({'events': []}));
      return result;
    }
    return {
      'items': [workspace],
      'archivedSessionIds': [],
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
    requests.add((path: path, body: body, mutation: mutation));
    final route = Uri.parse(path).path;
    if (route == '/__dsh-preview/terminal-action' &&
        body?['action'] == 'open') {
      terminals.add({'id': 'terminal-${terminals.length + 1}'});
      return terminals.last;
    }
    if (route == '/__dsh-preview/terminal-list') {
      return await (terminalListing?.future ??
          Future.value(<String, dynamic>{'entries': terminals}));
    }
    if (route == '/__dsh-preview/terminal-output') return {'output': ''};
    if (route == '/__dsh-computer-use/meta') {
      return {'enabled': false, 'available': false};
    }
    if (route == '/__dsh-schedule/catalog') {
      return {'tasks': scheduledTasks, 'revision': 0};
    }
    return {'entries': [], 'providers': []};
  }
}

Future<(DesktopController, StartClient)> mountStarter(
  WidgetTester tester, {
  bool existing = false,
}) async {
  await tester.binding.setSurfaceSize(const Size(1440, 900));
  final api = StartClient();
  final controller = DesktopController(
    MemoryPreferences(),
    clientFactory: (_) => api,
  );
  await controller.connect('http://127.0.0.1');
  await tester.pump();
  controller.targetWorkspace('workspace');
  if (existing) {
    controller.selectedId = 'existing';
    controller.sessions = [
      SessionSummary.fromJson({
        'sessionId': 'existing',
        'cwd': r'E:\project',
        'blank': false,
      }),
    ];
  }
  addTearDown(() async {
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
    await tester.binding.setSurfaceSize(null);
  });
  await tester.pumpWidget(DesktopApp(controller: controller));
  await tester.pumpAndSettle();
  return (controller, api);
}

Future<void> openStarter(WidgetTester tester) async {
  await tester.tap(find.byTooltip('显示工作台'));
  await tester.pumpAndSettle();
  expect(find.byKey(const ValueKey('workbench-tab-start')), findsOneWidget);
}

void main() {
  testWidgets(
    'opening the Hero workbench does not read files or create resources',
    (tester) async {
      final (controller, api) = await mountStarter(tester);
      controller.setDraft('尚未发送的内容');
      await openStarter(tester);
      for (final key in ['files', 'terminal', 'computer-use']) {
        expect(find.byKey(ValueKey('workbench-start-$key')), findsOneWidget);
      }
      expect(controller.selectedId, isNull);
      expect(controller.draft, '尚未发送的内容');
      expect(api.methods, isNot(contains('session.create')));
      expect(
        api.requests.any((r) => r.path.startsWith('/__dsh-preview/')),
        isFalse,
      );
      expect(
        api.requests.any((r) => r.path.startsWith('/__dsh-computer-use/')),
        isFalse,
      );
      expect(tester.takeException(), isNull);
      await tester.pump(const Duration(milliseconds: 600));
    },
  );

  testWidgets(
    'explicit file click creates a guarded session and adopts the Hero draft',
    (tester) async {
      final (controller, api) = await mountStarter(tester);
      controller.setDraft('文件查看前的草稿');
      final original = controller.draftScopeKey;
      await openStarter(tester);
      await tester.tap(find.byKey(const ValueKey('workbench-start-files')));
      await tester.pumpAndSettle();
      expect(controller.selectedId, 'created');
      expect(controller.draft, '文件查看前的草稿');
      expect(controller.preferences.drafts.containsKey(original), isFalse);
      expect(api.methods.where((m) => m == 'session.create'), hasLength(1));
      expect(api.methods, isNot(contains('session.prompt')));
      expect(
        api.requests.any(
          (r) =>
              Uri.parse(r.path).path == '/__dsh-preview/list' &&
              Uri.parse(r.path).queryParameters['sessionId'] == 'created',
        ),
        isTrue,
      );
      expect(api.requests.where((r) => r.mutation), isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'a terminal card creates one terminal without sending a command',
    (tester) async {
      final (_, api) = await mountStarter(tester);
      await openStarter(tester);
      await tester.tap(find.byKey(const ValueKey('workbench-start-terminal')));
      await tester.pumpAndSettle();
      expect(api.terminals, hasLength(1));
      expect(
        api.requests.where((r) => r.mutation).map((r) => r.body?['action']),
        everyElement(isIn(['open', 'resize'])),
      );
      expect(api.requests.any((r) => r.body?['action'] == 'input'), isFalse);
      expect(api.methods, isNot(contains('session.prompt')));
      await tester.tap(find.byKey(const Key('workbench-add-tab')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('workbench-open-start')));
      await tester.pumpAndSettle();
      expect(api.terminals, hasLength(1));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'browser card opens the real Computer Use view without starting it',
    (tester) async {
      final (_, api) = await mountStarter(tester);
      await openStarter(tester);
      expect(find.textContaining('Computer Use ·'), findsOneWidget);
      await tester.tap(
        find.byKey(const ValueKey('workbench-start-computer-use')),
      );
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('workbench-tab-computer-use')),
        findsOneWidget,
      );
      expect(
        api.requests.any((r) => r.path.startsWith('/__dsh-computer-use/meta')),
        isTrue,
      );
      expect(api.requests.any((r) => r.body?['action'] == 'open'), isFalse);
      expect(api.methods, isNot(contains('session.prompt')));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'retargeting during creation leaves the draft and never reads the stale workspace',
    (tester) async {
      final (controller, api) = await mountStarter(tester);
      controller.setDraft('原工作区草稿');
      final original = controller.draftScopeKey;
      api.creation = Completer<Json>();
      await openStarter(tester);
      await tester.tap(find.byKey(const ValueKey('workbench-start-files')));
      await tester.pump();
      controller.targetWorkspace('different');
      controller.setDraft('新工作区草稿');
      api.creation!.complete({'sessionId': 'late'});
      await tester.pumpAndSettle();
      expect(controller.selectedId, isNull);
      expect(controller.draft, '新工作区草稿');
      expect(controller.preferences.drafts[original], '原工作区草稿');
      expect(
        api.requests.any((r) => r.path.startsWith('/__dsh-preview/')),
        isFalse,
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'sidebar plugins open a full workspace and header actions stay in the menu',
    (tester) async {
      final (_, api) = await mountStarter(tester, existing: true);
      expect(find.byType(SessionFeedbackDialog), findsNothing);
      expect(find.byKey(const Key('session-menu-feedback')), findsNothing);
      await tester.tap(find.byKey(const Key('more-header-menu')));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('session-menu-download')), findsOneWidget);
      expect(find.byKey(const Key('session-menu-schedule')), findsOneWidget);
      await tester.tap(find.byKey(const Key('session-menu-feedback')));
      await tester.pumpAndSettle();
      expect(find.byType(SessionFeedbackDialog), findsOneWidget);
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('open-plugins')));
      await tester.pumpAndSettle();
      expect(find.byType(PluginPage), findsOneWidget);
      expect(tester.getSize(find.byType(PluginPage)).width, greaterThan(800));
      expect(find.byKey(const Key('open-schedule-direct')), findsOneWidget);
      expect(
        find.byKey(const ValueKey('account-connection-menu')),
        findsOneWidget,
      );
      expect(api.methods, isNot(contains('session.prompt')));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('the more menu retains direct navigation to an active schedule', (
    tester,
  ) async {
    final (_, api) = await mountStarter(tester, existing: true);
    api.scheduledTasks.add({
      'id': 'active-task',
      'title': '检查构建结果',
      'status': 'active',
      'sessionId': 'existing',
    });
    await tester.tap(find.byKey(const Key('more-header-menu')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('schedule-badge')), findsOneWidget);
    await tester.tap(find.byKey(const Key('schedule-badge')));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<schedule.SchedulePage>(find.byType(schedule.SchedulePage))
          .initialTaskId,
      'active-task',
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'a stale header menu cannot act on a newly selected conversation',
    (tester) async {
      final (controller, _) = await mountStarter(tester, existing: true);
      await tester.tap(find.byKey(const Key('more-header-menu')));
      await tester.pumpAndSettle();
      controller.newConversation();
      await tester.pump();
      await tester.tap(find.byKey(const Key('session-menu-feedback')));
      await tester.pumpAndSettle();
      expect(find.byType(SessionFeedbackDialog), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'closing the workbench cancels a pending terminal creation intent',
    (tester) async {
      final (_, api) = await mountStarter(tester, existing: true);
      await openStarter(tester);
      api.terminalListing = Completer<Json>();
      await tester.tap(find.byKey(const ValueKey('workbench-start-terminal')));
      await tester.pump();
      await tester.tap(find.byTooltip('显示工作台'));
      await tester.pumpAndSettle();
      api.terminalListing!.complete({'entries': <Json>[]});
      await tester.pump();
      await tester.tap(find.byTooltip('显示工作台'));
      await tester.pumpAndSettle();
      expect(api.terminals, isEmpty);
      expect(api.requests.any((r) => r.body?['action'] == 'open'), isFalse);
      expect(tester.takeException(), isNull);
    },
  );

  for (final closeStartTab in [false, true]) {
    testWidgets(
      'cancelling the Hero terminal before session creation completes '
      '(close start tab: $closeStartTab)',
      (tester) async {
        final (_, api) = await mountStarter(tester);
        api.creation = Completer<Json>();
        await openStarter(tester);
        await tester.tap(
          find.byKey(const ValueKey('workbench-start-terminal')),
        );
        await tester.pump();
        expect(api.methods.where((m) => m == 'session.create'), hasLength(1));
        if (closeStartTab) {
          await tester.tap(find.byKey(const ValueKey('workbench-close-start')));
        } else {
          await tester.tap(find.byTooltip('显示工作台'));
        }
        await tester.pumpAndSettle();
        api.creation!.complete({'sessionId': 'created'});
        await tester.pumpAndSettle();
        expect(api.terminals, isEmpty);
        expect(
          api.requests.any((r) => r.path.startsWith('/__dsh-preview/')),
          isFalse,
        );
        expect(
          find.byKey(const ValueKey('workbench-tab-terminal')),
          findsNothing,
        );
        if (!closeStartTab) expect(find.byType(WorkbenchPanel), findsNothing);
        expect(api.methods, isNot(contains('session.prompt')));
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets(
    'a workspace choice during draft adoption invalidates the pending tool',
    (tester) async {
      final (controller, api) = await mountStarter(tester);
      api.creation = Completer<Json>();
      api.selectionHistory = Completer<HistoryPage>();
      await openStarter(tester);
      await tester.tap(find.byKey(const ValueKey('workbench-start-terminal')));
      await tester.pump();
      api.creation!.complete({'sessionId': 'created'});
      await tester.pump();
      expect(controller.selectedId, 'created');
      controller.targetWorkspace('different');
      api.selectionHistory!.complete(HistoryPage.fromJson({'events': []}));
      await tester.pumpAndSettle();
      expect(api.terminals, isEmpty);
      expect(
        api.requests.any((r) => r.path.startsWith('/__dsh-preview/')),
        isFalse,
      );
      expect(
        find.byKey(const ValueKey('workbench-tab-terminal')),
        findsNothing,
      );
      expect(tester.takeException(), isNull);
    },
  );

  for (final ownerChange in ['session', 'host', 'workspace']) {
    testWidgets('a pending terminal cannot outlive its $ownerChange owner', (
      tester,
    ) async {
      final (controller, api) = await mountStarter(tester);
      api.creation = Completer<Json>();
      await openStarter(tester);
      await tester.tap(find.byKey(const ValueKey('workbench-start-terminal')));
      await tester.pump();
      switch (ownerChange) {
        case 'session':
          await controller.select('another');
        case 'host':
          controller.host = HostInfo.fromJson({
            'home': 'other',
            'version': 'test',
            'cwd': 'other',
          });
          controller.emit();
        case 'workspace':
          controller.targetWorkspace('workspace');
      }
      api.creation!.complete({'sessionId': 'created'});
      await tester.pumpAndSettle();
      expect(api.terminals, isEmpty);
      expect(
        api.requests.any((r) => r.path.startsWith('/__dsh-preview/')),
        isFalse,
      );
      expect(
        find.byKey(const ValueKey('workbench-tab-terminal')),
        findsNothing,
      );
      expect(tester.takeException(), isNull);
    });
  }

  for (final hostOnly in [false, true]) {
    testWidgets(
      'a header menu rejects a changed Host (same client: $hostOnly)',
      (tester) async {
        await tester.binding.setSurfaceSize(const Size(1440, 900));
        final first = tabs.TabApi(), second = tabs.TabApi();
        final controller = tabs.TabController(first);
        await tester.pumpWidget(DesktopApp(controller: controller));
        await tester.pumpAndSettle();
        await tester.tap(find.byKey(const Key('more-header-menu')));
        await tester.pumpAndSettle();
        if (hostOnly) {
          controller.host = HostInfo.fromJson({
            'home': 'other-host',
            'version': 'test',
            'cwd': 'other',
          });
        } else {
          controller.api = second;
        }
        controller.emit();
        await tester.pumpAndSettle();
        await tester.tap(find.byKey(const Key('session-menu-feedback')));
        await tester.pumpAndSettle();
        expect(find.byType(SessionFeedbackDialog), findsNothing);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox());
        controller.dispose();
        await first.close();
        await second.close();
        await tester.binding.setSurfaceSize(null);
      },
    );
  }
}
