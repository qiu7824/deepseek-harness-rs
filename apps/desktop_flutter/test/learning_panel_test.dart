import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/learning_panel.dart';
import 'package:dsh_desktop/features/settings/resource_page.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class LearningClient extends DshClient {
  LearningClient() : super('http://127.0.0.1');
  final calls = <({String method, Json payload, bool mutation})>[];
  final scopes = <RequestScope>[];
  bool enabled = true, memoryEnabled = true;
  String? failMethod;
  final pending = <String, Completer<Json>>{};
  final rows = <Json>[
    {
      'id': 'rule',
      'tool': 'edit_file',
      'status': 'pending',
      'reusableRule': true,
      'enabled': true,
      'revision': 3,
      'suggestion': '先读取目标文件。',
      'occurrences': 2,
      'applicationCount': 0,
      'workspaceLabel': '项目甲',
    },
    {
      'id': 'diagnostic',
      'tool': 'shell',
      'status': 'pending',
      'reusableRule': false,
      'enabled': true,
      'revision': 9,
      'code': 'COMMAND_NOT_FOUND',
      'message': '程序未安装',
      'review': {
        'summary': '检查运行环境',
        'evidence': ['session:a/call:1'],
      },
    },
  ];

  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    calls.add((method: method, payload: payload, mutation: mutation));
    if (scope != null) scopes.add(scope);
    if (method == failMethod) throw DshException('conflict', '经验已更新，请重新读取');
    switch (method) {
      case 'memory.learningList':
        final query = '${payload['query'] ?? ''}';
        final filtered = rows
            .where(
              (row) =>
                  (payload['status'] == null ||
                      row['status'] == payload['status']) &&
                  '$row'.contains(query),
            )
            .toList();
        return {
          'enabled': enabled,
          'memoryEnabled': memoryEnabled,
          'effectiveEnabled': enabled && memoryEnabled,
          'revision': 12,
          'items': filtered,
          'total': filtered.length,
        };
      case 'memory.learningPreview':
        final id = '${payload['sessionId']}';
        return pending[id]?.future ?? Future.value(preview(id));
      case 'memory.learningConfigure':
        enabled = payload['enabled'] == true;
        return {};
      case 'memory.learningToggle':
        rows.firstWhere((row) => row['id'] == payload['id'])['enabled'] =
            payload['enabled'];
        return {};
      case 'memory.learningRemove':
        rows.removeWhere((row) => row['id'] == payload['id']);
        return {};
      case 'memory.learningConfirm':
        rows.firstWhere((row) => row['id'] == payload['id'])['status'] =
            'verified';
        return {};
      case 'memory.list':
        return {'entries': []};
      default:
        return {};
    }
  }

  Json preview(String session) => {
    'sessionId': session,
    'items': [
      {'id': 'rule', 'tool': 'edit_file', 'suggestion': '$session 的候选经验'},
    ],
    'text': '$session 的候选上下文',
    'notice': '实际请求会重新核对。',
    'usedCharacters': 12,
    'budget': 4096,
  };
}

class LearningController extends DesktopController {
  LearningController() : super(DesktopPreferences()) {
    selectedId = 'first';
  }
  final api = LearningClient();
  LearningClient? replacement;
  @override
  DshClient get client => replacement ?? api;
  void changeSession(String id) {
    selectedId = id;
    emit();
  }

  void changeHost() {
    replacement = LearningClient();
    emit();
  }
}

Future<LearningController> mount(
  WidgetTester tester, {
  LearningController? controller,
  bool page = false,
}) async {
  await tester.binding.setSurfaceSize(const Size(1100, 1200));
  final c = controller ?? LearningController();
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: Padding(
          padding: const EdgeInsets.all(20),
          child: page
              ? SettingsResourcePage(controller: c, page: 'memory')
              : SingleChildScrollView(child: LearningPanel(controller: c)),
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

void main() {
  testWidgets('preview failures do not hide the editable learning ledger', (
    tester,
  ) async {
    final controller = LearningController()
      ..api.failMethod = 'memory.learningPreview';
    final c = await mount(tester, controller: controller);
    expect(find.byKey(const ValueKey('learning-entry-rule')), findsOneWidget);
    await tester.tap(find.text('当前任务候选经验'));
    await tester.pumpAndSettle();
    expect(find.textContaining('经验已更新'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('learning-enabled')));
    await tester.pumpAndSettle();
    expect(c.api.enabled, isFalse);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'closing the panel cancels pending reads and ignores late results',
    (tester) async {
      final c = await mount(tester);
      final pending = c.api.pending['slow'] = Completer<Json>();
      c.changeSession('slow');
      await tester.pump();
      final scope = c.api.scopes.last;
      expect(scope.cancelled, isFalse);
      await tester.pumpWidget(const SizedBox());
      expect(scope.cancelled, isTrue);
      pending.complete(c.api.preview('slow'));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'memory settings expose automatic learning alongside manual memory',
    (tester) async {
      final c = await mount(tester, page: true);
      expect(find.text('自动经验'), findsOneWidget);
      expect(
        c.api.calls.any((call) => call.method == 'memory.learningList'),
        isTrue,
      );
      expect(find.text('当前任务候选经验'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'learning mutations use configuration and entry revisions separately',
    (tester) async {
      final c = await mount(tester);
      await tester.tap(find.byKey(const ValueKey('learning-enabled')));
      await tester.pumpAndSettle();
      expect(
        c.api.calls
            .lastWhere((call) => call.method == 'memory.learningConfigure')
            .payload,
        {'enabled': false, 'expectedRevision': 12},
      );
      await tester.tap(find.byKey(const ValueKey('learning-toggle-rule')));
      await tester.pumpAndSettle();
      final toggle = c.api.calls.lastWhere(
        (call) => call.method == 'memory.learningToggle',
      );
      expect(toggle.payload, {
        'id': 'rule',
        'enabled': false,
        'expectedRevision': 3,
      });
      expect(toggle.mutation, isTrue);
      expect(
        find.byKey(const ValueKey('learning-toggle-diagnostic')),
        findsNothing,
      );
      expect(find.text('确认修正建议'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'confirmation conflicts keep the suggestion draft and original revision',
    (tester) async {
      final c = await mount(tester);
      c.api.failMethod = 'memory.learningConfirm';
      await tester.tap(find.text('确认修正建议'));
      await tester.pumpAndSettle();
      final field = find.descendant(
        of: find.byType(LearningSuggestionEditor),
        matching: find.byType(TextField),
      );
      await tester.enterText(field, '读取文件并确认唯一匹配后再编辑。');
      await tester.tap(find.text('确认此建议'));
      await tester.pumpAndSettle();
      expect(find.byType(LearningSuggestionEditor), findsOneWidget);
      expect(find.text('读取文件并确认唯一匹配后再编辑。'), findsOneWidget);
      expect(find.textContaining('经验已更新'), findsOneWidget);
      final request = c.api.calls.lastWhere(
        (call) => call.method == 'memory.learningConfirm',
      );
      expect(request.payload['expectedRevision'], 3);
      expect(request.payload['confirmed'], isTrue);
      c.api.failMethod = null;
      await tester.tap(find.text('确认此建议'));
      await tester.pumpAndSettle();
      expect(find.byType(LearningSuggestionEditor), findsNothing);
      expect(find.text('edit_file · 已验证'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('entry deletion requires confirmation and refreshes the ledger', (
    tester,
  ) async {
    final c = await mount(tester);
    await tester.tap(find.byKey(const ValueKey('learning-remove-rule')));
    await tester.pumpAndSettle();
    expect(
      c.api.calls.where((call) => call.method == 'memory.learningRemove'),
      isEmpty,
    );
    await tester.tap(find.text('删除'));
    await tester.pumpAndSettle();
    expect(
      c.api.calls
          .lastWhere((call) => call.method == 'memory.learningRemove')
          .payload,
      {'id': 'rule', 'expectedRevision': 3},
    );
    expect(find.byKey(const ValueKey('learning-entry-rule')), findsNothing);
    expect(
      find.byKey(const ValueKey('learning-entry-diagnostic')),
      findsOneWidget,
    );
  });

  testWidgets('search is debounced and passed to the Host', (tester) async {
    final c = await mount(tester);
    final count = c.api.calls
        .where((call) => call.method == 'memory.learningList')
        .length;
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('learning-search')),
        matching: find.byType(TextField),
      ),
      'COMMAND_NOT_FOUND',
    );
    await tester.pump(const Duration(milliseconds: 100));
    expect(
      c.api.calls.where((call) => call.method == 'memory.learningList'),
      hasLength(count),
    );
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    expect(
      c.api.calls
          .lastWhere((call) => call.method == 'memory.learningList')
          .payload['query'],
      'COMMAND_NOT_FOUND',
    );
    expect(find.byKey(const ValueKey('learning-entry-rule')), findsNothing);
    expect(
      find.byKey(const ValueKey('learning-entry-diagnostic')),
      findsOneWidget,
    );
  });

  testWidgets('late preview cannot replace the current task preview', (
    tester,
  ) async {
    final c = await mount(tester);
    c.api.pending['slow'] = Completer<Json>();
    c.changeSession('slow');
    await tester.pump();
    c.changeSession('second');
    await tester.pumpAndSettle();
    c.api.pending['slow']!.complete(c.api.preview('slow'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('当前任务候选经验'));
    await tester.pumpAndSettle();
    expect(find.textContaining('second 的候选经验'), findsOneWidget);
    expect(find.textContaining('slow 的候选经验'), findsNothing);
    await tester.tap(find.text('查看候选上下文内容'));
    await tester.pumpAndSettle();
    expect(find.text('second 的候选上下文'), findsOneWidget);
    expect(
      c.api.calls
          .where((call) => call.method == 'memory.learningPreview')
          .every((call) => !call.mutation),
      isTrue,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('a changed Host cannot receive an open confirmation draft', (
    tester,
  ) async {
    final c = await mount(tester);
    await tester.tap(find.text('确认修正建议'));
    await tester.pumpAndSettle();
    c.changeHost();
    await tester.pump();
    await tester.tap(find.text('确认此建议'));
    await tester.pumpAndSettle();
    expect(find.byType(LearningSuggestionEditor), findsOneWidget);
    expect(find.textContaining('连接已变化'), findsWidgets);
    expect(c.replacement!.calls, isEmpty);
    expect(
      c.api.calls.where((call) => call.method == 'memory.learningConfirm'),
      isEmpty,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('disabled memory prevents automatic learning enablement', (
    tester,
  ) async {
    final controller = LearningController()..api.memoryEnabled = false;
    await mount(tester, controller: controller);
    expect(
      tester
          .widget<SwitchListTile>(
            find.byKey(const ValueKey('learning-enabled')),
          )
          .onChanged,
      isNull,
    );
    expect(find.textContaining('持久记忆已关闭'), findsOneWidget);
    await tester.binding.setSurfaceSize(const Size(420, 700));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });
}
