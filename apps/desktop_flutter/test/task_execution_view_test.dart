import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/task_execution_view.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class AcceptanceApi extends DshClient {
  AcceptanceApi() : super('http://127.0.0.1:9');
  final writes = <Json>[];
  final waiter = Completer<Json>();
  bool reject = false, wait = false;
  String state = 'awaiting_user';
  int revision = 7;
  Json record(String owner) => {
    'taskId': 'task-$owner',
    'revision': revision,
    'state': state,
    'spec': {
      'objective': '检查成品-$owner',
      'constraints': <String>[],
      'expectedOutputs': <String>[],
      'acceptanceChecks': [
        {
          'id': 'manual',
          'description': '排版符合要求',
          'checker': {'kind': 'manual', 'reason': '视觉检查'},
        },
      ],
    },
    'steps': <Json>[],
    'acceptanceResults': [
      {
        'checkId': 'manual',
        'status': 'awaiting_user',
        'inputIdentity': 'exact-file-v7',
        'evidenceRefs': <String>[],
      },
    ],
  };
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    expect(path, '/__dsh-task-execution');
    final owner = body!['sessionId'] as String;
    if (mutation) {
      writes.add({...body});
      if (reject) {
        throw DshException(
          'TRANSPORT',
          'response interrupted',
          outcomeUnknown: true,
        );
      }
      if (body['action'] == 'validate' && wait) return waiter.future;
      revision++;
      if (body['action'] == 'cancel') state = 'cancelled';
      if (body['action'] == 'resume') state = 'planned';
    }
    if (body['action'] == 'list') {
      return {
        'tasks': [record(owner)],
      };
    }
    return {
      'task': record(owner),
      'blockers': ['Manual acceptance required'],
      'recovery': <Json>[],
    };
  }
}

void main() {
  Future<void> mount(
    WidgetTester tester,
    AcceptanceApi api, {
    String session = 'a',
  }) async {
    tester.view.physicalSize = const Size(1200, 1800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: TaskExecutionView(api: api, session: session),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets(
    'manual confirmation binds exact input and ambiguous retries keep the request identity',
    (tester) async {
      final api = AcceptanceApi();
      await mount(tester, api);
      await tester.tap(find.text('核对并确认此项'));
      await tester.pumpAndSettle();
      expect(api.writes, isEmpty);
      api.reject = true;
      await tester.tap(find.text('确认验收通过'));
      await tester.pumpAndSettle();
      expect(api.writes.single['inputIdentity'], 'exact-file-v7');
      expect(api.writes.single['revision'], 7);
      api.reject = false;
      await tester.ensureVisible(find.text('使用原操作标识核实重试'));
      await tester.tap(find.text('使用原操作标识核实重试'));
      await tester.pumpAndSettle();
      expect(api.writes[1], api.writes[0]);
      expect(find.text('使用原操作标识核实重试'), findsNothing);
    },
  );
  testWidgets(
    'stop validation remains usable while request is pending and ignores its late error',
    (tester) async {
      final api = AcceptanceApi()..wait = true;
      await mount(tester, api);
      await tester.tap(find.text('重新验收'));
      await tester.pump();
      await tester.tap(find.text('停止验收'));
      await tester.pumpAndSettle();
      expect(api.writes.map((row) => row['action']), [
        'validate',
        'stop_validation',
      ]);
      api.waiter.completeError(DshException('CANCELLED', 'late-old-error'));
      await tester.pumpAndSettle();
      expect(find.textContaining('late-old-error'), findsNothing);
      expect(find.text('停止请求已处理。'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets('session replacement drops an old operation result', (
    tester,
  ) async {
    final api = AcceptanceApi()..wait = true;
    await mount(tester, api);
    await tester.tap(find.text('重新验收'));
    await tester.pump();
    await mount(tester, api, session: 'b');
    api.waiter.complete({'task': api.record('a')});
    await tester.pumpAndSettle();
    expect(find.text('检查成品-b'), findsOneWidget);
    expect(find.text('检查成品-a'), findsNothing);
    expect(api.writes.length, 1);
    expect(tester.takeException(), isNull);
  });
}
