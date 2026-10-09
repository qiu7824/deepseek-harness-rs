import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/account_usage_panel.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

Json response({String scope = 'a', int? reset, double? remaining = 63}) => {
  'provider': 'devin',
  'accountScope': scope,
  'status': 'fresh',
  'plan': 'Pro',
  'updatedAt': 1791172800,
  'windows': [
    {
      'id': 'daily',
      'label': '日额度',
      'remainingPercent': remaining,
      'resetsAt': reset,
      'windowDurationMins': 1440,
    },
  ],
};

class UsageApi extends DshClient {
  UsageApi() : super('http://127.0.0.1');
  final requests = <Json>[];
  Future<Json> Function(Json)? handler;

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) {
    expect(path, '/provider-auth/account-usage');
    expect(mutation, isFalse);
    requests.add(body ?? {});
    return handler?.call(body ?? {}) ?? Future.value(response());
  }
}

class UsageController extends DesktopController {
  UsageController(this.currentApi) : super(MemoryPreferences());
  DshClient currentApi;
  @override
  DshClient get client => currentApi;

  void account(String scope, {bool needsLogin = false}) {
    subscriptionAccounts = [
      {
        'id': 'devin',
        'signedIn': true,
        'accountScope': scope,
        'accounts': [
          {'active': true, 'accountScope': scope, 'needsLogin': needsLogin},
        ],
      },
    ];
    notifyListeners();
  }

  void changed() => notifyListeners();
}

Future<void> mount(
  WidgetTester tester,
  UsageController controller, {
  String scope = 'a',
  String provider = 'devin',
  bool visible = true,
  DateTime Function()? now,
}) async {
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: SizedBox(
          width: 520,
          child: AccountUsagePanel(
            controller: controller,
            api: controller.currentApi,
            provider: provider,
            accountScope: scope,
            visible: visible,
            now: now,
          ),
        ),
      ),
    ),
  );
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 100));
}

Future<void> cleanup(
  WidgetTester tester,
  UsageController controller,
  UsageApi api,
) async {
  await tester.pumpWidget(const SizedBox());
  controller.dispose();
  await api.close();
}

void main() {
  testWidgets('renders used and remaining from official percentages', (
    tester,
  ) async {
    final api = UsageApi(), controller = UsageController(UsageApi());
    controller.currentApi = api;
    controller.account('a');
    await mount(tester, controller);
    expect(find.text('套餐 · Pro'), findsOneWidget);
    expect(find.text('已用 37% · 剩余 63%'), findsOneWidget);
    final bar = tester.widget<LinearProgressIndicator>(
      find.byType(LinearProgressIndicator),
    );
    expect(bar.value, .37);
    expect(bar.semanticsValue, '37');
    expect(api.requests.single, {'provider': 'devin', 'accountScope': 'a'});
    await cleanup(tester, controller, api);
  });

  testWidgets(
    'unknown percentages and unsupported responses have no progress bar',
    (tester) async {
      final api = UsageApi()..handler = (_) async => response(remaining: null);
      final controller = UsageController(api)..account('a');
      await mount(tester, controller);
      expect(find.text('不可获取'), findsOneWidget);
      expect(find.byType(LinearProgressIndicator), findsNothing);
      api.handler = (_) async => {...response(), 'status': 'unsupported'};
      await tester.tap(find.text('刷新额度'));
      await tester.pump();
      expect(find.text('此账号暂未提供额度查询'), findsOneWidget);
      expect(find.byType(LinearProgressIndicator), findsNothing);
      await cleanup(tester, controller, api);
    },
  );

  testWidgets('only visible panels fetch, reopening honors 60 second cache', (
    tester,
  ) async {
    var now = DateTime.utc(2026, 10, 5);
    final api = UsageApi(), controller = UsageController(UsageApi());
    controller.currentApi = api;
    controller.account('a');
    await mount(tester, controller, visible: false, now: () => now);
    expect(api.requests, isEmpty);
    await mount(tester, controller, now: () => now);
    expect(api.requests, hasLength(1));
    await mount(tester, controller, visible: false, now: () => now);
    now = now.add(const Duration(seconds: 59));
    await mount(tester, controller, now: () => now);
    expect(api.requests, hasLength(1));
    await mount(tester, controller, visible: false, now: () => now);
    now = now.add(const Duration(seconds: 2));
    await mount(tester, controller, now: () => now);
    expect(api.requests, hasLength(2));
    await tester.tap(find.text('刷新额度'));
    await tester.pump();
    expect(api.requests.last['refresh'], isTrue);
    await cleanup(tester, controller, api);
  });

  testWidgets('failed refresh retains stale snapshot while 401 clears it', (
    tester,
  ) async {
    final api = UsageApi(), controller = UsageController(UsageApi());
    controller.currentApi = api;
    controller.account('a');
    await mount(tester, controller);
    api.handler = (_) =>
        Future.error(StateError('upstream token must not appear'));
    await tester.tap(find.text('刷新额度'));
    await tester.pump();
    expect(find.text('已用 37% · 剩余 63%'), findsOneWidget);
    expect(find.text('暂时无法刷新，显示上次数据'), findsOneWidget);
    expect(find.textContaining('upstream token'), findsNothing);
    api.handler = (_) => Future.error(DshException('http-401', 'secret'));
    await tester.tap(find.text('刷新额度'));
    await tester.pump();
    expect(find.byType(LinearProgressIndicator), findsNothing);
    expect(find.text('重新登录后可查询额度'), findsOneWidget);
    await cleanup(tester, controller, api);
  });

  testWidgets(
    'controller account change clears old data before widget props change',
    (tester) async {
      final api = UsageApi(), controller = UsageController(UsageApi());
      controller.currentApi = api;
      controller.account('a');
      await mount(tester, controller);
      controller.account('b');
      await tester.pump();
      expect(find.byType(LinearProgressIndicator), findsNothing);
      expect(api.requests, hasLength(1));
      api.handler = (_) async => response(scope: 'b', remaining: 81);
      await mount(tester, controller, scope: 'b');
      expect(find.text('已用 19% · 剩余 81%'), findsOneWidget);
      await cleanup(tester, controller, api);
    },
  );

  testWidgets('late result from previous account cannot restore its progress', (
    tester,
  ) async {
    final pending = Completer<Json>();
    final api = UsageApi()..handler = (_) => pending.future;
    final controller = UsageController(api)..account('a');
    await mount(tester, controller);
    controller.account('b');
    await tester.pump();
    pending.complete(response());
    await tester.pump();
    expect(find.byType(LinearProgressIndicator), findsNothing);
    expect(find.textContaining('37%'), findsNothing);
    await cleanup(tester, controller, api);
  });

  testWidgets('response identity mismatch discards the last good snapshot', (
    tester,
  ) async {
    final api = UsageApi(), controller = UsageController(UsageApi());
    controller.currentApi = api;
    controller.account('a');
    await mount(tester, controller);
    api.handler = (_) async => response(scope: 'b');
    await tester.tap(find.text('刷新额度'));
    await tester.pump();
    expect(find.byType(LinearProgressIndicator), findsNothing);
    expect(find.text('账号或连接已变化，请重新打开账号'), findsOneWidget);
    await cleanup(tester, controller, api);
  });

  testWidgets('visible reset refreshes once and an expired reset never polls', (
    tester,
  ) async {
    var now = DateTime.utc(2026, 10, 5);
    final reset = now.millisecondsSinceEpoch ~/ 1000 + 2;
    final api = UsageApi()..handler = (_) async => response(reset: reset);
    final controller = UsageController(api)..account('a');
    await mount(tester, controller, now: () => now);
    expect(api.requests, hasLength(1));
    now = now.add(const Duration(seconds: 2));
    await tester.pump(const Duration(seconds: 2));
    await tester.pump();
    expect(api.requests, hasLength(2));
    expect(api.requests.last['refresh'], isTrue);
    now = now.add(const Duration(minutes: 5));
    await tester.pump(const Duration(minutes: 5));
    expect(api.requests, hasLength(2));
    await cleanup(tester, controller, api);
  });

  testWidgets(
    'host identity changes clear snapshots, missing scope never requests',
    (tester) async {
      final api = UsageApi(), controller = UsageController(UsageApi());
      controller.currentApi = api;
      controller.account('a');
      await mount(tester, controller);
      final pending = Completer<Json>();
      api.handler = (_) => pending.future;
      controller.host = HostInfo.fromJson({
        'home': 'other',
        'version': 'test',
        'cwd': 'other',
      });
      controller.changed();
      await tester.pump();
      expect(find.byType(LinearProgressIndicator), findsNothing);
      pending.complete(response(remaining: 50));
      await tester.pump();
      expect(find.text('已用 50% · 剩余 50%'), findsNothing);
      expect(api.requests, hasLength(1));
      final count = api.requests.length;
      await mount(tester, controller, scope: '');
      expect(find.byType(LinearProgressIndicator), findsNothing);
      expect(api.requests, hasLength(count));
      await cleanup(tester, controller, api);
    },
  );

  testWidgets(
    'credit balances show explicit units, period end is not a reset timer',
    (tester) async {
      var now = DateTime.utc(2026, 10, 5);
      final end = now.millisecondsSinceEpoch ~/ 1000 + 2;
      final api = UsageApi()
        ..handler = (_) async => {
          ...response(),
          'windows': [
            {
              'id': 'subscription',
              'label': '订阅 credits',
              'remaining': 23.5,
              'limit': 20,
              'unit': 'credits',
              'periodEndsAt': end,
            },
            {'id': 'other', 'label': '余额', 'remaining': 7.5, 'unit': 'unknown'},
          ],
        };
      final controller = UsageController(api)..account('a');
      await mount(tester, controller, now: () => now);
      expect(find.text('剩余 23.5 credits · 额度 20 credits'), findsOneWidget);
      expect(find.text('剩余 7.5 单位未提供'), findsOneWidget);
      expect(find.textContaining('周期结束 · '), findsOneWidget);
      expect(find.text('重置 · 不可获取'), findsNWidgets(2));
      expect(find.byType(LinearProgressIndicator), findsNothing);
      now = now.add(const Duration(seconds: 3));
      await tester.pump(const Duration(seconds: 3));
      expect(api.requests, hasLength(1));
      await cleanup(tester, controller, api);
    },
  );

  testWidgets(
    'safe host denial messages and clearSnapshot discard previous data',
    (tester) async {
      final api = UsageApi();
      final controller = UsageController(api)..account('a');
      await mount(tester, controller);
      api.handler = (_) async => {
        ...response(),
        'status': 'unavailable',
        'clearSnapshot': true,
        'message': '账号没有额度查询权限',
        'windows': [],
      };
      await tester.tap(find.text('刷新额度'));
      await tester.pump();
      expect(find.text('账号没有额度查询权限'), findsOneWidget);
      expect(find.byType(LinearProgressIndicator), findsNothing);
      expect(find.textContaining('37%'), findsNothing);
      api.handler = (_) async => response();
      await tester.tap(find.text('刷新额度'));
      await tester.pump();
      api.handler = (_) => Future.error(DshException('http-403', 'secret'));
      await tester.tap(find.text('刷新额度'));
      await tester.pump();
      expect(find.byType(LinearProgressIndicator), findsNothing);
      await cleanup(tester, controller, api);
    },
  );

  testWidgets('collapsed reset waits until reopening and refreshes only once', (
    tester,
  ) async {
    var now = DateTime.utc(2026, 10, 5);
    final reset = now.millisecondsSinceEpoch ~/ 1000 + 5;
    final api = UsageApi()..handler = (_) async => response(reset: reset);
    final controller = UsageController(api)..account('a');
    await mount(tester, controller, now: () => now);
    await mount(tester, controller, visible: false, now: () => now);
    now = now.add(const Duration(seconds: 6));
    await tester.pump(const Duration(seconds: 6));
    expect(api.requests, hasLength(1));
    await mount(tester, controller, now: () => now);
    await tester.pump();
    expect(api.requests, hasLength(2));
    expect(api.requests.last['refresh'], isTrue);
    await tester.pump(const Duration(minutes: 2));
    expect(api.requests, hasLength(2));
    await cleanup(tester, controller, api);
  });

  testWidgets(
    'Claude without account identity offers official usage guidance without querying',
    (tester) async {
      final api = UsageApi();
      final controller = UsageController(api);
      await mount(tester, controller, provider: 'claude-code', scope: '');
      expect(find.text('在 Claude Code 中使用 /usage 查看账号额度'), findsOneWidget);
      expect(find.text('账号或连接已变化，请重新打开账号'), findsNothing);
      expect(api.requests, isEmpty);
      await cleanup(tester, controller, api);
    },
  );

  testWidgets(
    'unsupported providers with genuine scope show the safe Host reason',
    (tester) async {
      final api = UsageApi()
        ..handler = (body) async => {
          'provider': body['provider'],
          'accountScope': body['accountScope'],
          'status': 'unsupported',
          'plan': null,
          'updatedAt': null,
          'windows': [],
          'message': 'Qwen 未提供可用的官方账号额度接口',
        };
      final controller = UsageController(api)
        ..subscriptionAccounts = [
          {
            'id': 'qwen-oauth',
            'signedIn': true,
            'accountScope': 'qwen-real-scope',
          },
        ];
      await mount(
        tester,
        controller,
        provider: 'qwen-oauth',
        scope: 'qwen-real-scope',
      );
      expect(find.text('Qwen 未提供可用的官方账号额度接口'), findsOneWidget);
      expect(find.byType(LinearProgressIndicator), findsNothing);
      expect(api.requests.single['accountScope'], 'qwen-real-scope');
      await cleanup(tester, controller, api);
    },
  );
}
