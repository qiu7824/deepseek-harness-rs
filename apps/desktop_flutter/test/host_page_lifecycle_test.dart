import 'dart:async';

import 'package:dsh_client/dsh_client.dart' hide ScheduleApi;
import 'package:dsh_desktop/features/knowledge/knowledge_page.dart';
import 'package:dsh_desktop/features/schedule/schedule_page.dart' as schedule;
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class _Preferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class _Controller extends DesktopController {
  _Controller(this.active) : super(_Preferences());
  DshClient? active;
  @override
  DshClient? get client => active;
  @override
  bool get connected => active != null;
}

class _HostClient extends DshClient {
  _HostClient(int port) : super('http://127.0.0.1:$port');
  final scheduleCatalog = Completer<Json>(),
      knowledgeCatalog = Completer<Json>();
  final calls = <(String, RequestScope?)>[];

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) {
    calls.add((path, scope));
    if (path == '/__dsh-schedule/catalog') return scheduleCatalog.future;
    if (path == '/__dsh-knowledge/catalog') return knowledgeCatalog.future;
    if (path == '/__dsh-knowledge/documents') {
      return Future.value({'documents': <Json>[]});
    }
    if (path == '/__dsh-schedule/wait') {
      final pending = Completer<Json>();
      scope?.register(() {
        if (!pending.isCompleted) {
          pending.completeError(DshException('cancelled', 'closed'));
        }
      });
      return pending.future;
    }
    throw StateError('Unexpected request: $path');
  }
}

Json _task(String title) => {
  'id': 'shared-task',
  'sessionId': 'session',
  'title': title,
  'prompt': 'Run task',
  'rule': {'kind': 'daily', 'time': '09:00', 'timeZone': 'UTC'},
  'status': 'active',
  'origin': 'agent',
  'updatedAt': '2026-09-27T00:00:00.000Z',
  'historyCount': 0,
};

Json _base(String name) => {
  'id': 'shared-base',
  'name': name,
  'description': '',
  'enabled': true,
  'documentCount': 0,
  'chunkCount': 0,
  'updatedAt': '1',
};

class _ImportApi extends KnowledgeApi {
  _ImportApi() : super(DshClient('http://127.0.0.1:9'));
  final result = Completer<Json>();
  final imported = <String>[];
  RequestScope? importScope;
  @override
  Future<Json> call(
    String operation, [
    Json body = const {},
    RequestScope? scope,
  ]) {
    if (operation == 'documents') return Future.value({'documents': <Json>[]});
    if (operation == 'importPath') {
      imported.add('${body['path']}');
      importScope = scope;
      return result.future;
    }
    throw StateError(operation);
  }
}

class _RunApi extends schedule.ScheduleApi {
  _RunApi() : super(DshClient('http://127.0.0.1:9'));
  final result = Completer<Json>();
  RequestScope? runScope;
  @override
  Future<Json> call(
    String operation, [
    Json body = const {},
    RequestScope? scope,
  ]) {
    if (operation != 'runNow') throw StateError(operation);
    runScope = scope;
    return result.future;
  }
}

void main() {
  testWidgets(
    'scheduled-task watcher and fetch move together to the new Host',
    (tester) async {
      final oldHost = _HostClient(8), newHost = _HostClient(9);
      final controller = _Controller(oldHost);
      try {
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: schedule.SchedulePage(
                controller: controller,
                onClose: () {},
                onOpenSession: (_) {},
              ),
            ),
          ),
        );
        expect(oldHost.calls.single.$1, '/__dsh-schedule/catalog');
        newHost.scheduleCatalog.complete({
          'revision': 2,
          'tasks': [_task('新 Host 任务')],
          'hostTimeZone': 'UTC',
          'deliveryError': 'receipt write failed',
        });
        controller.active = newHost;
        controller.emit();
        await tester.pumpAndSettle();
        expect(oldHost.calls.single.$2!.cancelled, true);
        expect(
          newHost.calls.map((call) => call.$1),
          contains('/__dsh-schedule/wait'),
        );
        oldHost.scheduleCatalog.complete({
          'revision': 1,
          'tasks': [_task('旧 Host 任务')],
          'hostTimeZone': 'UTC',
        });
        await tester.pumpAndSettle();
        expect(find.text('旧 Host 任务'), findsNothing);
        expect(find.text('新 Host 任务'), findsOneWidget);
        expect(find.textContaining('投递或回执未能持久化'), findsOneWidget);
        expect(oldHost.calls, hasLength(1));
        expect(tester.takeException(), isNull);
      } finally {
        await tester.pumpWidget(const SizedBox());
        controller.dispose();
        await oldHost.close();
        await newHost.close();
      }
    },
  );

  testWidgets(
    'knowledge catalog ignores a late response from the previous Host',
    (tester) async {
      final oldHost = _HostClient(8), newHost = _HostClient(9);
      final controller = _Controller(oldHost);
      try {
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: KnowledgePage(controller: controller, onClose: () {}),
            ),
          ),
        );
        newHost.knowledgeCatalog.complete({
          'bases': [_base('新知识库')],
          'extensions': ['md'],
        });
        controller.active = newHost;
        controller.emit();
        await tester.pumpAndSettle();
        expect(oldHost.calls.single.$2!.cancelled, true);
        oldHost.knowledgeCatalog.complete({
          'bases': [_base('旧知识库')],
          'extensions': ['md'],
        });
        await tester.pumpAndSettle();
        expect(find.text('旧知识库'), findsNothing);
        expect(find.text('新知识库'), findsWidgets);
        expect(tester.takeException(), isNull);
      } finally {
        await tester.pumpWidget(const SizedBox());
        controller.dispose();
        await oldHost.close();
        await newHost.close();
      }
    },
  );

  testWidgets(
    'late import cannot continue a batch against a replacement base API',
    (tester) async {
      final oldApi = _ImportApi(), newApi = _ImportApi();
      var changed = 0;
      Future<void> show(KnowledgeApi api, String name) => tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: KnowledgeBaseDetail(
              key: const Key('detail'),
              base: _base(name),
              api: api,
              extensions: const ['md'],
              onChanged: () async {
                changed++;
              },
              onDeleted: () {},
              onClose: () {},
            ),
          ),
        ),
      );
      try {
        await show(oldApi, '旧知识库');
        await tester.pumpAndSettle();
        final state = tester.state<KnowledgeBaseDetailState>(
          find.byType(KnowledgeBaseDetail),
        );
        final importing = state.importPaths(['first.md', 'second.md']);
        await tester.pump();
        expect(oldApi.imported, ['first.md']);
        await show(newApi, '新知识库');
        await tester.pumpAndSettle();
        expect(oldApi.importScope!.cancelled, true);
        oldApi.result.complete({
          'report': {
            'added': [<String, dynamic>{}],
            'skipped': [],
          },
        });
        await importing;
        await tester.pumpAndSettle();
        expect(oldApi.imported, ['first.md']);
        expect(newApi.imported, isEmpty);
        expect(changed, 0);
        expect(find.textContaining('已导入'), findsNothing);
        expect(tester.takeException(), isNull);
      } finally {
        await tester.pumpWidget(const SizedBox());
        await oldApi.client.close();
        await newApi.client.close();
      }
    },
  );

  testWidgets(
    'closing task details cancels its request and ignores a late run receipt',
    (tester) async {
      final api = _RunApi();
      var changed = 0;
      try {
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: schedule.ScheduleTaskDetail(
                task: _task('任务'),
                api: api,
                hostZone: 'UTC',
                sessionTitle: '会话',
                sessionKnown: true,
                onChanged: () async {
                  changed++;
                },
                onDeleted: () {},
                onOpenSession: () {},
                onClose: () {},
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        await tester.tap(find.byKey(const Key('schedule-run-now')));
        await tester.pump();
        expect(api.runScope, isNotNull);
        await tester.pumpWidget(const SizedBox());
        expect(api.runScope!.cancelled, true);
        api.result.complete({'delivered': true});
        await tester.pump();
        expect(changed, 0);
        expect(tester.takeException(), isNull);
      } finally {
        await tester.pumpWidget(const SizedBox());
        await api.client.close();
      }
    },
  );
}
