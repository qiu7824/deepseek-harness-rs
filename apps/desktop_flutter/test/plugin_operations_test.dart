import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/settings/plugin_operations_panel.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

const _source =
    'github:example/plugin#0123456789abcdef0123456789abcdef01234567';

Json _operation({
  String id = 'operation-1',
  String phase = 'running',
  String action = 'add',
  String spec = _source,
  String log = '解析插件来源',
  String? error,
  bool restartRequired = false,
}) => {
  'version': 1,
  'operationId': id,
  'action': action,
  'spec': spec,
  'phase': phase,
  'log': log,
  'error': error,
  'restartRequired': restartRequired,
  'effects': phase == 'succeeded' ? 'committed' : 'pending',
  'startedAt': 1790460000000,
  'finishedAt': phase == 'succeeded' ? 1790460001000 : null,
};

typedef _Request = ({
  String path,
  Json body,
  RequestScope? scope,
  bool mutation,
});

class _PluginApi extends DshClient {
  _PluginApi() : super('http://127.0.0.1');
  final requests = <_Request>[];
  Json status = {'operation': null, 'configurationError': null};
  FutureOr<Json> Function(Json)? onStatus, onMutation;

  Iterable<_Request> get mutations => requests.where((r) => r.mutation);
  Iterable<_Request> get reads => requests.where((r) => !r.mutation);

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    requests.add((
      path: path,
      body: {...?body},
      scope: scope,
      mutation: mutation,
    ));
    expect(path, '/__dsh-plugin-manager');
    expect(body, isNotNull, reason: 'The Host endpoint accepts POST only.');
    expect(scope, isNotNull);
    expect(scope!.cancelled, isFalse);
    if (body!['action'] == 'status') {
      expect(mutation, isFalse);
      return onStatus == null ? status : await onStatus!(body);
    }
    expect(mutation, isTrue);
    return onMutation == null
        ? {'operation': _operation()}
        : await onMutation!(body);
  }
}

class _PluginController extends DesktopController {
  _PluginController(this.active) : super(MemoryPreferences());
  DshClient active;
  @override
  DshClient get client => active;
  void switchHost(DshClient api) {
    active = api;
    notifyListeners();
  }
}

Future<void> _withPanel(
  WidgetTester tester,
  _PluginApi api,
  Future<void> Function(_PluginController controller) body, {
  VoidCallback? onCompleted,
}) async {
  final controller = _PluginController(api);
  await tester.binding.setSurfaceSize(const Size(1000, 1200));
  try {
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: Padding(
              padding: const EdgeInsets.all(20),
              child: PluginOperationsPanel(
                controller: controller,
                onOperationCompleted: onCompleted,
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    await body(controller);
  } finally {
    await tester.pumpWidget(const SizedBox.shrink());
    controller.dispose();
    await api.close();
    await tester.binding.setSurfaceSize(null);
  }
}

Finder _key(String key) => find.byKey(ValueKey(key));
Future<void> _tap(WidgetTester tester, String key) async {
  await tester.ensureVisible(_key(key));
  await tester.tap(_key(key));
  await tester.pump();
}

Future<void> _spec(WidgetTester tester, String value) async {
  await tester.enterText(
    find.descendant(of: _key('plugin-spec'), matching: find.byType(TextField)),
    value,
  );
  await tester.pump();
}

String _draft(WidgetTester tester) =>
    tester.widget<DshField>(_key('plugin-spec')).controller!.text;
bool _enabled(WidgetTester tester, String key) =>
    tester.widget<DshButton>(_key(key)).onPressed != null;

void main() {
  testWidgets('same client Host switch clears confirmation and stops reads', (
    tester,
  ) async {
    final api = _PluginApi();
    await _withPanel(tester, api, (controller) async {
      await _spec(tester, _source);
      await _tap(tester, 'plugin-check');
      final confirm = tester
          .widget<DshButton>(_key('plugin-confirm'))
          .onPressed!;
      controller.host = HostInfo.fromJson({
        'version': 'other-host',
        'home': 'other-home',
        'cwd': 'other-workspace',
      });
      controller.notifyListeners();
      confirm();
      await tester.pump(const Duration(seconds: 20));
      expect(_key('plugin-confirm'), findsNothing);
      expect(_enabled(tester, 'plugin-check'), isFalse);
      expect(api.reads, hasLength(1));
      expect(api.mutations, isEmpty);
      expect(_draft(tester), _source);
      expect(find.textContaining('连接已变化'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  });

  testWidgets(
    'install requires trusted full SHA confirmation and preserves separate drafts',
    (tester) async {
      final api = _PluginApi();
      var completed = 0;
      await _withPanel(tester, api, (_) async {
        expect(api.reads.single.body, {'action': 'status'});
        await _spec(tester, 'github:example/plugin#main');
        await _tap(tester, 'plugin-check');
        expect(find.textContaining('40 位提交 SHA'), findsOneWidget);
        expect(api.mutations, isEmpty);
        await _spec(tester, '  $_source  ');
        await _tap(tester, 'plugin-check');
        expect(api.mutations, isEmpty);
        expect(find.textContaining('确认信任此来源'), findsOneWidget);
        await _tap(tester, 'plugin-dismiss-confirm');
        expect(_draft(tester), '  $_source  ');
        await _tap(tester, 'plugin-action-remove');
        await _spec(tester, '@example/plugin');
        await _tap(tester, 'plugin-action-add');
        expect(_draft(tester), '  $_source  ');
        await _tap(tester, 'plugin-check');
        await _tap(tester, 'plugin-confirm');
        expect(api.mutations.single.body, {'action': 'add', 'spec': _source});
        expect(find.text('正在处理'), findsOneWidget);
        expect(find.text('解析插件来源'), findsOneWidget);
        expect(_enabled(tester, 'plugin-check'), isFalse);
        api.status = {
          'operation': _operation(
            phase: 'succeeded',
            restartRequired: true,
            log: '提交成功',
          ),
        };
        await tester.pump(const Duration(milliseconds: 750));
        await tester.pump();
        expect(find.text('已完成'), findsOneWidget);
        expect(find.text('提交成功'), findsOneWidget);
        expect(find.textContaining('请重启应用'), findsOneWidget);
        expect(completed, 1);
        await _tap(tester, 'plugin-refresh');
        expect(completed, 1);
        await _tap(tester, 'plugin-action-remove');
        expect(_draft(tester), '@example/plugin');
        expect(api.mutations, hasLength(1));
      }, onCompleted: () => completed++);
    },
  );

  testWidgets('uninstall confirms the exact package and retains failed draft', (
    tester,
  ) async {
    final api = _PluginApi();
    api.onMutation = (_) => throw DshException('http-400', '包仍被其他插件依赖');
    await _withPanel(tester, api, (_) async {
      await _tap(tester, 'plugin-action-remove');
      await _spec(tester, '@example/plugin');
      await _tap(tester, 'plugin-check');
      expect(find.textContaining('确认卸载此插件'), findsOneWidget);
      expect(api.mutations, isEmpty);
      await _tap(tester, 'plugin-confirm');
      await tester.pump();
      expect(api.mutations.single.body, {
        'action': 'remove',
        'spec': '@example/plugin',
      });
      expect(find.textContaining('包仍被其他插件依赖'), findsOneWidget);
      expect(_draft(tester), '@example/plugin');
      expect(_enabled(tester, 'plugin-check'), isTrue);
      expect(api.reads, hasLength(2));
    });
  });

  testWidgets(
    'recover is explicit and uses the Host recover contract without spec',
    (tester) async {
      final api = _PluginApi()
        ..status = {'operation': null, 'configurationError': '插件配置无效'};
      api.onMutation = (_) => {
        'operation': _operation(
          action: 'recover',
          spec: '',
          phase: 'succeeded',
          restartRequired: true,
        ),
      };
      await _withPanel(tester, api, (_) async {
        expect(find.text('插件配置无效'), findsOneWidget);
        await _tap(tester, 'plugin-recover');
        expect(api.mutations, isEmpty);
        expect(find.textContaining('确认用上次有效配置'), findsOneWidget);
        await _tap(tester, 'plugin-confirm');
        expect(api.mutations.single.body, {'action': 'recover'});
        api.status = {
          'operation': _operation(
            action: 'recover',
            spec: '',
            phase: 'succeeded',
          ),
          'configurationError': null,
        };
        await _tap(tester, 'plugin-refresh');
        expect(find.text('插件配置无效'), findsNothing);
        expect(_key('plugin-recover'), findsNothing);
      });
    },
  );

  testWidgets(
    'cancel pins operationId and a late status cannot overwrite its result',
    (tester) async {
      final api = _PluginApi()
        ..status = {'operation': _operation(id: 'operation-a')};
      final lateRead = Completer<Json>();
      await _withPanel(tester, api, (_) async {
        api.onStatus = (_) => lateRead.future;
        await _tap(tester, 'plugin-refresh');
        final readScope = api.reads.last.scope!;
        await _tap(tester, 'plugin-cancel');
        expect(api.mutations, isEmpty);
        api.onMutation = (_) => {
          'operation': _operation(id: 'operation-a', phase: 'cancelling'),
        };
        await _tap(tester, 'plugin-confirm');
        expect(api.mutations.single.body, {
          'action': 'cancel',
          'operationId': 'operation-a',
        });
        expect(readScope.cancelled, isTrue);
        expect(find.text('正在取消并清理'), findsOneWidget);
        expect(_enabled(tester, 'plugin-cancel'), isFalse);
        lateRead.complete({
          'operation': _operation(id: 'wrong-old-response', phase: 'running'),
        });
        await tester.pump();
        expect(find.text('正在取消并清理'), findsOneWidget);
        expect(find.textContaining('wrong-old-response'), findsNothing);
        api.onStatus = null;
        api.status = {
          'operation': _operation(
            id: 'operation-a',
            phase: 'cancelled',
            log: '临时文件已清理',
          ),
        };
        await tester.pump(const Duration(milliseconds: 750));
        await tester.pump();
        expect(find.text('已取消'), findsOneWidget);
        expect(find.text('临时文件已清理'), findsOneWidget);
        expect(_key('plugin-cancel'), findsNothing);
      });
    },
  );

  testWidgets('an external replacement invalidates cancellation confirmation', (
    tester,
  ) async {
    final api = _PluginApi()
      ..status = {'operation': _operation(id: 'operation-a')};
    await _withPanel(tester, api, (_) async {
      await _tap(tester, 'plugin-cancel');
      expect(_key('plugin-confirm'), findsOneWidget);
      api.status = {'operation': _operation(id: 'operation-b')};
      await tester.pump(const Duration(milliseconds: 750));
      await tester.pump();
      expect(_key('plugin-confirm'), findsNothing);
      expect(find.text('操作标识：operation-b'), findsOneWidget);
      expect(api.mutations, isEmpty);
    });
  });

  testWidgets('polling is single flight and close only aborts frontend reads', (
    tester,
  ) async {
    final api = _PluginApi();
    final pending = Completer<Json>();
    api.onStatus = (_) => pending.future;
    await _withPanel(tester, api, (_) async {
      expect(api.reads, hasLength(1));
      await tester.pump(const Duration(seconds: 20));
      await _tap(tester, 'plugin-refresh');
      expect(api.reads, hasLength(1));
      final scope = api.reads.single.scope!;
      await tester.pumpWidget(const SizedBox.shrink());
      expect(scope.cancelled, isTrue);
      pending.complete({'operation': _operation()});
      await tester.pump(const Duration(seconds: 20));
      expect(api.requests, hasLength(1));
      expect(api.mutations, isEmpty);
      expect(tester.takeException(), isNull);
    });
  });

  testWidgets(
    'Host change aborts pending mutation and never sends it to replacement',
    (tester) async {
      final api = _PluginApi(), replacement = _PluginApi();
      final pending = Completer<Json>();
      api.onMutation = (_) => pending.future;
      await _withPanel(tester, api, (controller) async {
        await _spec(tester, _source);
        await _tap(tester, 'plugin-check');
        await _tap(tester, 'plugin-confirm');
        expect(api.mutations, hasLength(1));
        final scope = api.mutations.single.scope!;
        controller.switchHost(replacement);
        await tester.pump();
        expect(scope.cancelled, isTrue);
        expect(find.textContaining('连接已变化'), findsOneWidget);
        expect(_enabled(tester, 'plugin-check'), isFalse);
        pending.complete({
          'operation': _operation(phase: 'succeeded', restartRequired: true),
        });
        await tester.pump(const Duration(seconds: 20));
        expect(find.text('已完成'), findsNothing);
        expect(replacement.requests, isEmpty);
        expect(api.mutations, hasLength(1));
        expect(api.requests.any((r) => r.body['action'] == 'cancel'), isFalse);
        expect(_draft(tester), _source);
        await replacement.close();
      });
    },
  );

  testWidgets(
    'close during mutation does not cancel or report late backend completion',
    (tester) async {
      final api = _PluginApi();
      final pending = Completer<Json>();
      api.onMutation = (_) => pending.future;
      var completed = 0;
      await _withPanel(tester, api, (_) async {
        await _spec(tester, _source);
        await _tap(tester, 'plugin-check');
        await _tap(tester, 'plugin-confirm');
        final scope = api.mutations.single.scope!;
        await tester.pumpWidget(const SizedBox.shrink());
        expect(scope.cancelled, isTrue);
        pending.complete({'operation': _operation(phase: 'succeeded')});
        await tester.pump(const Duration(seconds: 20));
        expect(api.mutations.single.body['action'], 'add');
        expect(completed, 0);
        expect(tester.takeException(), isNull);
      }, onCompleted: () => completed++);
    },
  );

  testWidgets(
    'transport failure reconciles status without resubmitting mutation',
    (tester) async {
      final api = _PluginApi();
      api.onMutation = (_) {
        api.status = {
          'operation': _operation(id: 'accepted-before-disconnect'),
        };
        throw DshException('transport', '连接中断', outcomeUnknown: true);
      };
      await _withPanel(tester, api, (_) async {
        await _spec(tester, _source);
        await _tap(tester, 'plugin-check');
        await _tap(tester, 'plugin-confirm');
        await tester.pump();
        expect(api.mutations, hasLength(1));
        expect(find.text('操作标识：accepted-before-disconnect'), findsOneWidget);
        expect(find.textContaining('连接中断'), findsOneWidget);
        expect(_enabled(tester, 'plugin-check'), isFalse);
        await tester.pump(const Duration(milliseconds: 750));
        expect(api.mutations, hasLength(1));
        expect(_draft(tester), _source);
      });
    },
  );

  testWidgets(
    'malformed status and read failure cannot enable destructive actions',
    (tester) async {
      final api = _PluginApi()
        ..status = {
          'operation': {'phase': 'succeeded'},
        };
      await _withPanel(tester, api, (_) async {
        expect(_enabled(tester, 'plugin-check'), isFalse);
        expect(find.textContaining('状态不完整'), findsOneWidget);
        api.onStatus = (_) => throw DshException('http-503', '服务暂不可用');
        await _tap(tester, 'plugin-refresh');
        expect(find.textContaining('服务暂不可用'), findsOneWidget);
        expect(_enabled(tester, 'plugin-check'), isFalse);
        api.onStatus = null;
        api.status = {
          'operation': _operation(phase: 'failed', error: '插件摘要不一致'),
        };
        await _tap(tester, 'plugin-refresh');
        expect(find.text('操作失败'), findsOneWidget);
        expect(find.text('插件摘要不一致'), findsOneWidget);
        expect(_enabled(tester, 'plugin-check'), isTrue);
        expect(api.mutations, isEmpty);
      });
    },
  );

  testWidgets(
    'reopening attaches to background operation without another install',
    (tester) async {
      final api = _PluginApi()
        ..status = {'operation': _operation(id: 'background-operation')};
      await _withPanel(tester, api, (_) async {
        expect(find.text('操作标识：background-operation'), findsOneWidget);
        expect(api.mutations, isEmpty);
        expect(_enabled(tester, 'plugin-check'), isFalse);
        expect(_enabled(tester, 'plugin-cancel'), isTrue);
      });
      expect(api.requests.every((r) => r.body['action'] == 'status'), isTrue);
    },
  );
}
