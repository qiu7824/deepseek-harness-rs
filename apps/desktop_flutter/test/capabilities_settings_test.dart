import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/resource_page.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class CapabilitiesClient extends DshClient {
  CapabilitiesClient() : super('http://127.0.0.1');
  final calls = <({String method, Json payload, bool mutation})>[];
  final servers = <Json>[
    {
      'name': 'remote',
      'transport': 'http',
      'endpoint': 'https://example.com/mcp',
      'enabled': false,
      'hasSecrets': true,
      'status': 'disabled',
    },
  ];
  final skills = <Json>[
    {'name': 'managed-skill', 'managed': true, 'enabled': true},
    {'name': 'builtin-skill', 'managed': false, 'enabled': true},
  ];
  String? failMethod;
  int revision = 7;
  bool changeRevisionOnRead = false;
  Json testResult = {'status': 'tested', 'toolCount': 4, 'enabled': false};
  Completer<Json>? saveResult;

  Future<Json> invoke(String method, Json payload, bool mutation) async {
    calls.add((method: method, payload: payload, mutation: mutation));
    if (method == failMethod) {
      throw DshException('conflict', '配置已被其他窗口修改');
    }
    if (method == 'capabilities.skillRead' && changeRevisionOnRead) revision++;
    return switch (method) {
      'capabilities.list' => {
        'revision': revision,
        'skills': skills,
        'servers': servers,
      },
      'capabilities.skillRead' => {'content': 'original skill body'},
      'capabilities.serverSave' =>
        saveResult == null ? {'status': 'disabled'} : await saveResult!.future,
      'capabilities.serverTest' => testResult,
      _ => {},
    };
  }

  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) => invoke(method, payload, mutation);

  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) => invoke(method, payload, mutation);
}

class CapabilitiesController extends DesktopController {
  CapabilitiesController() : super(DesktopPreferences());
  final api = CapabilitiesClient();
  CapabilitiesClient? replacement;
  @override
  DshClient get client => replacement ?? api;
  void reconnect() {
    replacement = CapabilitiesClient();
    notifyListeners();
  }
}

Finder mcpField(String label) => find.descendant(
  of: find.byKey(ValueKey('mcp-$label')),
  matching: find.byType(TextField),
);

Future<CapabilitiesController> mountPage(WidgetTester tester) async {
  await tester.binding.setSurfaceSize(const Size(1100, 950));
  final controller = CapabilitiesController();
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: Padding(
          padding: const EdgeInsets.all(24),
          child: SettingsResourcePage(controller: controller, page: 'skills'),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  addTearDown(() async {
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
    await controller.api.close();
    await controller.replacement?.close();
    await tester.binding.setSurfaceSize(null);
  });
  return controller;
}

void main() {
  testWidgets('skill editing keeps the revision from before the content read', (
    tester,
  ) async {
    final c = await mountPage(tester);
    c.api.changeRevisionOnRead = true;
    await tester.tap(find.byTooltip('编辑技能'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(c.api.revision, 8);
    expect(
      c.api.calls
          .lastWhere((row) => row.method == 'capabilities.skillSave')
          .payload['expectedRevision'],
      7,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('MCP controls and editor stay usable in a narrow window', (
    tester,
  ) async {
    await mountPage(tester);
    await tester.binding.setSurfaceSize(const Size(420, 580));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('添加服务器'));
    await tester.pumpAndSettle();
    await tester.enterText(mcpField('名称'), 'compact');
    await tester.enterText(mcpField('可执行程序'), 'python');
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(find.byType(McpServerDialog), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('MCP drafts cannot be submitted to a changed Host connection', (
    tester,
  ) async {
    final c = await mountPage(tester);
    await tester.tap(find.byTooltip('编辑 MCP 服务器'));
    await tester.pumpAndSettle();
    await tester.enterText(
      mcpField('请求头（JSON 对象）'),
      '{"Authorization":"secret"}',
    );
    c.reconnect();
    await tester.pump();
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(find.byType(McpServerDialog), findsOneWidget);
    expect(find.textContaining('连接已变化'), findsWidgets);
    expect(
      c.api.calls.where((row) => row.method == 'capabilities.serverSave'),
      isEmpty,
    );
    expect(c.replacement!.calls, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'adding an existing MCP name cannot overwrite its configuration',
    (tester) async {
      final c = await mountPage(tester);
      await tester.tap(find.text('添加服务器'));
      await tester.pumpAndSettle();
      await tester.enterText(mcpField('名称'), 'remote');
      await tester.enterText(mcpField('可执行程序'), 'python');
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(find.textContaining('同名 MCP 服务器已存在'), findsOneWidget);
      expect(find.byType(McpServerDialog), findsOneWidget);
      expect(
        c.api.calls.where((row) => row.method == 'capabilities.serverSave'),
        isEmpty,
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'HTTP MCP edits retain secrets and preserve the draft on conflict',
    (tester) async {
      final c = await mountPage(tester);
      c.api.failMethod = 'capabilities.serverSave';
      await tester.tap(find.byTooltip('编辑 MCP 服务器'));
      await tester.pumpAndSettle();
      expect(tester.widget<TextField>(mcpField('名称')).enabled, isFalse);
      await tester.enterText(mcpField('服务器地址'), 'https://example.com/new-mcp');
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(find.byType(McpServerDialog), findsOneWidget);
      expect(find.textContaining('配置已被其他窗口修改'), findsOneWidget);
      expect(find.text('https://example.com/new-mcp'), findsOneWidget);
      final request = c.api.calls.lastWhere(
        (row) => row.method == 'capabilities.serverSave',
      );
      expect(request.mutation, isTrue);
      expect(request.payload['expectedRevision'], 7);
      final server = object(request.payload['server']);
      expect(server['transport'], 'http');
      expect(server['name'], 'remote');
      expect(server.containsKey('env'), isFalse);
      expect(server.containsKey('headers'), isFalse);
      c.api.failMethod = null;
      await tester.enterText(mcpField('请求头（JSON 对象）'), '{}');
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(find.byType(McpServerDialog), findsNothing);
      final cleared = object(
        c.api.calls
            .lastWhere((row) => row.method == 'capabilities.serverSave')
            .payload['server'],
      );
      expect(cleared['headers'], isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'stdio MCP validates parameters and submits environment and cwd',
    (tester) async {
      final c = await mountPage(tester);
      await tester.tap(find.text('添加服务器'));
      await tester.pumpAndSettle();
      await tester.enterText(mcpField('名称'), 'local-tools');
      await tester.enterText(mcpField('可执行程序'), 'python');
      await tester.enterText(mcpField('参数（JSON 数组）'), '[42]');
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(find.textContaining('参数必须为字符串'), findsOneWidget);
      expect(
        c.api.calls.where((row) => row.method == 'capabilities.serverSave'),
        isEmpty,
      );
      await tester.enterText(mcpField('参数（JSON 数组）'), '["server.py", "中文参数"]');
      await tester.enterText(mcpField('工作目录'), 'D:/workspace');
      await tester.enterText(mcpField('环境变量（JSON 对象）'), '{"TOKEN":42}');
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(find.textContaining('环境变量必须为字符串'), findsOneWidget);
      await tester.enterText(
        mcpField('环境变量（JSON 对象）'),
        '{"TOKEN":"local-test"}',
      );
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      final server = object(
        c.api.calls
            .lastWhere((row) => row.method == 'capabilities.serverSave')
            .payload['server'],
      );
      expect(server['args'], ['server.py', '中文参数']);
      expect(server['cwd'], 'D:/workspace');
      expect(server['env'], {'TOKEN': 'local-test'});
      expect(server['enabled'], isFalse);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'MCP save prevents duplicate submission while the request is pending',
    (tester) async {
      final c = await mountPage(tester);
      c.api.saveResult = Completer<Json>();
      await tester.tap(find.byTooltip('编辑 MCP 服务器'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('保存'));
      await tester.pump();
      expect(find.text('保存中…'), findsOneWidget);
      await tester.tap(find.text('保存中…'), warnIfMissed: false);
      await tester.pump();
      expect(
        c.api.calls.where((row) => row.method == 'capabilities.serverSave'),
        hasLength(1),
      );
      c.api.saveResult!.complete({'status': 'disabled'});
      await tester.pumpAndSettle();
      expect(find.byType(McpServerDialog), findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('MCP test reports returned failures and successful tool counts', (
    tester,
  ) async {
    final c = await mountPage(tester);
    c.api.testResult = {'status': 'error', 'error': '连接被拒绝'};
    await tester.tap(find.text('测试'));
    await tester.pumpAndSettle();
    expect(find.textContaining('连接被拒绝'), findsOneWidget);
    expect(find.textContaining('连接成功'), findsNothing);
    c.api.testResult = {'status': 'tested', 'toolCount': 4, 'enabled': false};
    await tester.tap(find.text('测试'));
    await tester.pumpAndSettle();
    expect(find.text('remote 连接成功，可用工具 4 个'), findsOneWidget);
    expect(find.textContaining('连接被拒绝'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'MCP and managed skill removal use the revision and confirmation',
    (tester) async {
      final c = await mountPage(tester);
      expect(find.byTooltip('移除技能'), findsOneWidget);
      await tester.tap(find.byTooltip('移除 MCP 服务器'));
      await tester.pumpAndSettle();
      expect(
        c.api.calls.where((row) => row.method == 'capabilities.serverRemove'),
        isEmpty,
      );
      await tester.tap(find.text('移除'));
      await tester.pumpAndSettle();
      final removed = c.api.calls.lastWhere(
        (row) => row.method == 'capabilities.serverRemove',
      );
      expect(removed.payload, {'name': 'remote', 'expectedRevision': 7});
      expect(removed.mutation, isTrue);
      await tester.tap(find.byTooltip('移除技能'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('移除'));
      await tester.pumpAndSettle();
      expect(
        c.api.calls
            .lastWhere((row) => row.method == 'capabilities.skillRemove')
            .payload,
        {'name': 'managed-skill', 'expectedRevision': 7},
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'skill save failure retains content and the immutable skill name',
    (tester) async {
      final c = await mountPage(tester);
      c.api.failMethod = 'capabilities.skillSave';
      await tester.tap(find.byTooltip('编辑技能'));
      await tester.pumpAndSettle();
      final fields = find.descendant(
        of: find.byType(TextResourceEditor),
        matching: find.byType(TextField),
      );
      expect(tester.widget<TextField>(fields.first).enabled, isFalse);
      await tester.enterText(fields.last, 'edited skill body');
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(find.byType(TextResourceEditor), findsOneWidget);
      expect(find.text('edited skill body'), findsOneWidget);
      expect(find.textContaining('配置已被其他窗口修改'), findsOneWidget);
      expect(
        c.api.calls
            .lastWhere((row) => row.method == 'capabilities.skillSave')
            .payload,
        {
          'name': 'managed-skill',
          'content': 'edited skill body',
          'overwrite': true,
          'expectedRevision': 7,
        },
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('capability search filters MCP servers as well as skills', (
    tester,
  ) async {
    await mountPage(tester);
    await tester.enterText(find.byType(TextField), 'builtin');
    await tester.pumpAndSettle();
    expect(find.text('builtin-skill'), findsOneWidget);
    expect(find.text('managed-skill'), findsNothing);
    expect(find.text('remote'), findsNothing);
    await tester.enterText(find.byType(TextField), 'example.com');
    await tester.pumpAndSettle();
    expect(find.text('remote'), findsOneWidget);
    expect(find.text('builtin-skill'), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
