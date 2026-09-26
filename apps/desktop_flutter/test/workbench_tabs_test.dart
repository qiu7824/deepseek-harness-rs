import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/src/resource_diagnostics.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:xterm/xterm.dart';

class TabPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class TabApi extends DshClient {
  TabApi() : super('http://127.0.0.1:1');
  final requests =
      <({String operation, String? session, RequestScope? scope})>[];
  final childReads = <RequestScope?>[];
  final listReads = <RequestScope?>[];
  Completer<Json>? pendingJobs;

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    final uri = Uri.parse(path),
        operation = Uri.parse(path).path.split('/').last;
    requests.add((
      operation: operation,
      session: uri.queryParameters['sessionId'],
      scope: scope,
    ));
    return switch (operation) {
      'list' => {
        'path': '',
        'entries': [
          {'name': 'notes.txt', 'path': 'notes.txt', 'kind': 'file'},
        ],
      },
      'source' => {'text': 'First line\nSecond line'},
      'git-status' => {'branch': 'main', 'entries': <Json>[]},
      'terminal-list' => {
        'entries': [
          {'id': 'terminal', 'name': 'Shell'},
        ],
      },
      'terminal-read' => {'totalLines': 1, 'text': 'prompt> '},
      'job-list' =>
        pendingJobs == null ? {'entries': <Json>[]} : await pendingJobs!.future,
      _ => throw StateError('Unexpected request: $path'),
    };
  }

  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    if (method == 'subagent.list') {
      listReads.add(scope);
      return {
        'entries': [
          {'id': 'child', 'label': 'Child task', 'mode': 'continuable'},
        ],
      };
    }
    if (method == 'subagent.history') {
      childReads.add(scope);
      return {'events': <Json>[], 'hasMore': false};
    }
    throw StateError('Unexpected RPC: $method');
  }
}

class TabController extends DesktopController {
  TabController(this.api) : super(TabPreferences()) {
    selectedId = 'a';
    sessions = [
      for (final id in ['a', 'b'])
        SessionSummary.fromJson({
          'sessionId': id,
          'cwd': 'E:/fixture',
          'projections': {
            'values': {'title': 'Task $id'},
          },
        }),
    ];
    transcript = [TranscriptItem(id: 'one', kind: 'assistant', text: 'Ready')];
  }
  TabApi api;
  @override
  DshClient get client => api;
  @override
  bool get connected => true;
}

class DelayedPanelApi extends TabApi {
  final pending = <String, Completer<Json>>{};
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    final uri = Uri.parse(path);
    final operation = uri.path.split('/').last;
    if (operation == 'list' && (uri.queryParameters['path'] ?? '').isEmpty) {
      return {
        'path': '',
        'entries': [
          for (final name in ['A', 'B'])
            {'path': name, 'name': name, 'kind': 'directory'},
        ],
      };
    }
    if (operation == 'git-status') {
      return {
        'branch': 'main',
        'entries': [
          for (final name in ['A.txt', 'B.txt'])
            {'path': name, 'group': 'unstaged', 'status': 'M'},
        ],
      };
    }
    return pending
        .putIfAbsent(uri.queryParameters['path']!, Completer<Json>.new)
        .future;
  }
}

class TabHarness extends StatefulWidget {
  const TabHarness({super.key, required this.controller});
  final TabController controller;
  @override
  State<TabHarness> createState() => TabHarnessState();
}

class TabHarnessState extends State<TabHarness> {
  String initial = 'files';
  int request = 0;
  FileOpenRequest? file;
  void open(String type, {FileOpenRequest? fileRequest}) => setState(() {
    initial = type;
    request++;
    file = fileRequest;
  });
  @override
  Widget build(BuildContext context) => ShadApp(
    home: Scaffold(
      body: WorkbenchPanel(
        key: ValueKey((widget.controller.client, widget.controller.selectedId)),
        controller: widget.controller,
        initialTab: initial,
        openRequest: request,
        fileRequest: file,
        onFileRequestHandled: (value) {
          if (identical(value, file)) file = null;
        },
        onClose: () {},
      ),
    ),
  );
}

Future<void> openTab(WidgetTester tester, String type) async {
  await tester.tap(find.byKey(const Key('workbench-add-tab')));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 300));
  await tester.tap(find.byKey(ValueKey('workbench-open-$type')));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 300));
  await tester.pump();
}

Future<void> selectTab(WidgetTester tester, String type) async {
  await tester.ensureVisible(find.byKey(ValueKey('workbench-select-$type')));
  await tester.tap(find.byKey(ValueKey('workbench-select-$type')));
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 300));
  await tester.pump();
}

Future<void> cleanup(
  WidgetTester tester,
  TabController c,
  Iterable<TabApi> apis,
) async {
  await tester.pumpWidget(const SizedBox());
  c.dispose();
  for (final api in apis) {
    await api.close();
  }
  await tester.binding.setSurfaceSize(null);
  expect(tester.takeException(), isNull);
}

void main() {
  for (final oldFails in [false, true]) {
    testWidgets(
      'directory navigation ignores late older ${oldFails ? 'errors' : 'results'}',
      (tester) async {
        final api = DelayedPanelApi();
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: FilePanel(api: api, session: 's', cwd: 'E:/fixture'),
            ),
          ),
        );
        await tester.pumpAndSettle();
        await tester.tap(find.text('A'));
        await tester.pump();
        await tester.tap(find.text('B'));
        await tester.pump();
        api.pending['B']!.complete({
          'path': 'B',
          'entries': [
            {'path': 'B/new.txt', 'name': 'new.txt', 'kind': 'file'},
          ],
        });
        await tester.pump();
        expect(find.byType(LinearProgressIndicator), findsNothing);
        if (oldFails) {
          api.pending['A']!.completeError(
            StateError('stale directory failure'),
          );
        } else {
          api.pending['A']!.complete({
            'path': 'A',
            'entries': [
              {'path': 'A/old.txt', 'name': 'old.txt', 'kind': 'file'},
            ],
          });
        }
        await tester.pump();
        expect(find.text('new.txt'), findsOneWidget);
        expect(find.text('old.txt'), findsNothing);
        expect(find.textContaining('stale directory failure'), findsNothing);
        await tester.pumpWidget(const SizedBox());
        await api.close();
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets(
      'Git selection ignores late older ${oldFails ? 'errors' : 'diffs'}',
      (tester) async {
        final api = DelayedPanelApi();
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: GitPanel(api: api, session: 's'),
            ),
          ),
        );
        await tester.pumpAndSettle();
        await tester.tap(find.text('A.txt'));
        await tester.pump();
        await tester.tap(find.text('B.txt'));
        await tester.pump();
        api.pending['B.txt']!.complete({'diff': '+ New B diff'});
        await tester.pump();
        if (oldFails) {
          api.pending['A.txt']!.completeError(StateError('stale diff failure'));
        } else {
          api.pending['A.txt']!.complete({'diff': '+ Old A diff'});
        }
        await tester.pump();
        expect(find.text('+ New B diff'), findsOneWidget);
        expect(find.text('+ Old A diff'), findsNothing);
        expect(find.textContaining('stale diff failure'), findsNothing);
        await tester.pumpWidget(const SizedBox());
        await api.close();
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets(
    'tool tabs retain file state, close individually and reopen without replaying a file request',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(800, 700));
      final api = TabApi(), key = GlobalKey<TabHarnessState>();
      final c = TabController(api);
      await tester.pumpWidget(TabHarness(key: key, controller: c));
      await tester.pumpAndSettle();
      key.currentState!.open(
        'files',
        fileRequest: const FileOpenRequest('notes.txt'),
      );
      await tester.pumpAndSettle();
      final oldFile = tester.state(find.byType(FilePanel));
      final oldViewer = tester.state(find.byType(NativeFileViewer));
      await openTab(tester, 'git');
      expect(oldFile.mounted, isTrue);
      expect(oldViewer.mounted, isTrue);
      expect(find.byKey(const ValueKey('workbench-tab-files')), findsOneWidget);
      expect(find.byKey(const ValueKey('workbench-tab-git')), findsOneWidget);
      key.currentState!.open('files');
      await tester.pumpAndSettle();
      expect(tester.state(find.byType(NativeFileViewer)), same(oldViewer));
      await selectTab(tester, 'git');
      key.currentState!.open('files');
      await tester.pumpAndSettle();
      expect(tester.state(find.byType(NativeFileViewer)), same(oldViewer));
      await tester.tap(find.byKey(const ValueKey('workbench-close-files')));
      await tester.pumpAndSettle();
      expect(oldFile.mounted, isFalse);
      expect(
        (oldFile as ResourceDiagnostics)
            .resourceDiagnostics['documentCacheBytes'],
        0,
      );
      await openTab(tester, 'files');
      expect(find.byType(NativeFileViewer), findsNothing);
      expect(find.text('选择文件以查看内容'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('workbench-close-files')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('workbench-close-git')));
      await tester.pumpAndSettle();
      expect(find.text('使用 + 打开文件、终端或其他工具标签。'), findsOneWidget);
      await openTab(tester, 'files');
      expect(find.byType(FilePanel), findsOneWidget);
      await cleanup(tester, c, [api]);
    },
  );

  testWidgets(
    'hidden terminal stops reads and retains its buffer until its tab closes',
    (tester) async {
      final api = TabApi();
      final c = TabController(api);
      await tester.pumpWidget(TabHarness(controller: c));
      await tester.pumpAndSettle();
      await openTab(tester, 'terminal');
      final terminal = tester
          .widget<TerminalView>(find.byType(TerminalView))
          .terminal;
      final scope = api.requests
          .lastWhere((r) => r.operation == 'terminal-read')
          .scope!;
      await selectTab(tester, 'files');
      expect(scope.cancelled, isTrue);
      final count = api.requests
          .where((r) => r.operation == 'terminal-read')
          .length;
      await tester.pump(const Duration(seconds: 5));
      expect(
        api.requests.where((r) => r.operation == 'terminal-read').length,
        count,
      );
      await selectTab(tester, 'terminal');
      expect(
        tester.widget<TerminalView>(find.byType(TerminalView)).terminal,
        same(terminal),
      );
      expect(
        api.requests.where((r) => r.operation == 'terminal-read').length,
        greaterThan(count),
      );
      await tester.tap(find.byKey(const ValueKey('workbench-close-terminal')));
      await tester.pumpAndSettle();
      expect(terminal.onOutput, isNull);
      expect(terminal.onResize, isNull);
      await cleanup(tester, c, [api]);
    },
  );

  testWidgets(
    'hidden child conversation preserves its draft and pauses all child reads',
    (tester) async {
      final api = TabApi(), key = GlobalKey<TabHarnessState>();
      final c = TabController(api);
      await tester.pumpWidget(TabHarness(key: key, controller: c));
      await tester.pumpAndSettle();
      await openTab(tester, 'team');
      await tester.tap(find.text('Child task'));
      await tester.pumpAndSettle();
      final input = find.byWidgetPredicate(
        (w) => w is TextField && w.decoration?.hintText == '继续向子任务发送消息…',
      );
      await tester.enterText(input, 'Unsent child draft');
      final oldRead = api.childReads.last!;
      await selectTab(tester, 'files');
      expect(oldRead.cancelled, isTrue);
      final count = api.childReads.length, listCount = api.listReads.length;
      await tester.pump(const Duration(seconds: 9));
      expect(api.childReads.length, count);
      expect(api.listReads.length, listCount);
      await selectTab(tester, 'team');
      expect(find.text('Unsent child draft'), findsOneWidget);
      expect(api.childReads.length, greaterThan(count));
      await cleanup(tester, c, [api]);
    },
  );

  testWidgets(
    'hidden background jobs cancel in-flight reads and ignore their late response',
    (tester) async {
      final api = TabApi()..pendingJobs = Completer<Json>();
      final c = TabController(api);
      await tester.pumpWidget(TabHarness(controller: c));
      await tester.pumpAndSettle();
      await openTab(tester, 'tasks');
      final scope = api.requests
          .lastWhere((r) => r.operation == 'job-list')
          .scope!;
      await selectTab(tester, 'files');
      expect(scope.cancelled, isTrue);
      final count = api.requests.where((r) => r.operation == 'job-list').length;
      api.pendingJobs!.complete({
        'entries': [
          {'title': 'Stale result'},
        ],
      });
      api.pendingJobs = null;
      await tester.pump(const Duration(seconds: 8));
      expect(
        api.requests.where((r) => r.operation == 'job-list').length,
        count,
      );
      await selectTab(tester, 'tasks');
      expect(find.text('Stale result'), findsNothing);
      expect(find.text('当前没有后台任务'), findsOneWidget);
      await cleanup(tester, c, [api]);
    },
  );

  for (final width in [1400.0, 800.0]) {
    testWidgets(
      'shell isolates tool tabs by session and Host at width $width',
      (tester) async {
        await tester.binding.setSurfaceSize(Size(width, 900));
        final first = TabApi(), second = TabApi();
        final c = TabController(first);
        await tester.pumpWidget(DesktopApp(controller: c));
        await tester.pumpAndSettle();
        await tester.tap(find.byTooltip('显示工作台'));
        await tester.pumpAndSettle();
        await openTab(tester, 'git');
        final old = tester.state(find.byType(GitPanel));
        c.selectedId = 'b';
        c.emit();
        await tester.pumpAndSettle();
        expect(old.mounted, isFalse);
        expect(find.byKey(const ValueKey('workbench-tab-git')), findsNothing);
        expect(tester.widget<FilePanel>(find.byType(FilePanel)).session, 'b');
        await openTab(tester, 'git');
        final oldHost = tester.state(find.byType(GitPanel));
        c.api = second;
        c.emit();
        await tester.pumpAndSettle();
        expect(oldHost.mounted, isFalse);
        expect(
          tester.widget<FilePanel>(find.byType(FilePanel)).api,
          same(second),
        );
        await cleanup(tester, c, [first, second]);
      },
    );
  }
}
