import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/schedule/schedule_page.dart';
import 'package:dsh_desktop/features/sidebar_entries.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class SchedulePreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class FakeScheduleApi extends ScheduleApi {
  FakeScheduleApi(this.tasks) : super(DshClient('http://127.0.0.1:9'));
  List<Json> tasks;
  final calls = <(String, Json)>[];
  int revision = 1;
  bool conflictNext = false;

  @override
  Future<Json> call(
    String operation, [
    Json body = const {},
    RequestScope? scope,
  ]) async {
    calls.add((operation, body));
    switch (operation) {
      case 'wait':
        return Completer<Json>().future;
      case 'catalog':
        final session = body['sessionId'];
        return {
          'tasks': [
            for (final task in tasks)
              if (session == null || task['sessionId'] == session) task,
          ],
          'revision': revision,
          'error': null,
          'hostTimeZone': 'Asia/Shanghai',
        };
      case 'update':
        if (conflictNext) {
          conflictNext = false;
          throw DshException('http-409', '任务已被其他操作修改');
        }
        final task = tasks.firstWhere((row) => row['id'] == body['id']);
        task['title'] = body['title'];
        task['updatedAt'] = '2026-09-26T09:00:00.000Z';
        if (body['rule'] != null) task['rule'] = body['rule'];
        revision++;
        return {'task': task};
      case 'setActive':
        final task = tasks.firstWhere((row) => row['id'] == body['id']);
        task['status'] = body['active'] == true ? 'active' : 'inactive';
        revision++;
        return {'task': task};
      case 'delete':
        tasks.removeWhere((row) => row['id'] == body['id']);
        revision++;
        return {'deleted': true};
      case 'history':
        return {'records': <Json>[], 'total': 0, 'hasMore': false};
      case 'create':
        final task = {
          ...tasks.first,
          'id': 'task-new',
          'sessionId': body['sessionId'],
          'title': body['title'] == '' ? body['prompt'] : body['title'],
          'prompt': body['prompt'],
          'rule': body['rule'],
        };
        tasks = [...tasks, task];
        revision++;
        return {'task': task};
    }
    throw StateError(operation);
  }
}

Json task(
  String id, {
  String status = 'active',
  String session = 's1',
  Json? rule,
  String origin = 'user',
}) => {
  'id': id,
  'sessionId': session,
  'title': '任务 $id',
  'prompt': '执行 $id',
  'rule':
      rule ?? {'kind': 'daily', 'time': '09:00', 'timeZone': 'Asia/Shanghai'},
  'status': status,
  'origin': origin,
  'createdAt': '2026-09-01T00:00:00.000Z',
  'updatedAt': '2026-09-01T00:00:00.000Z',
  if (status == 'active')
    'nextRunAt': DateTime.now()
        .add(const Duration(hours: 3))
        .toUtc()
        .toIso8601String(),
  'historyCount': 0,
};

DesktopController controllerWithSessions() {
  final c = DesktopController(SchedulePreferences());
  c.sessions = [
    SessionSummary.fromJson({
      'sessionId': 's1',
      'cwd': 'D:/work',
      'displayTitle': '新闻整理',
    }),
    SessionSummary.fromJson({
      'sessionId': 's2',
      'cwd': 'D:/work',
      'displayTitle': '周报',
    }),
  ];
  c.workspaces = [
    {'workspaceId': 'w1', 'title': '工作', 'path': r'\\?\D:\work'},
  ];
  c.selectedId = 's1';
  return c;
}

Future<void> pumpPage(
  WidgetTester tester,
  FakeScheduleApi api,
  DesktopController c, {
  List<String>? opened,
}) async {
  tester.view.physicalSize = const Size(1400, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: SchedulePage(
          controller: c,
          api: api,
          onClose: () {},
          onOpenSession: (id) => opened?.add(id),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  unsupportedHostTests();
  test('rule labels and drafts cover every kind', () {
    expect(
      scheduleRuleLabel({'kind': 'every', 'everySeconds': 7200}),
      '每 2 小时',
    );
    expect(
      scheduleRuleLabel({'kind': 'every', 'everySeconds': 900}),
      '每 15 分钟',
    );
    expect(
      scheduleRuleLabel({
        'kind': 'weekly',
        'time': '18:30',
        'weekdays': [1, 5],
        'timeZone': 'Asia/Shanghai',
      }, localZone: 'Asia/Shanghai'),
      '每周一、周五 18:30',
    );
    expect(
      scheduleRuleLabel({
        'kind': 'daily',
        'time': '09:00',
        'timeZone': 'UTC',
      }, localZone: 'Asia/Shanghai'),
      '每天 09:00（UTC）',
    );
    expect(
      scheduleRuleLabel({
        'kind': 'cron',
        'expression': '0 9 * * 1-5',
        'timeZone': 'UTC',
      }),
      'Cron · 0 9 * * 1-5',
    );
    for (final rule in [
      {'kind': 'every', 'everySeconds': 86400},
      {'kind': 'daily', 'time': '07:45', 'timeZone': 'UTC'},
      {
        'kind': 'weekly',
        'time': '08:00',
        'weekdays': [2, 4],
        'timeZone': 'UTC',
      },
      {'kind': 'cron', 'expression': '*/30 * * * *', 'timeZone': 'UTC'},
    ]) {
      expect(RuleDraft.fromRule(rule, 'UTC').toRule(), rule);
    }
    final now = DateTime.utc(2026, 9, 26, 8);
    expect(relativeScheduleTime('2026-09-26T08:30:00Z', now: now), '30 分钟后');
    expect(relativeScheduleTime('2026-09-26T11:00:00Z', now: now), '3 小时后');
    expect(relativeScheduleTime('2026-09-28T08:00:00Z', now: now), '2 天后');
  });

  testWidgets(
    'filters, edits with the observed revision, toggles and deletes',
    (tester) async {
      final api = FakeScheduleApi([
        task('a'),
        task(
          'b',
          session: 's2',
          rule: {
            'kind': 'weekly',
            'time': '18:30',
            'weekdays': [1, 5],
            'timeZone': 'Asia/Shanghai',
          },
          origin: 'agent',
        ),
        task('c', status: 'inactive'),
      ]);
      final c = controllerWithSessions();
      final opened = <String>[];
      await pumpPage(tester, api, c, opened: opened);
      expect(find.byKey(const ValueKey('schedule-task-a')), findsOneWidget);
      expect(
        find.text('周报'),
        findsOneWidget,
        reason: 'cards name their conversation',
      );
      await tester.tap(find.byKey(const ValueKey('schedule-filter-inactive')));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('schedule-task-a')), findsNothing);
      expect(find.byKey(const ValueKey('schedule-task-c')), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('schedule-filter-all')));
      await tester.pumpAndSettle();

      await tester.tap(find.byKey(const ValueKey('schedule-task-a')));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('schedule-detail')), findsOneWidget);
      expect(find.text('由你创建'), findsOneWidget);
      final name = find
          .descendant(
            of: find.byKey(const Key('schedule-detail')),
            matching: find.byType(TextField),
          )
          .first;
      await tester.enterText(name, '晨报');
      await tester.pump();
      expect(find.text('有未保存的修改'), findsOneWidget);
      await tester.tap(find.byKey(const Key('schedule-save')));
      await tester.pumpAndSettle();
      final update = api.calls.lastWhere((call) => call.$1 == 'update').$2;
      expect(update['expectedUpdatedAt'], '2026-09-01T00:00:00.000Z');
      expect(update['title'], '晨报');
      expect(
        update.containsKey('rule'),
        isFalse,
        reason: 'an unchanged rule is not resent',
      );

      api.conflictNext = true;
      await tester.enterText(name, 'stale');
      await tester.pump();
      await tester.tap(find.byKey(const Key('schedule-save')));
      await tester.pumpAndSettle();
      expect(find.text('任务已被其他操作修改，已刷新为最新内容'), findsOneWidget);

      await tester.tap(find.byKey(const Key('schedule-active')));
      await tester.pumpAndSettle();
      expect(api.calls.lastWhere((call) => call.$1 == 'setActive').$2, {
        'id': 'a',
        'sessionId': 's1',
        'active': false,
      });

      await tester.tap(find.text('打开会话'));
      expect(opened, ['s1']);

      await tester.tap(find.byKey(const Key('schedule-delete')));
      await tester.pumpAndSettle();
      await tester.tap(find.widgetWithText(DshButtonFinder.type, '删除').last);
      await tester.pumpAndSettle();
      expect(api.calls.lastWhere((call) => call.$1 == 'delete').$2, {
        'id': 'a',
        'sessionId': 's1',
      });
      expect(find.byKey(const Key('schedule-detail')), findsNothing);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );

  testWidgets('creates a weekly task in the current conversation', (
    tester,
  ) async {
    final api = FakeScheduleApi([task('a')]);
    final c = controllerWithSessions();
    await pumpPage(tester, api, c);
    await tester.tap(find.byKey(const Key('schedule-create')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.descendant(
        of: find.byKey(const Key('schedule-create-prompt')),
        matching: find.byType(TextField),
      ),
      '每周汇总提交',
    );
    await tester.tap(find.byKey(const ValueKey('rule-kind-weekly')).last);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('rule-weekday-6')).last);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('schedule-create-submit')));
    await tester.pumpAndSettle();
    final create = api.calls.lastWhere((call) => call.$1 == 'create').$2;
    expect(create['sessionId'], 's1');
    expect(create['prompt'], '每周汇总提交');
    expect(create['rule'], {
      'kind': 'weekly',
      'time': '09:00',
      'weekdays': [1, 2, 3, 4, 5, 6],
      'timeZone': 'Asia/Shanghai',
    });
    expect(
      find.byKey(const Key('schedule-detail')),
      findsOneWidget,
      reason: 'the created task opens',
    );
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });

  testWidgets(
    'sidebar entries share one row and collapse to icons when narrow',
    (tester) async {
      Future<void> pumpWidth(double width) => tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(
              child: SizedBox(
                width: width,
                child: SidebarEntryRow(
                  entries: [
                    SidebarEntry(
                      key: const Key('e1'),
                      icon: LucideIcons.alarmClock,
                      label: '定时任务',
                      onPressed: () {},
                    ),
                    SidebarEntry(
                      key: const Key('e2'),
                      icon: LucideIcons.grid2x2,
                      label: '插件',
                      onPressed: () {},
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      );
      await pumpWidth(240);
      expect(find.text('定时任务'), findsOneWidget);
      expect(
        tester.getSize(find.byKey(const Key('e1'))).width,
        tester.getSize(find.byKey(const Key('e2'))).width,
      );
      expect(
        tester.getTopLeft(find.byKey(const Key('e1'))).dy,
        tester.getTopLeft(find.byKey(const Key('e2'))).dy,
      );
      await pumpWidth(120);
      expect(find.text('定时任务'), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'conversation badge shows only active tasks of that conversation',
    (tester) async {
      final api = FakeScheduleApi([
        task('a'),
        task('b', status: 'inactive'),
        task('c', session: 's2'),
      ]);
      final c = controllerWithSessions();
      String? opened = 'none';
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ScheduleSessionBadge(
              controller: c,
              sessionId: 's1',
              api: api,
              onOpen: (id) => opened = id,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('1'), findsOneWidget);
      await tester.tap(find.byKey(const Key('schedule-badge')));
      expect(opened, 'a');
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ScheduleSessionBadge(
              key: const ValueKey('other'),
              controller: c,
              sessionId: 'none',
              api: api,
              onOpen: (_) {},
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('schedule-badge')), findsNothing);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
}

/// Finder helper for the design system's button type.
abstract final class DshButtonFinder {
  static const type = ShadButton;
}

class UnsupportedScheduleApi extends ScheduleApi {
  UnsupportedScheduleApi() : super(DshClient('http://127.0.0.1:9'));
  int calls = 0;
  @override
  Future<Json> call(
    String operation, [
    Json body = const {},
    RequestScope? scope,
  ]) async {
    calls++;
    return {'providers': []};
  }
}

void unsupportedHostTests() {
  testWidgets(
    'a Host without scheduled tasks ends the watch instead of spinning',
    (tester) async {
      final api = UnsupportedScheduleApi();
      final c = controllerWithSessions();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ScheduleSessionBadge(
              controller: c,
              sessionId: 's1',
              api: api,
              onOpen: (_) {},
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(api.calls, 1);
      expect(find.byKey(const Key('schedule-badge')), findsNothing);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
}
