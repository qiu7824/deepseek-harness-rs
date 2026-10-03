import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/computer_use_panel.dart';
import 'package:dsh_desktop/features/workbench/reclaimable_preview.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/src/resource_diagnostics.dart';
import 'package:flutter/material.dart' hide TabController;
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'computer_use_panel_test.dart'
    show ComputerUseHost, desktopSurface, frame, pixel;
import 'workbench_tabs_test.dart'
    show
        TabApi,
        TabController,
        TabHarness,
        TabHarnessState,
        openTab,
        selectTab,
        cleanup;

class PendingSourceApi extends TabApi {
  final pending = Completer<Json>();
  RequestScope? sourceScope;

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) {
    if (Uri.parse(path).path.endsWith('/source')) {
      sourceScope = scope;
      return pending.future;
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

class PreviewImageApi extends TabApi {
  RequestScope? imageScope;
  String? imageRoute;
  @override
  Future<Uint8List> bytes(
    String path, {
    Json? body,
    RequestScope? scope,
    int maxBytes = 16 * 1024 * 1024,
    bool mutation = false,
  }) async {
    imageScope = scope;
    imageRoute = path;
    return base64Decode(pixel);
  }
}

class DelayedForegroundHost extends ComputerUseHost {
  final response = Completer<Json>();
  RequestScope? foregroundScope;

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) {
    if (body?['action'] == 'click') {
      foregroundScope = scope;
      return response.future;
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

void pauseApp(WidgetTester tester) {
  for (final state in [
    AppLifecycleState.inactive,
    AppLifecycleState.hidden,
    AppLifecycleState.paused,
  ]) {
    tester.binding.handleAppLifecycleStateChanged(state);
  }
}

void resumeApp(WidgetTester tester) {
  final binding = tester.binding;
  if (binding.lifecycleState == AppLifecycleState.paused) {
    binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
  }
  if (binding.lifecycleState == AppLifecycleState.hidden) {
    binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
  }
  if (binding.lifecycleState != AppLifecycleState.resumed) {
    binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
  }
}

void main() {
  for (final pressure in [false, true]) {
    testWidgets(
      'paused file preview disposes without vsync on ${pressure ? 'memory pressure' : 'expiry'}',
      (tester) async {
        final api = TabApi();
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: FilePanel(
                api: api,
                session: 'a',
                cwd: '/fixture',
                fileRequest: const FileOpenRequest('notes.txt'),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        final old = tester.state(find.byType(NativeFileViewer));
        final panel =
            tester.state(find.byType(FilePanel)) as ResourceDiagnostics;
        pauseApp(tester);
        addTearDown(() => resumeApp(tester));
        expect(tester.binding.framesEnabled, isFalse);
        if (pressure) {
          tester.binding.handleMemoryPressure();
        } else {
          await tester.pump(const Duration(minutes: 2));
        }
        expect(old.mounted, isFalse);
        expect(panel.resourceDiagnostics['documentCacheBytes'], 0);
        resumeApp(tester);
        await tester.pumpAndSettle();
        expect(find.byType(NativeFileViewer), findsOneWidget);
        expect(
          api.requests.where((request) => request.operation == 'source').length,
          2,
        );
        await tester.pumpWidget(const SizedBox());
        await api.close();
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets('paused Computer Use evicts its frame without a resumed vsync', (
    tester,
  ) async {
    await desktopSurface(tester);
    final api = ComputerUseHost();
    final key = GlobalKey<ComputerUsePanelState>();
    await tester.pumpWidget(
      frame(ComputerUsePanel(key: key, api: api, session: 's')),
    );
    await tester.pumpAndSettle();
    final panel = key.currentState!;
    pauseApp(tester);
    addTearDown(() => resumeApp(tester));
    await tester.pump(const Duration(minutes: 2));
    expect(panel.resourceDiagnostics['computerUseFrameBytes'], 0);
    expect(panel.resourceDiagnostics['computerUsePendingEvictions'], 0);
    expect(panel.resourceDiagnostics['computerUseDisplayedImageSlots'], 0);
    expect(api.names, isNot(contains('close')));
    resumeApp(tester);
    await tester.pumpAndSettle();
    expect(panel.resourceDiagnostics['computerUseFrameBytes'], greaterThan(0));
    await tester.pumpWidget(const SizedBox());
    await api.close();
    expect(tester.takeException(), isNull);
  });

  for (final close in [false, true]) {
    testWidgets(
      'late foreground capture cannot recreate a ${close ? 'closed' : 'reclaimed hidden'} Computer Use frame',
      (tester) async {
        await desktopSurface(tester);
        final api = DelayedForegroundHost();
        final key = GlobalKey<ComputerUsePanelState>();
        var visible = true;
        late StateSetter update;
        await tester.pumpWidget(
          frame(
            StatefulBuilder(
              builder: (_, setState) {
                update = setState;
                return TickerMode(
                  enabled: visible,
                  child: ComputerUsePanel(key: key, api: api, session: 's'),
                );
              },
            ),
          ),
        );
        await tester.pumpAndSettle();
        final panel = key.currentState!;
        final pending = panel.action('click', {'x': 1, 'y': 1});
        await tester.pump();
        if (close) {
          await tester.pumpWidget(const SizedBox());
        } else {
          update(() => visible = false);
          await tester.pump();
          await tester.pump(const Duration(minutes: 2));
          await tester.pump();
        }
        expect(api.foregroundScope!.cancelled, close);
        expect(panel.resourceDiagnostics['computerUseFrameBytes'], 0);
        api.response.complete({
          'control': {
            'mode': 'manual',
            'generation': 9,
            'pauseReason': 'gui-input',
          },
          'screenshot': {'base64': pixel},
        });
        await pending;
        await tester.pump();
        expect(panel.resourceDiagnostics['computerUseFrameBytes'], 0);
        expect(panel.resourceDiagnostics['computerUseRequests'], 0);
        if (!close) expect(panel.control?['generation'], 9);
        expect(api.names, isNot(contains('close')));
        await tester.pumpWidget(const SizedBox());
        await api.close();
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets(
    'a Computer Use view mounted in an inactive app waits to poll and decode',
    (tester) async {
      await desktopSurface(tester);
      final api = ComputerUseHost()..adapters = {'local': 'native-desktop'};
      final key = GlobalKey<ComputerUsePanelState>();
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      addTearDown(
        () => tester.binding.handleAppLifecycleStateChanged(
          AppLifecycleState.resumed,
        ),
      );
      await tester.pumpWidget(
        frame(ComputerUsePanel(key: key, api: api, session: 's')),
      );
      await tester.pump();
      await tester.pump();
      final panel = key.currentState!;
      expect(panel.resourceDiagnostics['computerUseFrameBytes'], 0);
      expect(panel.resourceDiagnostics['computerUsePollTimers'], 0);
      final count = api.actions.length;
      await tester.pump(const Duration(seconds: 3));
      expect(api.actions.length, count);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      await tester.pump();
      expect(
        panel.resourceDiagnostics['computerUseFrameBytes'],
        greaterThan(0),
      );
      await tester.pumpWidget(const SizedBox());
      await api.close();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'restored search advances from its last match rather than the reading line',
    (tester) async {
      final api = PendingSourceApi();
      api.pending.complete({
        'text': List.generate(
          260,
          (index) => index == 100 || index == 200 ? 'needle' : 'line $index',
        ).join('\n'),
      });
      final cache = ResourceCache<String, String>(
        maxBytes: 16384,
        maxEntries: 1,
        sizeOf: (text) => text.length * 2,
      );
      final position = FilePreviewPosition()
        ..source = true
        ..query = 'needle'
        ..match = 100
        ..line = 220
        ..offset = 4500;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: NativeFileViewer(
              api: api,
              session: 'a',
              path: 'source.txt',
              cache: cache,
              position: position,
              onPage: (_) {},
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('查找下一个'));
      await tester.pumpAndSettle();
      expect(position.match, 200);
      await tester.pumpWidget(const SizedBox());
      await api.close();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'closing an authorized image preview evicts its decoded cache key',
    (tester) async {
      final api = PreviewImageApi();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: FilePanel(
              api: api,
              session: 'a',
              cwd: '/workspace',
              fileRequest: const FileOpenRequest('picture.png'),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final viewer =
          tester.state(find.byType(NativeFileViewer)) as ResourceDiagnostics;
      final provider = tester.widget<Image>(find.byType(Image).first).image;
      final key = await provider.obtainKey(ImageConfiguration.empty);
      expect(Uri.parse(api.imageRoute!).queryParameters['sessionId'], 'a');
      expect(viewer.resourceDiagnostics['fileBinaryBytes'], greaterThan(0));
      await tester.tap(find.byTooltip('关闭 picture.png'));
      await tester.pumpAndSettle();
      expect(viewer.resourceDiagnostics['fileBinaryBytes'], 0);
      expect(api.imageScope!.cancelled, isTrue);
      final status = PaintingBinding.instance.imageCache.statusForKey(key);
      expect(status.live || status.keepAlive || status.pending, isFalse);
      await tester.pumpWidget(const SizedBox());
      await api.close();
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'hidden file preview releases renderers and restores search and location',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1000, 750));
      final api = TabApi(), controller = TabController(api);
      final harness = GlobalKey<TabHarnessState>();
      await tester.pumpWidget(TabHarness(key: harness, controller: controller));
      await tester.pumpAndSettle();
      harness.currentState!.open(
        'files',
        fileRequest: const FileOpenRequest('notes.txt'),
      );
      await tester.pumpAndSettle();
      final query = find.byWidgetPredicate(
        (w) => w is TextField && w.decoration?.hintText == '查找内容',
      );
      await tester.enterText(query, 'Second');
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await tester.pump();
      final old = tester.state(find.byType(NativeFileViewer));
      final file = tester.state(find.byType(FilePanel)) as ResourceDiagnostics;
      final position = tester
          .widget<NativeFileViewer>(find.byType(NativeFileViewer))
          .position!;
      expect(position.query, 'Second');
      await openTab(tester, 'git');
      await tester.pump(const Duration(seconds: 119));
      expect(old.mounted, isTrue);
      await tester.pump(const Duration(seconds: 2));
      await tester.pump();
      expect(old.mounted, isFalse);
      expect(
        (old as ResourceDiagnostics).resourceDiagnostics['indexedLinesBytes'],
        0,
      );
      expect(file.resourceDiagnostics['documentCacheBytes'], 0);
      await selectTab(tester, 'files');
      await tester.pumpAndSettle();
      expect(
        tester.widget<NativeFileViewer>(find.byType(NativeFileViewer)).position,
        same(position),
      );
      expect(find.text('Second'), findsOneWidget);
      expect(api.requests.where((r) => r.operation == 'source').length, 2);
      await tester.tap(find.byTooltip('关闭 notes.txt'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('notes.txt'));
      await tester.pumpAndSettle();
      expect(find.text('Second'), findsOneWidget);
      await cleanup(tester, controller, [api]);
    },
  );

  testWidgets(
    'memory pressure cancels a hidden load and ignores its late response',
    (tester) async {
      final api = PendingSourceApi();
      final cache = ResourceCache<String, String>(
        maxBytes: 1024,
        maxEntries: 1,
        sizeOf: (text) => text.length * 2,
      );
      var visible = true;
      late StateSetter update;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: StatefulBuilder(
              builder: (_, setState) {
                update = setState;
                return TickerMode(
                  enabled: visible,
                  child: ReclaimablePreview(
                    onRelease: cache.clear,
                    builder: (_) => NativeFileViewer(
                      api: api,
                      session: 'a',
                      path: 'notes.txt',
                      cache: cache,
                      onPage: (_) {},
                    ),
                  ),
                );
              },
            ),
          ),
        ),
      );
      await tester.pump();
      final old = tester.state(find.byType(NativeFileViewer));
      update(() => visible = false);
      await tester.pump();
      tester.binding.handleMemoryPressure();
      await tester.pump();
      expect(old.mounted, isFalse);
      expect(api.sourceScope!.cancelled, isTrue);
      api.pending.complete({'text': 'late private document'});
      await tester.pump();
      expect(cache.bytes, 0);
      expect(
        (old as ResourceDiagnostics).resourceDiagnostics['fileTextUnits'],
        0,
      );
      await tester.pumpWidget(const SizedBox());
      await api.close();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'Computer Use releases hidden frames and keeps its control session',
    (tester) async {
      await desktopSurface(tester);
      final api = ComputerUseHost();
      final key = GlobalKey<ComputerUsePanelState>();
      var visible = true;
      late StateSetter update;
      await tester.pumpWidget(
        frame(
          StatefulBuilder(
            builder: (_, setState) {
              update = setState;
              return TickerMode(
                enabled: visible,
                child: ComputerUsePanel(key: key, api: api, session: 's'),
              );
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      final panel = key.currentState!;
      expect(
        panel.resourceDiagnostics['computerUseFrameBytes'],
        greaterThan(0),
      );
      await panel.action('capture');
      await tester.pumpAndSettle();
      expect(panel.resourceDiagnostics['computerUsePendingEvictions'], 0);
      update(() => visible = false);
      await tester.pump();
      expect(panel.resourceDiagnostics['computerUsePollTimers'], 0);
      final count = api.actions.length;
      await tester.pump(const Duration(minutes: 2));
      await tester.pump();
      expect(api.actions.length, count);
      expect(panel.resourceDiagnostics['computerUseFrameBytes'], 0);
      expect(panel.state, isNotNull);
      expect(api.names, isNot(contains('close')));
      update(() => visible = true);
      await tester.pumpAndSettle();
      expect(api.names.last, 'capture');
      expect(
        panel.resourceDiagnostics['computerUseFrameBytes'],
        greaterThan(0),
      );
      await tester.pumpWidget(const SizedBox());
      expect(panel.resourceDiagnostics['computerUseFrameBytes'], 0);
      expect(panel.resourceDiagnostics['computerUseRetentionTimers'], 0);
      expect(api.names, isNot(contains('close')));
      await api.close();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'tool navigation cycles both directions and returns focus after close',
    (tester) async {
      final api = TabApi(), controller = TabController(api);
      await tester.pumpWidget(TabHarness(controller: controller));
      await tester.pumpAndSettle();
      await openTab(tester, 'git');
      final panel = tester.state<WorkbenchPanelState>(
        find.byType(WorkbenchPanel),
      );
      panel.cycleTab();
      await tester.pumpAndSettle();
      expect(panel.tab, 'files');
      expect(panel.tabFocusNodes['files']!.hasFocus, isTrue);
      panel.cycleTab(reverse: true);
      await tester.pumpAndSettle();
      expect(panel.tab, 'git');
      panel.close('git');
      await tester.pumpAndSettle();
      expect(panel.tabFocusNodes['files']!.hasFocus, isTrue);
      await cleanup(tester, controller, [api]);
    },
  );
}
