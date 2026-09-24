import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/plan_preview.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/src/resource_diagnostics.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

TranscriptItem plan(int i, [String? text]) => TranscriptItem(
  id: 'plan-$i',
  kind: 'tool',
  text: '{}',
  title: 'Plan $i',
  planText: text ?? '# Plan $i\n\nRead-only snapshot.',
);

class PreviewPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class PreviewController extends DesktopController {
  PreviewController() : super(PreviewPreferences()) {
    selectedId = 's';
    transcript = [plan(1)];
  }
  @override
  bool get connected => true;
}

void main() {
  testWidgets(
    'dismissing a narrow drawer releases previews and their widgets',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(800, 700));
      final c = PreviewController();
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('preview-plan-plan-1')));
      await tester.pumpAndSettle();
      final shell = tester.state(find.byType(Workbench)) as ResourceDiagnostics;
      expect(shell.resourceDiagnostics['planPreviewCacheEntries'], 1);
      tester.state<ScaffoldState>(find.byType(Scaffold)).closeEndDrawer();
      await tester.pumpAndSettle();
      expect(shell.resourceDiagnostics['planPreviewCacheBytes'], 0);
      expect(find.byType(PlanPreviewPanel), findsNothing);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets('starting another conversation clears the old preview scope', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1400, 900));
    final c = PreviewController();
    await tester.pumpWidget(DesktopApp(controller: c));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('preview-plan-plan-1')));
    await tester.pumpAndSettle();
    final shell = tester.state(find.byType(Workbench)) as ResourceDiagnostics;
    c.newConversation();
    await tester.pumpAndSettle();
    expect(shell.resourceDiagnostics['planPreviewCacheBytes'], 0);
    expect(find.byType(PlanPreviewPanel), findsNothing);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await tester.binding.setSurfaceSize(null);
    expect(tester.takeException(), isNull);
  });
  test(
    'snapshots have a fixed budget, eviction order and host/session isolation',
    () {
      final store = PlanPreviewStore(), host = Object();
      store.scope(host, 'a');
      for (var i = 0; i < 9; i++) {
        store.open(plan(i, 'x' * PlanPreviewStore.maxTextUnits));
      }
      expect(store.items.length, 8);
      expect(store.items.first.sourceId, 'plan-1');
      expect(store.retainedBytes, PlanPreviewStore.maxRetainedBytes);
      expect(
        () => store.open(plan(10, 'x' * (PlanPreviewStore.maxTextUnits + 1))),
        throwsStateError,
      );
      expect(store.items.length, 8);
      store.scope(host, 'a');
      expect(store.items.length, 8);
      store.scope(host, 'b');
      expect(store.retainedBytes, 0);
      store.open(plan(1));
      store.scope(Object(), 'b');
      expect(store.items, isEmpty);
      store.open(plan(2));
      store.close(store.activeId!);
      expect(store.retainedBytes, 0);
      store.dispose();
    },
  );
  testWidgets(
    'preview copies exact snapshot, returns to source and never exposes approval actions',
    (tester) async {
      final store = PlanPreviewStore()..open(plan(1));
      String? copied;
      var returned = 0;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied = (call.arguments as Map)['text'] as String;
          }
          return null;
        },
      );
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: PlanPreviewPanel(store: store, onSource: () => returned++),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('确认执行'), findsNothing);
      expect(find.text('拒绝'), findsNothing);
      await tester.tap(find.text('复制'));
      await tester.pump();
      expect(copied, plan(1).planText);
      await tester.tap(find.text('返回来源'));
      expect(returned, 1);
      await tester.tap(find.byTooltip('关闭计划 Plan 1'));
      await tester.pump();
      expect(store.retainedBytes, 0);
      expect(find.textContaining('预览已失效'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      store.dispose();
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      );
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'history opens a native dock tab and closing the dock releases all snapshots',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1400, 900));
      final c = PreviewController();
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('preview-plan-plan-1')));
      await tester.pumpAndSettle();
      expect(find.byType(PlanPreviewPanel), findsOneWidget);
      final shell = tester.state(find.byType(Workbench)) as ResourceDiagnostics;
      expect(shell.resourceDiagnostics['planPreviewCacheEntries'], 1);
      await tester.tap(find.byTooltip('关闭工作台'));
      await tester.pumpAndSettle();
      expect(shell.resourceDiagnostics['planPreviewCacheBytes'], 0);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
      expect(tester.takeException(), isNull);
    },
  );
}
