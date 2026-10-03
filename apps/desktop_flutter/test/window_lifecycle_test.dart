import 'dart:async';
import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/src/window_lifecycle.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:window_manager/window_manager.dart';

class LifecycleController extends DesktopController {
  LifecycleController(super.preferences);
  int stopCalls = 0;
  bool disposed = false;
  @override
  Future<void> stop() async {
    stopCalls++;
  }

  @override
  void dispose() {
    if (disposed) return;
    disposed = true;
    super.dispose();
  }
}

const windowChannel = MethodChannel('window_manager');

Future<void> nativeClose(WidgetTester tester) async {
  final response = Completer<void>();
  // Deliver the same event that the installed window_manager native plugin sends.
  tester.binding.defaultBinaryMessenger.handlePlatformMessage(
    windowChannel.name,
    const StandardMethodCodec().encodeMethodCall(
      const MethodCall('onEvent', {'eventName': 'close'}),
    ),
    (_) => response.complete(),
  );
  await response.future;
}

void main() {
  testWidgets(
    'failed native interception reports a retry action without failing app startup',
    (tester) async {
      final c = LifecycleController(DesktopPreferences(writer: (_) async {}));
      String? reported;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        windowChannel,
        (call) async {
          throw PlatformException(code: 'window-hook-unavailable');
        },
      );
      addTearDown(() {
        c.dispose();
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          windowChannel,
          null,
        );
      });
      await installWindowLifecycle(c, onError: (message) => reported = message);
      expect(c.error, contains('保存保护未能启用'));
      expect(c.error, contains('重启桌面应用后重试'));
      expect(c.error, contains('window-hook-unavailable'));
      expect(reported, c.error);
      expect(
        windowManager.listeners.whereType<WindowLifecycleBinding>().where(
          (binding) => binding.controller == c,
        ),
        isEmpty,
      );
    },
  );
  testWidgets(
    'native close saves the unnamed draft before handing the close to Windows',
    (tester) async {
      final order = <String>[];
      final writes = <Json>[];
      final saved = Completer<void>();
      final c = LifecycleController(
        DesktopPreferences(
          writer: (content) async {
            order.add('save');
            writes.add(object(jsonDecode(content)));
            await saved.future;
          },
        ),
      )..workspaceId = 'workspace';
      c.sessions = [
        SessionSummary.fromJson({'sessionId': 'session', 'running': true}),
      ];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        windowChannel,
        (call) async {
          order.add(
            call.method == 'setPreventClose'
                ? 'setPreventClose:${object(call.arguments)['isPreventClose']}'
                : call.method,
          );
          return null;
        },
      );
      final binding = WindowLifecycleBinding(c);
      addTearDown(() {
        binding.dispose();
        c.dispose();
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          windowChannel,
          null,
        );
      });
      await binding.start();
      await binding.start();
      c.setDraft('关闭前最后输入的草稿');
      await nativeClose(tester);
      await tester.pump();
      expect(order, ['setPreventClose:true', 'save']);
      expect(object(writes.single['drafts'])[c.unnamedDraftKey], '关闭前最后输入的草稿');
      final closing = binding.requestClose();
      expect(binding.requestClose(), same(closing));
      await nativeClose(tester);
      expect(writes, hasLength(1));
      saved.complete();
      await closing;
      // Windows destroys the window inside its message loop; window_manager's
      // destroy() left teardown to process exit, which crashed the engine.
      expect(order, [
        'setPreventClose:true',
        'save',
        'setPreventClose:false',
        'close',
      ]);
      expect(order, isNot(contains('destroy')));
      expect(c.stopCalls, 0);
      expect(c.sessions.single.running, isTrue);
      // The close Windows echoes back must not start another save.
      await nativeClose(tester);
      expect(order.where((method) => method == 'close'), hasLength(1));
      expect(writes, hasLength(1));
      c.dispose();
    },
  );

  testWidgets(
    'save failure keeps the window and permits an explicit close retry',
    (tester) async {
      final methods = <String>[];
      var saves = 0, notifications = 0;
      final c = LifecycleController(
        DesktopPreferences(
          writer: (_) async {
            if (++saves == 1) throw StateError('存储空间不足');
          },
        ),
      )..selectedId = 'session';
      c.addListener(() => notifications++);
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        windowChannel,
        (call) async {
          methods.add(call.method);
          return null;
        },
      );
      final binding = WindowLifecycleBinding(c);
      addTearDown(() {
        binding.dispose();
        c.dispose();
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          windowChannel,
          null,
        );
      });
      await binding.start();
      c.setDraft('保留这份草稿');
      await binding.requestClose();
      expect(methods, ['setPreventClose']);
      expect(notifications, 1);
      expect(c.error, contains('窗口仍然打开'));
      expect(c.error, contains('再次关闭窗口以重试'));
      expect(c.error, contains('存储空间不足'));
      expect(c.preferences.drafts['session'], '保留这份草稿');
      await binding.requestClose();
      expect(saves, 2);
      expect(methods, ['setPreventClose', 'setPreventClose', 'close']);
      expect(c.stopCalls, 0);
      c.dispose();
    },
  );

  testWidgets(
    'edits made during a pending save are flushed before the window closes',
    (tester) async {
      final writes = <Json>[];
      final pending = Completer<void>();
      var destroyed = false;
      final c = LifecycleController(
        DesktopPreferences(
          writer: (content) async {
            writes.add(object(jsonDecode(content)));
            if (writes.length == 1) await pending.future;
          },
        ),
      )..selectedId = 'session';
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        windowChannel,
        (call) async {
          if (call.method == 'close') destroyed = true;
          return null;
        },
      );
      final binding = WindowLifecycleBinding(c);
      addTearDown(() {
        binding.dispose();
        c.dispose();
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          windowChannel,
          null,
        );
      });
      await binding.start();
      c.setDraft('原草稿');
      final closing = binding.requestClose();
      await tester.pump();
      c.setDraft('写入等待期间继续输入的草稿');
      expect(destroyed, isFalse);
      pending.complete();
      await closing;
      expect(writes, hasLength(2));
      expect(object(writes.last['drafts'])['session'], '写入等待期间继续输入的草稿');
      expect(destroyed, isTrue);
      c.dispose();
    },
  );

  testWidgets(
    'native destruction is guarded against reentry and failures remain retryable',
    (tester) async {
      var saves = 0, destroys = 0;
      final destroying = Completer<void>();
      final c = LifecycleController(
        DesktopPreferences(
          writer: (_) async {
            saves++;
          },
        ),
      );
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        windowChannel,
        (call) async {
          if (call.method == 'close') {
            destroys++;
            if (destroys == 1) await destroying.future;
          }
          return null;
        },
      );
      final binding = WindowLifecycleBinding(c);
      addTearDown(() {
        binding.dispose();
        c.dispose();
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          windowChannel,
          null,
        );
      });
      await binding.start();
      final closing = binding.requestClose();
      await tester.pump();
      expect(destroys, 1);
      expect(binding.requestClose(), same(closing));
      await nativeClose(tester);
      expect(saves, 1);
      destroying.completeError(
        PlatformException(code: 'native-close-failed', message: '暂时无法关闭窗口'),
      );
      await closing;
      expect(c.error, contains('草稿和设置已保存'));
      expect(c.error, contains('窗口未能关闭'));
      await binding.requestClose();
      expect(destroys, 2);
      expect(c.stopCalls, 0);
    },
  );

  testWidgets('detaching a binding ignores a late preference completion', (
    tester,
  ) async {
    final saved = Completer<void>();
    final methods = <String>[];
    final c = LifecycleController(
      DesktopPreferences(writer: (_) => saved.future),
    );
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      windowChannel,
      (call) async {
        methods.add(call.method);
        return null;
      },
    );
    final binding = WindowLifecycleBinding(c);
    addTearDown(() {
      c.dispose();
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        windowChannel,
        null,
      );
    });
    await binding.start();
    expect(windowManager.listeners, contains(binding));
    final closing = binding.requestClose();
    await tester.pump();
    binding.dispose();
    expect(windowManager.listeners, isNot(contains(binding)));
    saved.complete();
    await closing;
    expect(methods, ['setPreventClose']);
    expect(c.error, isNull);
  });
}
