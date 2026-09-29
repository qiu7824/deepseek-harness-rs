import 'dart:io';
import 'dart:ui' as ui;

import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/features/workbench/computer_use_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

class TestPreferences extends DesktopPreferences {
  TestPreferences(String endpoint) : super(address: endpoint);
  @override
  Future<void> save() async {}
}

/// Requires an isolated Host (DSH_TEST_HOST) with Computer Use enabled on the
/// `native-browser` adapter and a fixture model that answers the prompt
/// DSH_TEST_TASK by starting `computer_use` on an http page whose whole
/// viewport is one button that retitles the page "clicked", then replies
/// COMPUTER_USE_READY.
void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('a person joins the Computer Use session of the model', (
    tester,
  ) async {
    tester.testTextInput.register();
    addTearDown(tester.testTextInput.unregister);
    final address = Platform.environment['DSH_TEST_HOST'];
    final cwd = Platform.environment['DSH_TEST_CWD'];
    final task = Platform.environment['DSH_TEST_TASK'];
    expect(
      address,
      isNotNull,
      reason: 'DSH_TEST_HOST must point at an isolated test Host',
    );
    expect(cwd, isNotNull);
    expect(task, isNotNull);
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
        await File('$output/computer-use-$name.png')
            .writeAsBytes(bytes!.buffer.asUint8List());
      } finally {
        image.dispose();
      }
    }

    Future<void> until(String what, bool Function() predicate) async {
      final deadline = DateTime.now().add(const Duration(seconds: 60));
      while (!predicate()) {
        if (DateTime.now().isAfter(deadline)) {
          await capture('failure');
          fail('$what timed out; error=${c.error}');
        }
        await tester.pump(const Duration(milliseconds: 100));
      }
    }

    bool shown(Finder finder) => finder.evaluate().isNotEmpty;
    Finder inPanel(Finder finder) =>
        find.descendant(of: find.byType(ComputerUsePanel), matching: finder);
    final frame = inPanel(find.byKey(const ValueKey('computer-use-frame')));
    String status() =>
        inPanel(find.byKey(const ValueKey('computer-use-status')))
            .evaluate()
            .map((e) => (e.widget as Text).data ?? '')
            .join();

    await c.connect(address);
    await until('connection', () => c.connected && !c.loading);
    final id = await c.create(cwd!);
    await until('session', () => id != null && c.selectedId == id);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('prompt-input')));
    await tester.enterText(find.byKey(const Key('prompt-input')), task!);
    await tester.pump();
    await tester.tap(find.byKey(const Key('send-message')));
    // Starting a control session changes state, so it asks first.
    await until('approval', () => shown(find.text('允许一次')));
    await capture('approval');
    await tester.tap(find.text('允许一次'));

    await until('model turn', () => shown(find.text('COMPUTER_USE_READY')));
    // Browser state reports no desktop connection, so the Host records no
    // activity for it; the tab finds the model's session when opened.
    expect(c.computerUse, isNull);
    await tester.tap(find.byTooltip('显示工作台'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('workbench-add-tab')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('workbench-open-computer-use')));
    await until(
      'model page',
      () =>
          shown(inPanel(find.textContaining('Computer Use fixture'))) &&
          status() == '智能体可操作' &&
          shown(find.descendant(of: frame, matching: find.byType(Image))),
    );
    expect(status(), '智能体可操作');
    await capture('model-page');

    // The fixture page is one full-viewport button.
    await tester.tapAt(tester.getCenter(frame));
    var refreshes = 0;
    await until('click', () {
      if (shown(inPanel(find.textContaining('clicked')))) return true;
      // The page may settle after the click response; observe it again.
      if (++refreshes % 20 == 0) tester.tap(inPanel(find.text('刷新画面')));
      return false;
    });
    await capture('clicked');

    // Direct input on the frame is a takeover; the Host pauses the agent.
    await until('input takeover', () => shown(find.text('交还智能体')));
    expect(find.text('智能体控制暂停 · 控制画面收到人工输入'), findsOneWidget);
    await tester.tap(find.text('交还智能体'));
    await until('hand back', () => status() == '智能体可操作');

    await tester.tap(find.text('人工接管'));
    await until('takeover', () => shown(find.text('交还智能体')));
    expect(find.text('智能体控制暂停 · 已在控制面板选择人工接管'), findsOneWidget);
    await capture('manual');
    await tester.tap(find.text('交还智能体'));
    await until('hand back again', () => status() == '智能体可操作');

    await tester.tap(find.byKey(const ValueKey('computer-use-close')));
    await until('close', () => status() == '未连接');
    await capture('closed');
  });
}
