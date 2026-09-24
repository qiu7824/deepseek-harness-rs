import 'dart:io';
import 'dart:convert';
import 'dart:ui' as ui;

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class TestPreferences extends DesktopPreferences {
  TestPreferences(String endpoint) : super(address: endpoint);
  @override
  Future<void> save() async {}
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('Windows task creation, send, stream, cancel and reconnect', (
    tester,
  ) async {
    // Windows integration tests otherwise share the real IME, whose delayed
    // editing updates can overwrite synthetic test input.
    tester.testTextInput.register();
    addTearDown(tester.testTextInput.unregister);
    final address = Platform.environment['DSH_TEST_HOST'];
    final cwd = Platform.environment['DSH_TEST_CWD'];
    final expectedHome = Platform.environment['DSH_TEST_HOME'];
    expect(
      address,
      isNotNull,
      reason: 'DSH_TEST_HOST must point at an isolated test Host',
    );
    expect(cwd, isNotNull);
    expect(expectedHome, contains('flow-host-qa'));
    final c = DesktopController(TestPreferences(address!));
    final root = GlobalKey();
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    });
    await tester.pumpWidget(
      RepaintBoundary(
        key: root,
        child: DesktopApp(controller: c),
      ),
    );
    Future<void> capture(String name) async {
      final output = Platform.environment['DSH_QA_RESULT'];
      if (output == null) return;
      await tester.pump(const Duration(milliseconds: 100));
      final image =
          await (root.currentContext!.findRenderObject()!
                  as RenderRepaintBoundary)
              .toImage();
      try {
        final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
        await File('${File(output).parent.path}/desktop-flow-$name.png')
            .writeAsBytes(bytes!.buffer.asUint8List());
      } finally {
        image.dispose();
      }
    }

    Future<void> until(bool Function() predicate) async {
      final deadline = DateTime.now().add(const Duration(seconds: 20));
      while (!predicate()) {
        if (DateTime.now().isAfter(deadline)) {
          await capture('failure');
          fail(
            'Condition timed out: ${c.error}; connected=${c.connected}; sending=${c.sending}; running=${c.running}; session=${c.selectedId}; draft=${c.draft}; workspace=${c.workspaceId}; input=${find.byKey(const Key('prompt-input')).evaluate().isEmpty ? 'absent' : tester.widget<TextField>(find.byKey(const Key('prompt-input'))).controller!.value}; visible=${c.transcript.map((m) => m.text.length > 50 ? m.text.substring(0, 50) : m.text).toList()}',
          );
        }
        await tester.pump(const Duration(milliseconds: 50));
      }
      await tester.pump();
    }

    await c.connect(address);
    expect(c.host!.home.replaceFirst(r'\\?\', ''), expectedHome);
    await until(() => c.connected && !c.loading);
    await tester.tap(
      find.byWidgetPredicate((w) => w is DshIcon && w.label == '添加工作区').first,
    );
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('working-directory')), cwd!);
    await tester.tap(find.byKey(const Key('create-task')));
    await until(
      () => c.currentWorkspace != null && c.selectedId == null && !c.loading,
    );
    final workspace = c.workspaceId;
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('prompt-input')));
    await tester.pump();
    await tester.enterText(
      find.byKey(const Key('prompt-input')),
      'desktop-ui-hello',
    );
    await tester.pump();
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('prompt-input')))
          .controller!
          .text,
      'desktop-ui-hello',
    );
    expect(
      tester
          .widget<ShadButton>(find.byKey(const Key('send-message')))
          .onPressed,
      isNotNull,
    );
    await tester.tap(find.byKey(const Key('send-message')));
    await until(
      () => c.transcript.any((m) => m.text == '桌面客户端连接验证成功。') && !c.sending,
    );
    expect(c.error, isNull);
    final session = c.selectedId!;
    final registered = await c.client!.call('workspace.list');
    expect(
      objects(registered['items']).any(
        (w) =>
            w['workspaceId'] == workspace &&
            (w['sessionIds'] as List? ?? []).contains(session),
      ),
      isTrue,
    );
    final files = await c.client!.request(previewUrl('list', session));
    expect(files['entries'], isA<List>());
    await until(() => !c.running);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('prompt-input')));
    await tester.pump();
    await tester.enterText(
      find.byKey(const Key('prompt-input')),
      'desktop-slow',
    );
    await tester.pump();
    expect(
      c.draft,
      'desktop-slow',
      reason:
          'input=${tester.widget<TextField>(find.byKey(const Key('prompt-input'))).controller!.value}; session=${c.selectedId}; input log=${tester.testTextInput.log.map((e) => e.method).toList()}',
    );
    expect(
      tester
          .widget<ShadButton>(find.byKey(const Key('send-message')))
          .onPressed,
      isNotNull,
    );
    await tester.tap(find.byKey(const Key('send-message')));
    await until(
      () => c.running && c.transcript.any((m) => m.text.contains('slow-')),
    );
    await tester.tap(find.byKey(const Key('stop-task')));
    await until(() => !c.running);
    await c.connect(address);
    await until(() => c.connected && c.selectedId == session && !c.loading);
    expect(c.transcript.any((m) => m.text == 'desktop-ui-hello'), isTrue);
    expect(
      c.transcript.where(
        (m) => m.kind == 'user' && m.text == 'desktop-ui-hello',
      ),
      hasLength(1),
    );
    expect(
      c.transcript.where((m) => m.kind == 'user' && m.text == 'desktop-slow'),
      hasLength(1),
    );
    expect(c.transcript.any((m) => m.streaming), isFalse);
    await capture('reconnected');
    Future<void> sendText(String text) async {
      await tester.enterText(find.byKey(const Key('prompt-input')), text);
      await tester.pump();
      await tester.tap(find.byKey(const Key('send-message')));
      await tester.pump();
    }

    await tester.tap(find.byKey(const Key('new-task')));
    await tester.pump();
    c.preset = 'standard';
    await sendText('desktop-question');
    await until(
      () => c.interactions.any((f) => f.type == 'question/requested'),
    );
    await capture('question');
    await tester.tap(find.text('方案二'));
    await tester.pump();
    await tester.enterText(
      find.byKey(const ValueKey('answer-detail')),
      '原生客户端完整回答',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('question-continue')));
    await until(() => c.interactions.isEmpty && !c.running && !c.sending);
    expect(c.error, isNull);
    await c.loadHistory();
    await tester.pump();
    final questionRow = c.transcript.firstWhere(
      (m) => m.kind == 'tool' && m.title == '提问',
    );
    expect(questionRow.summary, '2/2 已回答');
    await tester.ensureVisible(
      find.byKey(ValueKey('tool-row-${questionRow.id}')),
    );
    await tester.tap(find.byKey(ValueKey('tool-row-${questionRow.id}')));
    await tester.pump();
    expect(find.byKey(ValueKey('tool-body-${questionRow.id}')), findsOneWidget);
    await capture('question-result');
    await tester.tap(find.byKey(ValueKey('tool-row-${questionRow.id}')));
    await tester.pump();
    await sendText('desktop-approval');
    await until(
      () => c.interactions.any((f) => f.type == 'approval/requested'),
    );
    final firstApproval = c.interactions
        .firstWhere((f) => f.type == 'approval/requested')
        .rpcId;
    await capture('approval');
    await tester.tap(find.text('允许一次'));
    await until(() => c.interactions.isEmpty && !c.running && !c.sending);
    expect(c.error, isNull);
    await sendText('desktop-approval-again');
    await until(
      () => c.interactions.any(
        (f) => f.type == 'approval/requested' && f.rpcId != firstApproval,
      ),
    );
    await tester.tap(find.text('拒绝'));
    await until(() => c.interactions.isEmpty && !c.running && !c.sending);
    expect(c.error, isNull);
    await c.loadHistory();
    expect(
      c.transcript.any((m) => m.title == '已读取文件' && m.filePath != null),
      isTrue,
    );
    expect(c.transcript.any((m) => m.title == '写入文件失败'), isTrue);
    expect(
      c.transcript.any((m) => m.kind == 'tool' && m.status == 'failed'),
      isTrue,
    );
    await sendText('desktop-question-skip');
    await until(
      () => c.interactions.any((f) => f.type == 'question/requested'),
    );
    await tester.tap(find.text('跳过本题'));
    await tester.pump();
    await tester.tap(find.text('跳过本题'));
    await until(() => c.interactions.isEmpty && !c.running && !c.sending);
    expect(c.error, isNull);
    await sendText('desktop-question-cancel');
    await until(
      () => c.interactions.any((f) => f.type == 'question/requested'),
    );
    await tester.tap(find.byTooltip('放弃整组问题'));
    await until(() => c.interactions.isEmpty && !c.running && !c.sending);
    expect(c.error, isNull);
    expect(tester.takeException(), isNull);
    final output = Platform.environment['DSH_QA_RESULT'];
    if (output != null) {
      await File(output).writeAsString(
        jsonEncode({
          'processId': pid,
          'hostVersion': c.host!.version,
          'workspaceCreatedByUI': true,
          'sessionRegisteredInWorkspace': true,
          'sendAndStreamThroughUI': true,
          'stopThroughUI': true,
          'workspaceFileList': true,
          'reconnectRetainsHistoryWithoutDuplicateUsers': true,
          'batchQuestionsAnsweredThroughUI': true,
          'skipAllQuestionsThroughUI': true,
          'cancelQuestionBatchThroughUI': true,
          'localizedFileActivities': true,
          'questionHistoryCountAndInlineDetails': true,
          'approvalAllowedOnceAndWriteRejectedThroughUI': true,
          'scope': 'Windows native Profile integration test with real installed Host and local deterministic model provider; not a Release memory soak',
        }),
      );
    }
  });
}
