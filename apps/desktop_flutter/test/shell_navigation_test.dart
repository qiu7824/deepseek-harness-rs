import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/knowledge/knowledge_page.dart';
import 'package:dsh_desktop/features/schedule/schedule_page.dart' as schedule;
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

class _Client extends DshClient {
  _Client() : super('http://127.0.0.1:9');

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    if (path == '/__dsh-schedule/catalog') {
      return {'tasks': <Json>[], 'revision': 0, 'hostTimeZone': 'UTC'};
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
    if (path == '/__dsh-knowledge/catalog') return {'bases': <Json>[]};
    throw StateError('Unexpected request: $path');
  }
}

class _Controller extends DesktopController {
  _Controller() : super(DesktopPreferences(writer: (_) async {}));
  final api = _Client();
  @override
  DshClient get client => api;
  @override
  bool get connected => true;
  @override
  Future<void> select(
    String id, {
    bool adoptDraft = false,
    bool loadModels = true,
  }) async {
    selectedId = id;
    emit();
  }
}

Future<_Controller> _mount(
  WidgetTester tester, {
  bool collapsed = false,
}) async {
  await tester.binding.setSurfaceSize(const Size(1280, 800));
  final controller = _Controller()
    ..workspaceId = 'workspace'
    ..selectedId = 'history'
    ..workspaces = [
      {
        'workspaceId': 'workspace',
        'path': r'E:\project',
        'title': '工作区',
        'sessionIds': ['history'],
      },
    ]
    ..sessions = [
      SessionSummary.fromJson({
        'sessionId': 'history',
        'cwd': r'E:\project',
        'displayTitle': '历史会话',
        'blank': true,
      }),
    ];
  controller.preferences.layout['sideOpen'] = !collapsed;
  controller.preferences.layout['sidebarWidth'] = 220.0;
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

void main() {
  for (final collapsed in [false, true]) {
    testWidgets('global pages fill the workbench (collapsed: $collapsed)', (
      tester,
    ) async {
      await _mount(tester, collapsed: collapsed);
      await tester.tap(find.byKey(const Key('open-schedule-direct')));
      await tester.pumpAndSettle();
      final tasks = find.byType(schedule.SchedulePage);
      expect(tester.getSize(tasks).width, greaterThan(800));
      expect(tester.getSize(tasks).height, greaterThan(600));
      expect(find.text('新建任务').hitTestable(), findsOneWidget);
      await tester.tap(find.byKey(const Key('open-knowledge')));
      await tester.pumpAndSettle();
      final knowledge = find.byType(KnowledgePage);
      expect(tester.getSize(knowledge).width, greaterThan(800));
      expect(find.text('新建知识库').hitTestable(), findsOneWidget);
      await tester.tap(find.text('新建知识库'));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const Key('knowledge-create-name')).hitTestable(),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('selecting the current history leaves a global page', (
    tester,
  ) async {
    final controller = await _mount(tester);
    await tester.tap(find.byKey(const Key('open-knowledge')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('session-history')));
    await tester.pumpAndSettle();
    expect(controller.selectedId, 'history');
    expect(find.byType(KnowledgePage), findsNothing);
    expect(find.byType(Conversation), findsOneWidget);
    expect(find.byKey(const Key('prompt-input')).hitTestable(), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('new conversation leaves a global page', (tester) async {
    final controller = await _mount(tester);
    controller.workspaceId = null;
    await tester.tap(find.byKey(const Key('open-schedule-direct')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('new-task')));
    await tester.pumpAndSettle();
    expect(find.byType(schedule.SchedulePage), findsNothing);
    expect(find.byType(Conversation), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
