import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/models_page.dart';
import 'package:dsh_desktop/features/settings/model_editor_widgets.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

Json ns() => {
  'ns': 'llm-pi-ai',
  'revision': 1,
  'value': {
    'providers': {
      'alpha': {
        'baseURL': 'https://example.org/v1',
        'apiKeyEnv': 'ALPHA_API_KEY',
      },
      'devin': {'authProvider': 'devin', 'baseURL': 'http://127.0.0.1'},
    },
  },
  'schema': {
    'uid': 1,
    'refs': {
      '1': {
        'type': 'object',
        'dict': {'providers': 2},
      },
      '2': {'type': 'dict', 'inner': 3},
      '3': {
        'type': 'object',
        'dict': {'api': 4},
      },
      '4': {
        'type': 'union',
        'list': [5, 6],
      },
      '5': {'type': 'const', 'value': 'openai-completions'},
      '6': {'type': 'const', 'value': 'openai-responses'},
    },
  },
};

class ModelsApi extends DshClient {
  ModelsApi() : super('http://127.0.0.1');
  final mutations = <({String method, Json payload})>[];
  final requestedPaths = <String>[];
  bool failKey = false;
  bool keyConfigured = true;
  Completer<Json>? settingsMutation;
  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    if (method == 'llm.providers') {
      return {
        'providers': [
          {
            'provider': 'alpha',
            'active': true,
            'displayName': 'Alpha',
            'settingsNs': 'llm-pi-ai',
            'settingsPath': ['providers', 'alpha'],
          },
          {
            'provider': 'devin',
            'active': true,
            'displayName': 'Devin / Windsurf',
            'settingsNs': 'llm-pi-ai',
            'settingsPath': ['providers', 'devin'],
          },
        ],
      };
    }
    if (method == 'settings.describe') {
      return {
        'namespaces': [ns()],
      };
    }
    if (method == 'credentials.describe') {
      return {
        'credentials': {
          'ALPHA_API_KEY': {'configured': keyConfigured, 'writable': true},
        },
      };
    }
    return {};
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    requestedPaths.add(path);
    return {
      'models': [
        {
          'id': 'hidden',
          'name': '隐藏模型',
          'enabled': false,
          'contextWindow': 128000,
        },
      ],
      'profileModels': [],
      'namespaceRevision': 1,
      'accountScope': 'scope',
      'settingsNs': 'llm-pi-ai',
      'settingsPath': ['providers', 'alpha'],
      'preferencePath': ['providers', 'alpha', 'modelPreferences', 'scope'],
    };
  }

  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    mutations.add((method: method, payload: payload));
    if (method == 'settings.mutate' && settingsMutation != null) {
      return settingsMutation!.future;
    }
    if (method == 'credentials.set' && failKey) {
      failKey = false;
      throw StateError('密钥存储失败');
    }
    return {...ns(), 'revision': 2};
  }
}

class AccountsApi extends ModelsApi {
  final accountRequests = <({String path, Json body})>[];

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    if (path == '/provider-auth/providers') {
      return {
        'providers': [
          {
            'id': 'openai-codex',
            'name': 'ChatGPT / Codex',
            'signedIn': true,
            'accounts': [
              {
                'label': 'Other account',
                'accountScope': 'other',
                'active': false,
              },
            ],
          },
          {
            'id': 'devin',
            'name': 'Devin / Windsurf',
            'scope': 'subagent',
            'signedIn': true,
          },
        ],
      };
    }
    if (path.startsWith('/provider-auth/')) {
      accountRequests.add((path: path, body: body ?? {}));
      if (path == '/provider-auth/start') {
        return {
          'attempt': 'auth-test',
          'verificationUri': 'https://example.com/authorize',
          'interval': 3,
        };
      }
      return {'status': 'complete'};
    }
    return super.request(
      path,
      body: body,
      scope: scope,
      mutation: mutation,
      maxBytes: maxBytes,
    );
  }
}

class ModelsController extends DesktopController {
  ModelsController(this.api) : active = api, super(MemoryPreferences());
  final ModelsApi api;
  DshClient active;
  @override
  DshClient get client => active;
}

void main() {
  testWidgets(
    'refresh reads upstream and deleting an API profile uses scoped unsets',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1200, 1000));
      final api = ModelsApi(), controller = ModelsController(api);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ModelsPage(
              controller: controller,
              onSettingsChanged: () async {},
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('刷新目录'));
      await tester.pumpAndSettle();
      expect(api.requestedPaths, contains('/provider-auth/refresh'));
      await tester.tap(find.text('删除连接'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('删除连接').last);
      await tester.pumpAndSettle();
      expect(
        api.mutations.any(
          (m) =>
              m.method == 'credentials.unset' &&
              m.payload['ref'] == 'ALPHA_API_KEY',
        ),
        isTrue,
      );
      expect(
        api.mutations.any(
          (m) =>
              m.method == 'settings.mutate' &&
              objects(m.payload['ops']).any(
                (o) =>
                    o['op'] == 'unset' &&
                    (o['path'] as List).join('/') == 'providers/alpha',
              ),
        ),
        isTrue,
      );
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets(
    'signed-in accounts reconnect and switched accounts refresh models',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1100, 900));
      final api = AccountsApi(), controller = ModelsController(api);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ModelsPage(
              controller: controller,
              initialTab: 'accounts',
              onSettingsChanged: () async {},
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('ChatGPT / Codex'));
      await tester.pumpAndSettle();
      expect(find.text('重新连接'), findsOneWidget);
      expect(find.text('登录另一个账号'), findsOneWidget);
      expect(find.text('同步模型'), findsNothing);
      await tester.tap(find.text('重新连接'));
      await tester.pumpAndSettle();
      expect(
        api.accountRequests
            .where((r) => r.path == '/provider-auth/connect')
            .length,
        1,
      );
      expect(
        api.accountRequests.where((r) => r.path == '/provider-auth/start'),
        isEmpty,
      );
      await tester.tap(find.text('切换'));
      await tester.pumpAndSettle();
      expect(
        api.accountRequests
            .where((r) => r.path == '/provider-auth/switch')
            .single
            .body['accountScope'],
        'other',
      );
      expect(
        api.accountRequests
            .where((r) => r.path == '/provider-auth/refresh')
            .length,
        1,
      );
      await tester.tap(find.text('登录另一个账号'));
      await tester.pump();
      await tester.pump();
      expect(find.text('连接订阅账号'), findsOneWidget);
      expect(
        api.accountRequests
            .where((r) => r.path == '/provider-auth/start')
            .length,
        1,
      );
      await tester.tap(find.text('取消'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('ChatGPT / Codex'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Devin / Windsurf'));
      await tester.pumpAndSettle();
      expect(find.text('刷新'), findsOneWidget);
      expect(find.text('退出登录'), findsNothing);
      expect(find.text('登录另一个账号'), findsNothing);
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
      await api.close();
      await tester.binding.setSurfaceSize(null);
    },
  );

  test('capacities and protocols use the same accepted shape as the Web', () {
    expect(modelCapacity('128K'), 128000);
    expect(modelCapacity('1.5M'), 1500000);
    expect(modelCapacity('0'), isNull);
    expect(modelCapacity('-4'), isNull);
    expect(modelCapacity('invalid'), isNull);
    expect(modelProtocols(ns()), ['openai-completions', 'openai-responses']);
    expect(modelProtocols(null), isEmpty);
  });
  testWidgets(
    'model edits are inline, hidden stays hidden, manual drafts do not open dialogs',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1100, 900));
      final api = ModelsApi(), c = ModelsController(ModelsApi());
      final source = c.api;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ModelsPage(controller: c, onSettingsChanged: () async {}),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsNothing);
      expect(find.text('编辑连接'), findsOneWidget);
      expect(find.text('Devin / Windsurf'), findsNothing);
      expect(find.text('收起模型'), findsOneWidget);
      expect(find.byTooltip('已配置 API 密钥'), findsOneWidget);
      expect(tester.getTopLeft(find.text('编辑连接')).dx, greaterThan(600));
      await tester.tap(find.text('仅待修复'));
      await tester.pumpAndSettle();
      expect(find.text('没有匹配的提供方'), findsOneWidget);
      await tester.tap(find.text('仅待修复'));
      await tester.pumpAndSettle();
      await tester.pumpAndSettle();
      await tester.tap(find.text('容量').first);
      await tester.pumpAndSettle();
      final name = find.byKey(const ValueKey('model-field-name'));
      await tester.enterText(name, '新的显示名称');
      await tester.pump();
      final save = find.text('保存模型');
      await tester.ensureVisible(save);
      await tester.tap(save);
      await tester.pumpAndSettle();
      final ops = objects(source.mutations.first.payload['ops']);
      expect(
        ops.any(
          (op) =>
              (op['path'] as List).last == 'name' && op['value'] == '新的显示名称',
        ),
        isTrue,
      );
      expect(ops.any((op) => (op['path'] as List).last == 'enabled'), isFalse);
      final add = find.text('添加手动模型');
      await tester.ensureVisible(add);
      await tester.tap(add);
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsNothing);
      expect(find.byKey(const ValueKey('model-field-id')), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await source.close();
      await api.close();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets(
    'custom provider keeps a failed credential draft and retries only that write',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1100, 1200));
      final api = ModelsApi()..failKey = true;
      var saved = 0;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: CustomProviderCard(
                api: api,
                namespace: ns(),
                taken: const {'alpha'},
                onSaved: () async {
                  saved++;
                },
                onCancel: () {},
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      Future<void> field(String label, String value) async {
        final target = find.descendant(
          of: find.byKey(ValueKey('provider-field-$label')),
          matching: find.byType(TextField),
        );
        await tester.ensureVisible(target);
        await tester.enterText(target, value);
      }

      await field('提供方 ID', 'beta');
      await field('显示名称', 'Beta');
      await field('Base URL', 'https://example.org/v1');
      await field('API 密钥', 'test-secret');
      await tester.ensureVisible(find.text('添加模型'));
      await tester.tap(find.text('添加模型'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const ValueKey('model-field-id')),
        'test-model',
      );
      await tester.ensureVisible(find.text('添加'));
      await tester.tap(find.text('添加'));
      await tester.pumpAndSettle();
      expect(
        api.mutations.where((m) => m.method == 'settings.mutate').length,
        1,
      );
      expect(find.textContaining('密钥存储失败'), findsOneWidget);
      expect(saved, 0);
      final config = object(
        objects(api.mutations.first.payload['ops']).single['value'],
      );
      expect(config.containsKey('apiKey'), isFalse);
      expect(config['apiKeyEnv'], 'BETA_API_KEY');
      await tester.ensureVisible(find.text('重试保存密钥'));
      await tester.tap(find.text('重试保存密钥'));
      await tester.pumpAndSettle();
      expect(
        api.mutations.where((m) => m.method == 'settings.mutate').length,
        1,
      );
      expect(
        api.mutations.where((m) => m.method == 'credentials.set').length,
        2,
      );
      expect(saved, 1);
      await tester.pumpWidget(const SizedBox());
      await api.close();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets(
    'connection switch stops a delayed credential write to the old Host',
    (tester) async {
      final first = ModelsApi()..settingsMutation = Completer<Json>();
      final second = ModelsApi();
      final c = ModelsController(first);
      var saved = 0;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ModelsPage(
              controller: c,
              onSettingsChanged: () async {
                saved++;
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('编辑连接'));
      await tester.pumpAndSettle();
      final key = find.byWidgetPredicate(
        (widget) =>
            widget is TextField &&
            widget.decoration?.hintText == '已配置——输入新值可替换',
      );
      await tester.ensureVisible(key);
      await tester.enterText(key, 'isolated-test-secret');
      await tester.ensureVisible(find.text('保存'));
      await tester.tap(find.text('保存'));
      await tester.pump();
      expect(
        first.mutations
            .where((call) => call.method == 'settings.mutate')
            .length,
        1,
      );
      c.active = second;
      c.emit();
      await tester.pump();
      first.settingsMutation!.complete({...ns(), 'revision': 2});
      await tester.pumpAndSettle();
      expect(
        first.mutations.where((call) => call.method == 'credentials.set'),
        isEmpty,
      );
      expect(second.mutations, isEmpty);
      expect(saved, 0);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await first.close();
      await second.close();
    },
  );
  testWidgets('custom provider skips its key write after connection change', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1100, 1200));
    final api = ModelsApi()..settingsMutation = Completer<Json>();
    var current = true;
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: CustomProviderCard(
              api: api,
              namespace: ns(),
              taken: const {'alpha'},
              isCurrent: () => current,
              onSaved: () async {},
              onCancel: () {},
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    Future<void> field(String label, String value) async {
      final target = find.descendant(
        of: find.byKey(ValueKey('provider-field-$label')),
        matching: find.byType(TextField),
      );
      await tester.ensureVisible(target);
      await tester.enterText(target, value);
    }

    await field('提供方 ID', 'beta');
    await field('Base URL', 'https://example.org/v1');
    await field('API 密钥', 'isolated-test-secret');
    await tester.ensureVisible(find.text('添加模型'));
    await tester.tap(find.text('添加模型'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('model-field-id')),
      'beta-model',
    );
    await tester.ensureVisible(find.text('添加'));
    await tester.tap(find.text('添加'));
    await tester.pump();
    expect(api.mutations.where((m) => m.method == 'settings.mutate').length, 1);
    current = false;
    api.settingsMutation!.complete({...ns(), 'revision': 2});
    await tester.pumpAndSettle();
    expect(api.mutations.where((m) => m.method == 'credentials.set'), isEmpty);
    await tester.pumpWidget(const SizedBox());
    await api.close();
    await tester.binding.setSurfaceSize(null);
  });
}
