import 'dart:async';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/session_log_export.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

class RecordingExportClient extends FakeClient {
  String? requestedPath;
  int? requestedMaxBytes;
  Duration? requestedTimeout;

  @override
  Future<int> downloadTo(
    String path,
    File destination, {
    RequestScope? scope,
    int maxBytes = 64 * 1024 * 1024,
    Duration? totalTimeout,
    void Function(int bytes)? onProgress,
  }) async {
    requestedPath = path;
    requestedMaxBytes = maxBytes;
    requestedTimeout = totalTimeout;
    destination.writeAsBytesSync([80, 75, 3, 4]);
    return 4;
  }
}

class PendingExportClient extends FakeClient {
  final started = Completer<RequestScope?>();
  final finish = Completer<int>();

  @override
  Future<int> downloadTo(
    String path,
    File destination, {
    RequestScope? scope,
    int maxBytes = 64 * 1024 * 1024,
    Duration? totalTimeout,
    void Function(int bytes)? onProgress,
  }) {
    started.complete(scope);
    return finish.future;
  }
}

void main() {
  test('Host collaboration visibility follows agent-teams settings', () async {
    final api = FakeClient()
      ..handleCall = (method, _) async => method == 'settings.describe'
          ? {
              'namespaces': [
                {
                  'ns': 'agent-teams',
                  'value': {'showButton': false},
                },
              ],
            }
          : {'items': [], 'entries': [], 'presets': []};
    final controller = DesktopController(
      MemoryPreferences(),
      clientFactory: (_) => api,
    );
    await controller.connect('http://127.0.0.1');
    await controller.loadCatalogs();
    expect(controller.teamSettings['showButton'], false);
    controller.dispose();
  });

  test(
    'session archive requests descendants and preserves ZIP bytes',
    () async {
      final directory = await Directory.systemTemp.createTemp('dsh-export-');
      final api = RecordingExportClient();
      try {
        final destination = File('${directory.path}/session.zip');
        expect(await downloadSessionLog(api, 'id/子 会话', destination), 4);
        expect(await destination.readAsBytes(), [80, 75, 3, 4]);
        final uri = Uri.parse(api.requestedPath!);
        expect(uri.path, '/api/session.export');
        expect(uri.queryParameters['sessionId'], 'id/子 会话');
        expect(uri.queryParameters['includeDescendants'], 'true');
        expect(sessionLogFilename('id/子 会话'), 'dsh-session-id_____.zip');
        expect(api.requestedMaxBytes, 1 << 40);
        expect(api.requestedTimeout, const Duration(hours: 2));
      } finally {
        await api.close();
        await directory.delete(recursive: true);
      }
    },
  );

  testWidgets('canceling save location closes export without a Host request', (
    tester,
  ) async {
    final api = FakeClient();
    final controller = DesktopController(
      MemoryPreferences(),
      clientFactory: (_) => api,
    );
    await controller.connect('http://127.0.0.1');
    controller.selectedId = 'session-1';
    String? suggested;
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SessionLogExportAction(
            controller: controller,
            sessionId: 'session-1',
            pickLocation: (name) async {
              suggested = name;
              return null;
            },
          ),
        ),
      ),
    );
    await tester.tap(find.byTooltip('下载会话日志'));
    await tester.pumpAndSettle();
    expect(suggested, 'dsh-session-session-1.zip');
    expect(find.text('正在导出 Session'), findsNothing);
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
  });

  testWidgets('header action saves the archive and reports completion', (
    tester,
  ) async {
    final directory = Directory.systemTemp.createTempSync('dsh-export-ui-');
    final api = RecordingExportClient();
    final controller = DesktopController(
      MemoryPreferences(),
      clientFactory: (_) => api,
    );
    await controller.connect('http://127.0.0.1');
    controller.selectedId = 'session-1';
    final path = '${directory.path}/session.zip';
    try {
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SessionLogExportAction(
              controller: controller,
              sessionId: 'session-1',
              pickLocation: (_) async => path,
            ),
          ),
        ),
      );
      await tester.tap(find.byTooltip('下载会话日志'));
      await tester.pumpAndSettle();
      expect(find.text('Session 导出完成'), findsOneWidget);
      expect(File(path).readAsBytesSync(), [80, 75, 3, 4]);
      expect(
        Uri.parse(api.requestedPath!).queryParameters['sessionId'],
        'session-1',
      );
      await tester.tap(find.text('关闭'));
      await tester.pumpAndSettle();
      expect(find.text('Session 导出完成'), findsNothing);
    } finally {
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
      directory.deleteSync(recursive: true);
    }
  });

  testWidgets('switching sessions closes a pending export', (tester) async {
    final api = PendingExportClient();
    final controller = DesktopController(
      MemoryPreferences(),
      clientFactory: (_) => api,
    );
    await controller.connect('http://127.0.0.1');
    controller.selectedId = 'session-1';
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SessionLogExportAction(
            controller: controller,
            sessionId: 'session-1',
            pickLocation: (_) async => 'D:/codex操作目录/switch-test.zip',
          ),
        ),
      ),
    );
    await tester.tap(find.byTooltip('下载会话日志'));
    await tester.pump();
    final scope = await api.started.future;
    expect(find.text('正在导出 Session'), findsOneWidget);
    controller.selectedId = 'session-2';
    controller.emit();
    await tester.pumpAndSettle();
    expect(find.text('正在导出 Session'), findsNothing);
    expect(scope?.cancelled, isTrue);
    api.finish.complete(0);
    await tester.pump();
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
  });
}
