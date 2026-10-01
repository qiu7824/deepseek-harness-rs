import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/design/select.dart';
import 'package:dsh_desktop/features/settings/time_context_panel.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

class TimeContextClient extends DshClient {
  TimeContextClient() : super('http://127.0.0.1:58080');

  Json config = {};
  String revision = 'r1';
  final calls = <Json>[];
  final scopes = <RequestScope>[];
  final pending = <String, Completer<Json>>{};
  DshException? saveFailure;
  Json? readOverride;

  Json snapshot({String entry = 'clock'}) => {
    'entryId': entry,
    'moduleName': '@deepseek-ai/dsh-time-context',
    'revision': revision,
    'config': config,
  };

  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    calls.add({'method': method, 'payload': payload, 'mutation': mutation});
    if (scope != null) scopes.add(scope);
    if (pending[method] case final request?) return request.future;
    switch (method) {
      case 'pluginInventory.getConfig':
        return readOverride ?? snapshot(entry: payload['entryId'] as String);
      case 'pluginInventory.setConfig':
        if (saveFailure != null) throw saveFailure!;
        if (payload['expectedRevision'] != revision) {
          throw DshException('plugin-config-conflict', '配置已改变');
        }
        config = object(payload['config']);
        revision = '$revision-next';
        return snapshot(entry: payload['entryId'] as String);
      default:
        throw StateError('Unexpected call: $method');
    }
  }

  List<Json> get writes => calls
      .where((call) => call['method'] == 'pluginInventory.setConfig')
      .toList();
}

class TimeContextController extends DesktopController {
  TimeContextController() : super(MemoryPreferences()) {
    host = description();
  }

  final original = TimeContextClient();
  TimeContextClient? replacement;

  @override
  DshClient get client => replacement ?? original;

  HostInfo description() => HostInfo.fromJson({
    'version': 'test',
    'home': 'test-home',
    'cwd': 'test-workspace',
  });

  void changeApi() {
    replacement = TimeContextClient();
    emit();
  }

  void changeHost() {
    host = description();
    emit();
  }

  Future<void> close() async {
    dispose();
    await original.close();
    await replacement?.close();
  }
}

Finder keyed(String key) => find.byKey(ValueKey('time-context-$key'));

Finder input(String key) =>
    find.descendant(of: keyed(key), matching: find.byType(TextField));

String text(WidgetTester tester, String key) =>
    tester.widget<TextField>(input(key)).controller!.text;

Future<void> fill(WidgetTester tester, String key, String value) async {
  await tester.enterText(input(key), value);
  await tester.pump();
}

Future<void> tap(WidgetTester tester, String key, {bool settle = true}) async {
  await tester.ensureVisible(keyed(key));
  await tester.tap(keyed(key));
  if (settle) {
    await tester.pumpAndSettle();
  } else {
    await tester.pump();
  }
}

bool enabled(WidgetTester tester, String key) =>
    tester.widget<DshButton>(keyed(key)).onPressed != null;

Future<void> mount(
  WidgetTester tester,
  TimeContextController controller, {
  bool settle = true,
  String entryId = 'clock',
  bool milliseconds = true,
}) async {
  await tester.binding.setSurfaceSize(const Size(900, 900));
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: SingleChildScrollView(
          padding: const EdgeInsets.all(20),
          child: TimeContextPanel(controller: controller, entryId: entryId),
        ),
      ),
    ),
  );
  if (settle) {
    await tester.pumpAndSettle();
  } else {
    await tester.pump();
  }
  if (milliseconds) {
    tester.widget<DshSelect<int>>(keyed('unit')).onChanged?.call(1);
    await tester.pump();
  }
}

void main() {
  late TimeContextController controller;

  setUp(() => controller = TimeContextController());
  tearDown(() => controller.close());

  testWidgets('omitted settings display defaults without writing or enabling', (
    tester,
  ) async {
    await mount(tester, controller, milliseconds: false);
    expect(text(tester, 'interval'), '');
    expect(tester.widget<DshField>(keyed('interval')).hint, '10');
    expect(tester.widget<DshSelect<int>>(keyed('unit')).value, 60000);
    expect(find.textContaining('默认每十分钟'), findsOneWidget);
    expect(text(tester, 'zone'), '');
    expect(enabled(tester, 'save'), isFalse);
    expect(
      controller.original.calls.single['method'],
      'pluginInventory.getConfig',
    );
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets('explicit zero remains zero while saving a time zone', (
    tester,
  ) async {
    controller.original.config = {'refreshIntervalMs': 0};
    await mount(tester, controller);
    expect(text(tester, 'interval'), '0');
    await fill(tester, 'zone', ' Asia/Shanghai ');
    await tap(tester, 'save');
    final write = controller.original.writes.single;
    expect(write['mutation'], isTrue);
    expect(write['payload'], {
      'entryId': 'clock',
      'expectedRevision': 'r1',
      'config': {'refreshIntervalMs': 0, 'timeZone': 'Asia/Shanghai'},
    });
    expect(text(tester, 'interval'), '0');
    expect(enabled(tester, 'save'), isFalse);
    expect(find.textContaining('启停状态保持不变'), findsOneWidget);
    expect(controller.original.calls.length, 2);
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets(
    'custom interval saves and clearing both fields restores defaults',
    (tester) async {
      await mount(tester, controller);
      await fill(tester, 'interval', ' 45000 ');
      await fill(tester, 'zone', 'UTC');
      await tap(tester, 'save');
      expect(controller.original.config, {
        'refreshIntervalMs': 45000,
        'timeZone': 'UTC',
      });
      await fill(tester, 'interval', '');
      await fill(tester, 'zone', '');
      await tap(tester, 'save');
      expect(controller.original.config, isEmpty);
      expect(
        controller.original.writes.last['payload']['expectedRevision'],
        'r1-next',
      );
      await tester.pumpWidget(const SizedBox());
      await tester.binding.setSurfaceSize(null);
    },
  );

  testWidgets('invalid intervals never reach the Host', (tester) async {
    await mount(tester, controller);
    for (final invalid in ['-1', '0.5', '9007199254740992', 'NaN', '1e3']) {
      await fill(tester, 'interval', invalid);
      expect(enabled(tester, 'save'), isFalse, reason: invalid);
      expect(find.textContaining('刷新间隔须为'), findsOneWidget);
      expect(controller.original.writes, isEmpty);
    }
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets('safe integer limit preserves unedited configuration fields', (
    tester,
  ) async {
    controller.original.config = {
      'futureOption': {'enabled': false},
      'timeZone': 'UTC',
    };
    await mount(tester, controller);
    await fill(tester, 'interval', '9007199254740991');
    await tap(tester, 'save');
    expect(controller.original.config, {
      'futureOption': {'enabled': false},
      'timeZone': 'UTC',
      'refreshIntervalMs': 9007199254740991,
    });
    expect(enabled(tester, 'save'), isFalse);
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets('cancel modification restores snapshot without saving', (
    tester,
  ) async {
    controller.original.config = {'refreshIntervalMs': 0, 'timeZone': 'UTC'};
    await mount(tester, controller);
    await fill(tester, 'interval', '30000');
    await fill(tester, 'zone', 'Asia/Shanghai');
    await tap(tester, 'discard');
    expect(text(tester, 'interval'), '0');
    expect(text(tester, 'zone'), 'UTC');
    expect(controller.original.writes, isEmpty);
    expect(enabled(tester, 'save'), isFalse);
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets(
    'friendly units convert exactly and never round stored milliseconds',
    (tester) async {
      controller.original.config = {'refreshIntervalMs': 600001};
      await mount(tester, controller, milliseconds: false);
      expect(text(tester, 'interval'), '600001');
      expect(tester.widget<DshSelect<int>>(keyed('unit')).value, 1);
      tester.widget<DshSelect<int>>(keyed('unit')).onChanged!(60000);
      await tester.pump();
      expect(text(tester, 'interval'), '600001');
      expect(tester.widget<DshSelect<int>>(keyed('unit')).value, 1);
      expect(find.textContaining('已保留原单位'), findsOneWidget);
      await fill(tester, 'zone', 'UTC');
      await tap(tester, 'save');
      expect(controller.original.config['refreshIntervalMs'], 600001);

      await fill(tester, 'interval', '');
      tester.widget<DshSelect<int>>(keyed('unit')).onChanged!(60000);
      await tester.pump();
      await fill(tester, 'interval', '1.25');
      tester.widget<DshSelect<int>>(keyed('unit')).onChanged!(1000);
      await tester.pump();
      expect(text(tester, 'interval'), '75');
      await tap(tester, 'save');
      expect(controller.original.config['refreshIntervalMs'], 75000);
      expect(tester.widget<DshSelect<int>>(keyed('unit')).value, 1000);

      tester.widget<DshSelect<int>>(keyed('unit')).onChanged!(1);
      await tester.pump();
      await fill(tester, 'interval', '1234');
      tester.widget<DshSelect<int>>(keyed('unit')).onChanged!(1000);
      await tester.pump();
      expect(text(tester, 'interval'), '1.234');
      await tap(tester, 'save');
      expect(controller.original.config['refreshIntervalMs'], 1234);
      await tester.pumpWidget(const SizedBox());
      await tester.binding.setSurfaceSize(null);
    },
  );

  testWidgets('minute input validates exact millisecond precision and limit', (
    tester,
  ) async {
    await mount(tester, controller, milliseconds: false);
    for (final invalid in ['0.000001', '150119987579.016533333', '1e3']) {
      await fill(tester, 'interval', invalid);
      expect(enabled(tester, 'save'), isFalse, reason: invalid);
    }
    await fill(tester, 'interval', '0.00005');
    await tap(tester, 'save');
    expect(controller.original.config['refreshIntervalMs'], 3);
    expect(tester.widget<DshSelect<int>>(keyed('unit')).value, 1);
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets('conflict preserves draft until explicit refresh and compare', (
    tester,
  ) async {
    await mount(tester, controller);
    await fill(tester, 'interval', '0');
    await fill(tester, 'zone', 'Asia/Shanghai');
    controller.original
      ..revision = 'r2'
      ..config = {'refreshIntervalMs': 90000, 'timeZone': 'UTC'};
    await tap(tester, 'save');
    expect(text(tester, 'interval'), '0');
    expect(text(tester, 'zone'), 'Asia/Shanghai');
    expect(find.textContaining('配置已在其他位置更改'), findsOneWidget);
    expect(enabled(tester, 'save'), isFalse);
    await tap(tester, 'reload');
    expect(text(tester, 'interval'), '0');
    expect(text(tester, 'zone'), 'Asia/Shanghai');
    expect(find.textContaining('每 90 秒更新，UTC'), findsOneWidget);
    expect(controller.original.writes.length, 1);
    await tap(tester, 'save');
    expect(
      controller.original.writes.last['payload']['expectedRevision'],
      'r2',
    );
    expect(controller.original.config, {
      'refreshIntervalMs': 0,
      'timeZone': 'Asia/Shanghai',
    });
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets('failed save retains draft and retry uses refreshed revision', (
    tester,
  ) async {
    controller.original.saveFailure = DshException('io', '保存失败');
    await mount(tester, controller);
    await fill(tester, 'interval', '720000');
    await tap(tester, 'save');
    expect(text(tester, 'interval'), '720000');
    expect(find.textContaining('保存失败；草稿已保留'), findsOneWidget);
    expect(controller.original.config, isEmpty);
    expect(enabled(tester, 'save'), isFalse);
    controller.original
      ..saveFailure = null
      ..revision = 'retry-revision';
    await tap(tester, 'reload');
    await tap(tester, 'save');
    expect(
      controller.original.writes.last['payload']['expectedRevision'],
      'retry-revision',
    );
    expect(controller.original.config['refreshIntervalMs'], 720000);
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets(
    'unconfirmed save requires reload before any further submission',
    (tester) async {
      controller.original.saveFailure = DshException(
        'timeout',
        '超时',
        outcomeUnknown: true,
      );
      await mount(tester, controller);
      await fill(tester, 'interval', '30000');
      await tap(tester, 'save');
      expect(find.textContaining('保存结果尚未确认'), findsOneWidget);
      await fill(tester, 'interval', '45000');
      expect(enabled(tester, 'save'), isFalse);
      expect(text(tester, 'interval'), '45000');
      expect(controller.original.writes.length, 1);
      await tester.pumpWidget(const SizedBox());
      await tester.binding.setSurfaceSize(null);
    },
  );

  testWidgets('mismatched response cannot become an editable snapshot', (
    tester,
  ) async {
    controller.original.readOverride = controller.original.snapshot(
      entry: 'other',
    );
    await mount(tester, controller);
    expect(find.textContaining('返回的数据不完整'), findsOneWidget);
    expect(enabled(tester, 'save'), isFalse);
    controller.original.readOverride = null;
    await tap(tester, 'reload');
    await fill(tester, 'interval', '0');
    expect(enabled(tester, 'save'), isTrue);
    expect(controller.original.writes, isEmpty);
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets(
    'API replacement cancels pending read and ignores its late response',
    (tester) async {
      final pending = Completer<Json>();
      controller.original.pending['pluginInventory.getConfig'] = pending;
      await mount(tester, controller, settle: false);
      controller.changeApi();
      await tester.pump();
      expect(controller.original.scopes.single.cancelled, isTrue);
      pending.complete({
        ...controller.original.snapshot(),
        'config': {'refreshIntervalMs': 0},
      });
      await tester.pumpAndSettle();
      expect(text(tester, 'interval'), '');
      expect(find.textContaining('连接或插件已变化'), findsOneWidget);
      expect(enabled(tester, 'save'), isFalse);
      expect(controller.replacement!.calls, isEmpty);
      await tester.pumpWidget(const SizedBox());
      await tester.binding.setSurfaceSize(null);
    },
  );

  testWidgets(
    'Host change invalidates a save even when the API object is reused',
    (tester) async {
      await mount(tester, controller);
      await fill(tester, 'interval', '30000');
      final pending = Completer<Json>();
      controller.original.pending['pluginInventory.setConfig'] = pending;
      await tap(tester, 'save', settle: false);
      controller.changeHost();
      await tester.pump();
      expect(controller.original.scopes.last.cancelled, isTrue);
      pending.complete({
        ...controller.original.snapshot(),
        'config': {'refreshIntervalMs': 0},
      });
      await tester.pumpAndSettle();
      expect(text(tester, 'interval'), '30000');
      expect(find.textContaining('配置已保存'), findsNothing);
      expect(enabled(tester, 'reload'), isFalse);
      expect(find.textContaining('连接或插件已变化'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      await tester.binding.setSurfaceSize(null);
    },
  );

  testWidgets('entry replacement cannot apply a late save to another plugin', (
    tester,
  ) async {
    await mount(tester, controller);
    await fill(tester, 'interval', '30000');
    final pending = Completer<Json>();
    controller.original.pending['pluginInventory.setConfig'] = pending;
    await tap(tester, 'save', settle: false);
    await mount(tester, controller, entryId: 'second-clock');
    pending.complete(controller.original.snapshot());
    await tester.pumpAndSettle();
    expect(text(tester, 'interval'), '30000');
    expect(find.textContaining('连接或插件已变化'), findsOneWidget);
    expect(controller.original.writes.single['payload']['entryId'], 'clock');
    expect(enabled(tester, 'save'), isFalse);
    await tester.pumpWidget(const SizedBox());
    await tester.binding.setSurfaceSize(null);
  });

  for (final method in [
    'pluginInventory.getConfig',
    'pluginInventory.setConfig',
  ]) {
    testWidgets('unmount cancels $method and ignores late completion', (
      tester,
    ) async {
      final pending = Completer<Json>();
      if (method == 'pluginInventory.getConfig') {
        controller.original.pending[method] = pending;
        await mount(tester, controller, settle: false);
      } else {
        await mount(tester, controller);
        await fill(tester, 'interval', '30000');
        controller.original.pending[method] = pending;
        await tap(tester, 'save', settle: false);
      }
      final requestScope = controller.original.scopes.last;
      await tester.pumpWidget(const SizedBox());
      expect(requestScope.cancelled, isTrue);
      pending.complete(controller.original.snapshot());
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      await tester.binding.setSurfaceSize(null);
    });
  }
}
