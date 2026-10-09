import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/src/resource_diagnostics.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

class _DeleteClient extends DshClient {
  _DeleteClient() : super('http://127.0.0.1:9');
}

class _DeleteController extends DesktopController {
  _DeleteController() : super(DesktopPreferences(writer: (_) async {}));
  final api = _DeleteClient();
  final calls = <({String id, bool stopSchedules, DshClient? client})>[];
  bool schedules = false;
  Object? failure;

  @override
  DshClient get client => api;
  @override
  bool get connected => true;
  @override
  Future<void> deleteSession(
    String id, {
    bool stopSchedules = false,
    DshClient? expectedClient,
  }) async {
    calls.add((id: id, stopSchedules: stopSchedules, client: expectedClient));
    if (failure != null) throw failure!;
    if (schedules && !stopSchedules) {
      throw DshException(
        'agent-busy',
        '会话有有效提醒',
        details: {'reason': 'active-schedules'},
      );
    }
    sessions.removeWhere((row) => row.id == id);
    if (selectedId == id) newConversation();
    emit();
  }
}

Future<_DeleteController> _mount(
  WidgetTester tester, {
  bool running = false,
}) async {
  await tester.binding.setSurfaceSize(const Size(1280, 800));
  final controller = _DeleteController()
    ..selectedId = 'saved'
    ..sessions = [
      SessionSummary.fromJson({
        'sessionId': 'saved',
        'displayTitle': '会话记录',
        'cwd': r'E:\project',
        'running': running,
      }),
    ]
    ..host = HostInfo.fromJson({'home': 'one', 'cwd': 'one', 'version': 'one'});
  addTearDown(() async {
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
    await controller.api.close();
    await tester.binding.setSurfaceSize(null);
  });
  await tester.pumpWidget(DesktopApp(controller: controller));
  if (running) {
    await tester.pump(const Duration(milliseconds: 500));
  } else {
    await tester.pumpAndSettle();
  }
  return controller;
}

Future<void> _menu(WidgetTester tester) async {
  await tester.tap(
    find.byKey(const ValueKey('session-saved')),
    buttons: kSecondaryMouseButton,
  );
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
}

Future<void> _delete(WidgetTester tester) async {
  await _menu(tester);
  await tester.tap(find.byKey(const ValueKey('delete-session-saved')));
  await tester.pumpAndSettle();
}

Future<void> _confirm(WidgetTester tester, String title) async {
  await tester.tap(
    find.descendant(of: find.byType(DshButton), matching: find.text(title)),
  );
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 350));
  await tester.pump();
}

void main() {
  testWidgets('cancel leaves the session and current selection intact', (
    tester,
  ) async {
    final c = await _mount(tester);
    await _delete(tester);
    expect(find.textContaining('工作区文件保留'), findsOneWidget);
    await _confirm(tester, '取消');
    expect(c.calls, isEmpty);
    expect(c.selectedId, 'saved');
    expect(find.byKey(const ValueKey('session-saved')), findsOneWidget);
  });

  testWidgets('confirmed delete invokes the guarded controller operation', (
    tester,
  ) async {
    final c = await _mount(tester);
    await _delete(tester);
    await _confirm(tester, '删除会话');
    expect(c.calls, [(id: 'saved', stopSchedules: false, client: c.api)]);
    expect(c.selectedId, isNull);
    expect(find.byKey(const ValueKey('session-saved')), findsNothing);
    expect(tester.takeException(), isNull);
  });

  for (final proceed in [false, true]) {
    testWidgets('active reminders require a second confirmation: $proceed', (
      tester,
    ) async {
      final c = await _mount(tester);
      c.schedules = true;
      await _delete(tester);
      await _confirm(tester, '删除会话');
      expect(find.text('停止提醒和定时任务并删除会话？'), findsOneWidget);
      await _confirm(tester, proceed ? '停止并删除' : '取消');
      expect(
        c.calls.map((call) => call.stopSchedules),
        proceed ? [false, true] : [false],
      );
      expect(c.selectedId, proceed ? isNull : 'saved');
      expect(
        find.byKey(const ValueKey('session-saved')),
        proceed ? findsNothing : findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('a running session cannot be deleted from its menu', (
    tester,
  ) async {
    final c = await _mount(tester, running: true);
    await _menu(tester);
    final item = tester.widget<PopupMenuItem<String>>(
      find.byKey(const ValueKey('delete-session-saved')),
    );
    expect(item.enabled, isFalse);
    expect(c.calls, isEmpty);
  });

  testWidgets('Host switch invalidates an open delete confirmation', (
    tester,
  ) async {
    final c = await _mount(tester);
    await _delete(tester);
    c.host = HostInfo.fromJson({'home': 'two', 'cwd': 'two', 'version': 'two'});
    c.emit();
    await tester.pump();
    await _confirm(tester, '删除会话');
    expect(c.calls, isEmpty);
    expect(c.selectedId, 'saved');
  });

  testWidgets('failed deletion keeps the session and exposes a retry', (
    tester,
  ) async {
    final c = await _mount(tester);
    c.failure = StateError('服务拒绝删除');
    await _delete(tester);
    await _confirm(tester, '删除会话');
    expect(c.selectedId, 'saved');
    expect(find.byKey(const ValueKey('session-saved')), findsOneWidget);
    expect(find.byTooltip('“会话记录”同步失败'), findsOneWidget);
    final diagnostics =
        tester.state(find.byType(Workbench)) as ResourceDiagnostics;
    expect(diagnostics.resourceDiagnostics['sessionSyncRetries'], 1);
    c.sessions.clear();
    c.newConversation();
    await tester.pumpAndSettle();
    expect(diagnostics.resourceDiagnostics['sessionSyncFailures'], 0);
    expect(diagnostics.resourceDiagnostics['sessionSyncRetries'], 0);
    expect(tester.takeException(), isNull);
  });
}
