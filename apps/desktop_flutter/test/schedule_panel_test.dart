import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/settings/schedule_panel.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences, FakeClient;

Json reminder(String id, {String status = 'active'}) => {
  'id': id,
  'title': id == 'r1' ? '每日报告' : '旧提醒',
  'prompt': '检查项目状态',
  'kind': 'daily',
  'time': '09:00:00',
  'timeZone': 'Asia/Shanghai',
  'scheduledAt': '2030-01-01T01:00:00.000Z',
  'sessionId': 's2',
  'status': status,
};

class ScheduleWireClient extends DshClient {
  ScheduleWireClient() : super('http://127.0.0.1');
  bool enabled = false, conflict = false, configConflict = false;
  String configRevision = 'config-1';
  Json config = {'deliveryHistoryDays': 30, 'deliveryHistoryRecords': 200};
  final entries = <Json>[reminder('r1'), reminder('r2', status: 'inactive')];
  final requests = <Json>[], scopes = <RequestScope>[];
  final pending = <String, Completer<Object?>>{};

  @override
  Future<Uint8List> bytes(
    String path, {
    Json? body,
    RequestScope? scope,
    int maxBytes = 16 * 1024 * 1024,
    bool mutation = false,
  }) async {
    requests.add(body!);
    if (scope != null) scopes.add(scope);
    final method = body['method'] as String, payload = object(body['payload']);
    Object? value;
    Json? error;
    if (pending[method] != null) {
      value = await pending[method]!.future;
    } else {
      switch (method) {
        case 'schedule.catalog':
          value = entries;
        case 'pluginInventory.list':
          value = {
            'entries': [
              {
                'entryId': 'plugin-schedule',
                'moduleName': 'dsh-schedule',
                'enabled': enabled,
              },
            ],
          };
        case 'pluginInventory.setEnabled':
          enabled = payload['enabled'] == true;
          value = {};
        case 'pluginInventory.getConfig':
          value = {
            'entryId': 'plugin-schedule',
            'moduleName': 'dsh-schedule',
            'revision': configRevision,
            'config': config,
          };
        case 'pluginInventory.setConfig':
          if (configConflict) {
            error = {
              'code': 'plugin-config-conflict',
              'message': '配置已改变',
              'details': {},
            };
          } else {
            config = object(payload['config']);
            configRevision = 'config-2';
            value = {
              'entryId': 'plugin-schedule',
              'revision': configRevision,
              'config': config,
            };
          }
        case 'schedule.create':
          value = {
            'id': 'created',
            'kind': 'after',
            'title': payload['title'],
            'prompt': payload['prompt'],
            'afterSeconds': 600,
            'scheduledAt': '2030-01-01T01:00:00Z',
          };
        case 'schedule.update':
          value = conflict
              ? {
                  'id': payload['id'],
                  'updated': false,
                  'code': 'schedule_conflict',
                }
              : {
                  'id': payload['id'],
                  'updated': true,
                  'record': {
                    ...object(payload['expected']),
                    'title': payload['title'],
                    'prompt': payload['prompt'],
                  },
                };
        case 'schedule.delete':
          entries.removeWhere((entry) => entry['id'] == payload['id']);
          value = {'id': payload['id'], 'deleted': true};
        case 'schedule.history':
          value = deliveryPage(payload['before'] as String?);
        case 'schedule.retry':
          value = {'requested': true, 'enabled': enabled};
        default:
          value = {};
      }
    }
    return Uint8List.fromList(
      utf8.encode(
        jsonEncode({
          'type': 'server-response',
          'rpcId': body['rpcId'],
          'result': error == null
              ? {'ok': true, 'value': value}
              : {'ok': false, 'error': error},
        }),
      ),
    );
  }

  Json deliveryPage(String? before) => {
    'id': 'r1',
    'records': [
      {
        'scheduledAt': '2030-01-01T01:00:00Z',
        'deliveredAt': '2030-01-01T01:00:01Z',
        'messageId': before == null ? 'message-2' : 'message-1',
        'prompt': before == null ? '最近发送内容' : '更早发送内容',
      },
    ],
    if (before == null) 'nextBefore': 'message-2',
    'earlierRecordsUnavailable': true,
    'earlierRecordsPruned': true,
    'retention': {'days': 30, 'records': 200},
  };
  List<Json> called(String method) =>
      requests.where((request) => request['method'] == method).toList();
}

class ScheduleController extends DesktopController {
  ScheduleController() : super(MemoryPreferences()) {
    selectedId = 's1';
    sessions = [
      for (final id in ['s1', 's2'])
        SessionSummary.fromJson({'sessionId': id, 'displayTitle': '会话 $id'}),
    ];
  }
  final api = ScheduleWireClient();
  ScheduleWireClient? replacement;
  @override
  DshClient get client => replacement ?? api;
  void changeSession() {
    selectedId = 's2';
    emit();
  }

  void changeHost() {
    replacement = ScheduleWireClient();
    emit();
  }

  void notifySchedules({bool? enabled}) {
    scheduleRevision++;
    scheduleEnabled = enabled;
    emit();
  }
}

Future<ScheduleController> mount(
  WidgetTester tester, {
  ScheduleController? controller,
  bool shell = false,
}) async {
  await tester.binding.setSurfaceSize(const Size(1100, 1200));
  final c = controller ?? ScheduleController();
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: Padding(
          padding: const EdgeInsets.all(20),
          child: shell
              ? SettingsShell(controller: c, initialPage: 'schedule')
              : SchedulePanel(controller: c),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  addTearDown(() async {
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await c.api.close();
    await c.replacement?.close();
    await tester.binding.setSurfaceSize(null);
  });
  return c;
}

Finder key(String value) => find.byKey(ValueKey(value));
Future<void> click(WidgetTester tester, String value) async {
  await tester.ensureVisible(key(value));
  await tester.tap(key(value));
  await tester.pumpAndSettle();
}

Future<void> fill(WidgetTester tester, String value, String text) async {
  await tester.ensureVisible(key(value));
  await tester.enterText(
    find.descendant(of: key(value), matching: find.byType(TextField)),
    text,
  );
  await tester.pump();
}

Future<void> choose(WidgetTester tester, String value, String label) async {
  await tester.ensureVisible(key(value));
  await tester.tap(key(value));
  await tester.pumpAndSettle();
  await tester.tap(find.text(label).last);
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'settings exposes the global catalog while default-off preserves history and deletion',
    (tester) async {
      final c = await mount(tester, shell: true);
      expect(find.byType(SchedulePanel), findsOneWidget);
      expect(
        tester.widget<DshButton>(key('schedule-create')).onPressed,
        isNull,
      );
      expect(
        tester.widget<DshButton>(key('schedule-history-r1')).onPressed,
        isNotNull,
      );
      expect(
        tester.widget<DshButton>(key('schedule-delete-r1')).onPressed,
        isNotNull,
      );
      expect(c.api.called('pluginInventory.setEnabled'), isEmpty);
      expect(c.api.called('schedule.retry'), isEmpty);
    },
  );
  testWidgets(
    'only the explicit toggle enables; search and status include inactive reminders',
    (tester) async {
      final c = await mount(tester);
      await click(tester, 'schedule-enabled');
      expect(c.api.called('pluginInventory.setEnabled').single['payload'], {
        'entryId': 'plugin-schedule',
        'enabled': true,
      });
      await choose(tester, 'schedule-filter', '已结束');
      expect(key('schedule-entry-r1'), findsNothing);
      expect(key('schedule-entry-r2'), findsOneWidget);
      await fill(tester, 'schedule-search', '不存在');
      expect(key('schedule-entry-r2'), findsNothing);
      await fill(tester, 'schedule-search', '旧提醒');
      expect(key('schedule-entry-r2'), findsOneWidget);
    },
  );
  for (final kind in scheduleKinds.keys) {
    testWidgets(
      'create $kind uses its single selector and the original chosen session',
      (tester) async {
        final c = await mount(
          tester,
          controller: ScheduleController()..api.enabled = true,
        );
        await click(tester, 'schedule-create');
        await fill(tester, 'schedule-title', '新提醒');
        await fill(tester, 'schedule-prompt', '检查代码');
        if (kind != 'after') {
          await choose(tester, 'schedule-kind', scheduleKinds[kind]!);
        }
        if (kind == 'at') await fill(tester, 'schedule-date', '2030-01-01');
        if (kind == 'weekly') await click(tester, 'schedule-weekday-5');
        await click(tester, 'schedule-save');
        final payload = object(
          c.api.called('schedule.create').single['payload'],
        );
        expect(payload['sessionId'], 's1');
        expect(payload['title'], '新提醒');
        expect(payload['prompt'], '检查代码');
        final selectors = payload.keys.where(
          (name) => !['sessionId', 'title', 'prompt'].contains(name),
        );
        expect(selectors, [
          kind == 'after' || kind == 'every' ? '${kind}_seconds' : kind,
        ]);
        if (kind == 'weekly') {
          expect(object(payload['weekly'])['weekdays'], [1, 5]);
        }
        if (['daily', 'weekly', 'cron', 'at'].contains(kind)) {
          expect(object(payload[kind])['time_zone'], 'Asia/Shanghai');
        }
        if (['daily', 'weekly', 'at'].contains(kind)) {
          expect(object(payload[kind])['time'], '09:00:00');
        }
        expect(c.api.called('pluginInventory.setEnabled'), isEmpty);
        expect(tester.takeException(), isNull);
      },
    );
  }
  testWidgets('ambiguous zone blocks save before dispatch', (tester) async {
    final c = await mount(
      tester,
      controller: ScheduleController()..api.enabled = true,
    );
    await click(tester, 'schedule-create');
    await fill(tester, 'schedule-title', '标题');
    await fill(tester, 'schedule-prompt', '内容');
    await choose(tester, 'schedule-kind', '每天');
    await fill(tester, 'schedule-zone', 'CST');
    await click(tester, 'schedule-save');
    expect(c.api.called('schedule.create'), isEmpty);
    expect(find.textContaining('请输入明确的 IANA 时区'), findsOneWidget);
  });
  testWidgets(
    'CAS conflict preserves draft and only explicit reload adopts a fresh expected record',
    (tester) async {
      final c = await mount(
        tester,
        controller: ScheduleController()
          ..api.enabled = true
          ..api.conflict = true,
      );
      await click(tester, 'schedule-edit-r1');
      expect(key('schedule-session'), findsNothing);
      await fill(tester, 'schedule-title', '我的草稿');
      await click(tester, 'schedule-save');
      final first = object(c.api.called('schedule.update').single['payload']);
      expect(first['sessionId'], 's2');
      expect(
        first['expected'],
        ScheduleRecord.fromJson(reminder('r1')).toJson(),
      );
      expect(first.containsKey('change'), false);
      expect(find.text('我的草稿'), findsOneWidget);
      expect(key('schedule-refresh-expected'), findsOneWidget);
      c.api.entries.first['title'] = '别人更新的标题';
      await click(tester, 'schedule-refresh-expected');
      expect(find.text('我的草稿'), findsOneWidget);
      expect(find.textContaining('别人更新的标题'), findsOneWidget);
      c.api.conflict = false;
      await click(tester, 'schedule-save');
      final last = object(c.api.called('schedule.update').last['payload']);
      expect(object(last['expected'])['title'], '别人更新的标题');
      expect(last['title'], '我的草稿');
    },
  );
  testWidgets(
    'editing an after reminder preserves its anchor unless absolute at change is explicit',
    (tester) async {
      final c = ScheduleController()..api.enabled = true;
      c.api.entries[0] =
          {...reminder('r1'), 'kind': 'after', 'afterSeconds': 30}
            ..remove('time')
            ..remove('timeZone');
      await mount(tester, controller: c);
      await click(tester, 'schedule-edit-r1');
      await fill(tester, 'schedule-title', '只改标题');
      await click(tester, 'schedule-save');
      final first = object(c.api.called('schedule.update').single['payload']);
      expect(first.containsKey('change'), false);
      expect(object(first['expected'])['kind'], 'after');
      expect(
        object(first['expected'])['scheduledAt'],
        '2030-01-01T01:00:00.000Z',
      );
      await click(tester, 'schedule-edit-r1');
      await click(tester, 'schedule-change-timing');
      await fill(tester, 'schedule-instant', '2031-02-01T09:00:00+08:00');
      await click(tester, 'schedule-save');
      final last = object(c.api.called('schedule.update').last['payload']);
      expect(last['change'], {'kind': 'at', 'at': '2031-02-01T09:00:00+08:00'});
    },
  );
  testWidgets(
    'delete requires confirmation and preserves the bound session even when another is selected',
    (tester) async {
      final c = await mount(tester);
      await click(tester, 'schedule-delete-r1');
      expect(c.api.called('schedule.delete'), isEmpty);
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      expect(c.api.called('schedule.delete'), isEmpty);
      await click(tester, 'schedule-delete-r1');
      await tester.tap(
        find.descendant(
          of: find.byType(AlertDialog),
          matching: find.widgetWithText(DshButton, '删除'),
        ),
      );
      await tester.pumpAndSettle();
      expect(c.api.called('schedule.delete').single['payload'], {
        'sessionId': 's2',
        'id': 'r1',
      });
    },
  );
  testWidgets(
    'history pages use opaque cursor and show receipt limits without claiming model success',
    (tester) async {
      final c = await mount(tester);
      await click(tester, 'schedule-history-r1');
      expect(find.textContaining('不代表模型执行成功'), findsOneWidget);
      expect(find.textContaining('更早回执已按保留策略清理'), findsOneWidget);
      expect(find.textContaining('部分更早回执不可用'), findsOneWidget);
      await click(tester, 'schedule-history-more');
      expect(
        object(c.api.called('schedule.history').last['payload'])['before'],
        'message-2',
      );
      expect(find.textContaining('最近发送内容'), findsOneWidget);
      expect(find.textContaining('更早发送内容'), findsOneWidget);
      expect(key('schedule-history-more'), findsNothing);
    },
  );
  testWidgets(
    'retention config saves while disabled and config CAS retains a conflicting draft',
    (tester) async {
      final c = await mount(tester);
      await click(tester, 'schedule-retention');
      await fill(tester, 'schedule-retention-days', '45');
      await fill(tester, 'schedule-retention-records', '350');
      c.api.configConflict = true;
      await click(tester, 'schedule-retention-save');
      expect(find.text('45'), findsOneWidget);
      expect(find.textContaining('草稿已保留'), findsOneWidget);
      c.api.configRevision = 'external-2';
      c.api.config['deliveryHistoryDays'] = 60;
      await click(tester, 'schedule-retention-reload');
      expect(find.text('45'), findsOneWidget);
      c.api.configConflict = false;
      await click(tester, 'schedule-retention-save');
      expect(c.api.called('pluginInventory.setConfig').last['payload'], {
        'entryId': 'plugin-schedule',
        'expectedRevision': 'external-2',
        'config': {'deliveryHistoryDays': 45, 'deliveryHistoryRecords': 350},
      });
      expect(c.api.called('pluginInventory.setEnabled'), isEmpty);
      expect(c.api.enabled, isFalse);
    },
  );
  testWidgets(
    'selection switch cancels an in-flight save and ignores its late success',
    (tester) async {
      final c = await mount(
        tester,
        controller: ScheduleController()..api.enabled = true,
      );
      await click(tester, 'schedule-edit-r1');
      final completion = c.api.pending['schedule.update'] =
          Completer<Object?>();
      await tester.tap(key('schedule-save'));
      await tester.pump();
      final scope = c.api.scopes.last;
      c.changeSession();
      await tester.pump();
      expect(scope.cancelled, true);
      completion.complete({
        'id': 'r1',
        'updated': true,
        'record': ScheduleRecord.fromJson(reminder('r1')).toJson(),
      });
      await tester.pumpAndSettle();
      expect(find.byType(ScheduleEditor), findsOneWidget);
      expect(find.textContaining('此草稿已停止提交'), findsOneWidget);
      expect(tester.widget<DshButton>(key('schedule-save')).onPressed, isNull);
    },
  );
  testWidgets(
    'Host switch cancels pending history and unmount ignores the late response',
    (tester) async {
      final c = await mount(tester);
      final completion = c.api.pending['schedule.history'] =
          Completer<Object?>();
      await tester.tap(key('schedule-history-r1'));
      await tester.pump();
      final scope = c.api.scopes.last;
      c.changeHost();
      await tester.pump();
      expect(scope.cancelled, true);
      await tester.pumpWidget(const SizedBox());
      completion.complete(c.api.deliveryPage(null));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'change notification refreshes the global catalog without silently enabling reminders',
    (tester) async {
      final c = await mount(tester);
      c.api.entries.first['title'] = '另一客户端修改';
      c.notifySchedules(enabled: false);
      await tester.pumpAndSettle();
      expect(find.text('另一客户端修改 · 有效'), findsOneWidget);
      expect(c.api.called('schedule.catalog').length, 2);
      expect(c.api.called('pluginInventory.setEnabled'), isEmpty);
    },
  );
  testWidgets(
    'first change notification refreshes an already open receipt page',
    (tester) async {
      final c = await mount(tester);
      await click(tester, 'schedule-history-r1');
      expect(c.api.called('schedule.history').length, 1);
      c.notifySchedules(enabled: false);
      await tester.pumpAndSettle();
      expect(c.api.called('schedule.history').length, 2);
      expect(c.api.called('pluginInventory.setEnabled'), isEmpty);
      expect(tester.takeException(), isNull);
    },
  );
  test('Host schedule-changed event reaches controller listeners and retains omitted enabled state', () async {
    final fake = FakeClient();
    final controller = DesktopController(
      MemoryPreferences(),
      clientFactory: (_) => fake,
    );
    await controller.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    fake.channels.last.data.add(
      HostFrame.fromJson({
        'type': 'server-request',
        'rpcId': 'schedule-1',
        'payload': {'type': 'host/schedule-changed', 'enabled': false},
      }),
    );
    await Future<void>.delayed(Duration.zero);
    expect(controller.scheduleRevision, 1);
    expect(controller.scheduleEnabled, false);
    fake.channels.last.data.add(
      HostFrame.fromJson({
        'type': 'server-request',
        'rpcId': 'schedule-2',
        'payload': {'type': 'host/schedule-changed'},
      }),
    );
    await Future<void>.delayed(Duration.zero);
    expect(controller.scheduleRevision, 2);
    expect(controller.scheduleEnabled, false);
    controller.dispose();
    await Future<void>.delayed(Duration.zero);
  });
}
