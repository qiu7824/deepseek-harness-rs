import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/session_status.dart';
import 'package:dsh_desktop/features/conversation/session_views.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

class MutationsClient extends FakeClient {
  final mutations = <({String method, Json payload})>[];
  @override
  Future<HostInfo> describe() async => HostInfo.fromJson({
    'home': 'test',
    'version': 'test',
    'cwd': 'test',
    'supportsIdleTodoEdits': true,
  });
  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    if (mutation) mutations.add((method: method, payload: payload));
    if (method == 'session.updateTodos') return {'accepted': true};
    if (method.startsWith('goal.')) {
      return {
        'ref': {'id': object(payload['ref'])['id'], 'revision': 8},
      };
    }
    return super.call(method, payload, mutation);
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async => {'providers': []};
}

void main() {
  test('older hosts cannot receive unsafe todo mutations', () async {
    final api = MutationsClient(),
        c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    c.host = HostInfo.fromJson({
      'home': 'old',
      'version': 'same',
      'cwd': 'test',
    });
    c.selectedId = 'session';
    expect(c.canEditTodos, isFalse);
    await expectLater(
      c.updateTodos([], {'kind': 'remove', 'index': 0}),
      throwsStateError,
    );
    expect(api.mutations, isEmpty);
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });
  testWidgets(
    'goal controls remain visible with collapsed todos and hide on completion',
    (tester) async {
      final c = DesktopController(MemoryPreferences());
      final goal = <String, dynamic>{
        'id': 'g',
        'revision': 1,
        'phase': 'paused',
        'objective': '验证长目标在狭窄面板中截断而不挤出操作按钮' * 10,
      };
      c.projectionWindow.apply('goal', {'goal': goal, 'roundsStarted': 2}, 1);
      c.projectionWindow.apply('todos', [
        {'content': '核对状态', 'status': 'pending'},
      ], 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(
              child: SizedBox(width: 360, child: ProgressDock(controller: c)),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byTooltip('恢复目标'), findsOneWidget);
      expect(find.text('核对状态'), findsNothing);
      expect(
        tester.getSize(find.byKey(const ValueKey('goal-status-bar'))).height,
        36,
      );
      expect(tester.takeException(), isNull);
      c.projectionWindow.apply('goal', {
        'goal': {...goal, 'phase': 'complete'},
      }, 2);
      c.projectionChanges.value++;
      await tester.pumpAndSettle();
      expect(find.byTooltip('编辑目标'), findsNothing);
      expect(find.byKey(const ValueKey('todo-disclosure')), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
  test('permission choices retain only server-provided mode identifiers', () {
    final c = DesktopController(MemoryPreferences());
    c.projectionWindow.apply('permissions', {
      'currentValue': 'workspace-write',
      'options': [
        {'value': 'workspace-write', 'name': 'workspace-write'},
        {'value': 'danger-full-access', 'name': 'danger-full-access'},
      ],
    }, 1);
    expect(c.permissionChoices, {
      'workspace-write': '工作区内修改',
      'danger-full-access': '完全访问',
    });
    expect(c.permissionChoices.containsKey('full-access'), isFalse);
    expect(c.permissionChoices.containsKey('read-only'), isFalse);
    c.dispose();
  });
  test('whole-session stats keep request throughput and reported cache denominator', () {
    final text = sessionStatsText({
      'sessionStats': {
        'turns': 3,
        'steps': 9,
        'llmMs': 10000,
        'toolMs': 2000,
        'ttftMs': 4000,
        'ttftSteps': 2,
        'requestMs': 2000,
        'requestSamples': 1,
        'requestOutputTokens': 200,
        'decodeMs': 1,
        'decodeTokens': 9999,
      },
      'tokenUsage': {
        'uncachedInputTokens': 1000,
        'cacheReadTokens': 80,
        'cacheWriteTokens': 20,
        'outputTokens': 200,
        'cacheStatistics': {
          'reportedSamples': 1,
          'unreportedSamples': 2,
          'reportedInputTokens': 100,
        },
      },
    });
    expect(text, contains('3 轮 · 9 步'));
    expect(text, contains('请求平均 100 tok/s'));
    expect(text, contains('首 token 平均 2s'));
    expect(text, contains('缓存命中 80%（部分请求）'));
    expect(text, isNot(contains('9999')));
  });
  testWidgets('long session stats scroll horizontally without ellipsis', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences());
    c.projectionWindow.apply('sessionStats', {
      'turns': 1,
      'steps': 18,
      'llmMs': 360000,
      'toolMs': 21000,
      'ttftMs': 19300,
      'ttftSteps': 1,
      'requestMs': 5000,
      'requestSamples': 1,
      'requestOutputTokens': 220,
    }, 1);
    c.projectionWindow.apply('tokenUsage', {
      'uncachedInputTokens': 37000,
      'cacheReadTokens': 86000,
      'outputTokens': 4400,
    }, 1);
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(width: 640, child: SessionStatsLine(controller: c)),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final scroll = find.byKey(const ValueKey('session-stats-scroll'));
    expect(scroll, findsOneWidget);
    expect(find.textContaining('输出 4.4K tok'), findsOneWidget);
    final state = tester.state<ScrollableState>(
      find.descendant(of: scroll, matching: find.byType(Scrollable)),
    );
    expect(state.position.maxScrollExtent, greaterThan(0));
    await tester.drag(scroll, const Offset(-400, 0));
    await tester.pumpAndSettle();
    expect(state.position.pixels, greaterThan(0));
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });
  test(
    'todo and goal updates carry the exact compare-and-swap identity',
    () async {
      final api = MutationsClient(),
          c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
      await c.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      await c.select('session');
      final todos = <Json>[
        {'content': '核对状态', 'status': 'pending'},
      ];
      await c.updateTodos(todos, {
        'kind': 'edit',
        'index': 0,
        'content': '核对投影',
      });
      expect(api.mutations.last.payload, {
        'sessionId': 'session',
        'expected': todos,
        'action': {'kind': 'edit', 'index': 0, 'content': '核对投影'},
      });
      await c.changeGoal('pause', {'id': 'goal-a', 'revision': 7});
      expect(api.mutations.last.method, 'goal.pause');
      expect(api.mutations.last.payload, {
        'sessionId': 'session',
        'ref': {'id': 'goal-a', 'revision': 7},
      });
      c.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );
  testWidgets(
    'context meter uses projected occupancy and closes when it becomes unavailable',
    (tester) async {
      final c = DesktopController(MemoryPreferences());
      c.projectionWindow.apply('contextPressure', {
        'projectedTokens': 20,
        'pressureTokens': 80,
        'contextWindow': 100,
      }, 1);
      c.projectionWindow.apply('contextBreakdown', {
        'systemTokens': 2,
        'toolsTokens': 3,
        'messageTokens': 15,
      }, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(child: ContextMeter(controller: c)),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byTooltip('上下文已用 20%'), findsOneWidget);
      await tester.tap(find.byType(CircularProgressIndicator));
      await tester.pumpAndSettle();
      expect(find.text('系统提示词'), findsOneWidget);
      c.projectionWindow.apply('contextPressure', null, 2);
      c.projectionChanges.value++;
      await tester.pumpAndSettle();
      c.projectionWindow.apply('contextPressure', {
        'projectedTokens': 0,
        'contextWindow': 100,
      }, 3);
      c.projectionChanges.value++;
      await tester.pumpAndSettle();
      expect(find.byTooltip('上下文已用 0%'), findsOneWidget);
      expect(find.text('系统提示词'), findsNothing);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
  testWidgets('context inspector does not invent usage or cost when absent', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences());
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: ContextView(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('未提供'), findsWidgets);
    expect(find.textContaining('USD 0'), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });
  testWidgets('rejected projection edit retains the entered draft', (
    tester,
  ) async {
    await tester.pumpWidget(
      ShadApp(
        home: ProjectionTextEditor(
          title: '编辑任务',
          initial: '原任务',
          onSave: (_) async {
            throw DshException('conflict', '任务列表已更新');
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), '保留输入的修订');
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(find.text('保留输入的修订'), findsOneWidget);
    expect(find.textContaining('任务列表已更新'), findsOneWidget);
    expect(find.byType(ProjectionTextEditor), findsOneWidget);
  });
  testWidgets('large todo lists build only visible rows', (tester) async {
    final c = DesktopController(MemoryPreferences());
    c.projectionWindow.apply(
      'todos',
      List.generate(5000, (i) => {'content': '任务 $i', 'status': 'pending'}),
      1,
    );
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(width: 600, child: ProgressDock(controller: c)),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('todo-disclosure')));
    await tester.pumpAndSettle();
    expect(find.text('任务 0'), findsOneWidget);
    expect(find.text('任务 4999'), findsNothing);
    expect(find.byType(Text).evaluate().length, lessThan(40));
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });
}
