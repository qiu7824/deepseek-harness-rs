import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/artifacts_view.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class ArtifactClient extends DshClient {
  ArtifactClient() : super('http://127.0.0.1');
  final calls = <({String path, Json? body, RequestScope? scope})>[];
  Completer<Json>? pending;
  bool conflict = false;
  List<Json>? entriesOverride;
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    calls.add((path: path, body: body, scope: scope));
    if (path == '/__dsh-artifacts/list') {
      return pending?.future ??
          Future.value({
            'entries':
                entriesOverride ??
                [
                  {
                    'path': 'report.md',
                    'change': 'created',
                    'size': 1024,
                    'etag': 'revision-one',
                  },
                  {'path': 'removed.pdf', 'change': 'deleted'},
                ],
          });
    }
    if (path == '/__dsh-artifacts/resources') {
      return {
        'entries': [
          {
            'id': 'resource-a',
            'owner': 'session-a',
            'label': '临时材料',
            'path': 'scratch',
            'bytes': 128,
            'state': 'candidate',
            'busy': true,
            'pinned': false,
          },
        ],
      };
    }
    if (path == '/__dsh-artifacts/file-action' && conflict) {
      throw DshException('conflict', '文件已被修改，请刷新后重试');
    }
    if (path.startsWith('/__dsh-preview/source')) {
      return pending?.future ?? Future.value({'text': '# 文件内容'});
    }
    return {};
  }
}

void main() {
  testWidgets('large artifact lists remain virtualized in a narrow panel', (
    tester,
  ) async {
    final api = ArtifactClient()
      ..entriesOverride = List.generate(
        2000,
        (i) => {
          'path': 'documents/long-file-name-$i.md',
          'change': 'created',
          'size': 1024,
          'etag': 'v$i',
        },
      );
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 360,
              height: 420,
              child: ArtifactsView(api: api, session: 's'),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('2000 个文件'), findsOneWidget);
    expect(
      find.byType(PopupMenuButton<String>).evaluate().length,
      lessThan(30),
    );
    expect(find.text('documents/long-file-name-1999.md'), findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    await api.close();
  });
  testWidgets('rename carries etag and rejected save keeps the path draft', (
    tester,
  ) async {
    final api = ArtifactClient()..conflict = true;
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: ArtifactsView(api: api, session: 'session-a'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('2 个文件'), findsOneWidget);
    await tester.tap(find.byTooltip('管理 report.md'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('重命名'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'renamed.md');
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    final mutation = api.calls.lastWhere(
      (c) => c.path == '/__dsh-artifacts/file-action',
    );
    expect(mutation.body, {
      'sessionId': 'session-a',
      'path': 'report.md',
      'etag': 'revision-one',
      'action': 'rename',
      'newPath': 'renamed.md',
    });
    expect(find.text('renamed.md'), findsOneWidget);
    expect(find.textContaining('文件已被修改'), findsOneWidget);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    await tester.pumpWidget(const SizedBox());
    await api.close();
  });
  testWidgets(
    'artifact polling stops on hidden pages and cancels in-flight reads on disposal',
    (tester) async {
      final api = ArtifactClient();
      Widget page(bool visible) => ShadApp(
        home: Scaffold(
          body: TickerMode(
            enabled: visible,
            child: ArtifactsView(api: api, session: 'session-a'),
          ),
        ),
      );
      await tester.pumpWidget(page(true));
      await tester.pumpAndSettle();
      final initial = api.calls.length;
      await tester.pumpWidget(page(false));
      await tester.pump(const Duration(seconds: 10));
      expect(api.calls.length, initial);
      api.pending = Completer<Json>();
      await tester.pumpWidget(page(true));
      await tester.pump();
      final scope = api.calls.last.scope!;
      await tester.pumpWidget(const SizedBox());
      expect(scope.cancelled, true);
      api.pending!.complete({
        'entries': [
          {'path': 'late-file'},
        ],
      });
      await tester.pumpAndSettle();
      final count = api.calls.length;
      await tester.pump(const Duration(seconds: 10));
      expect(api.calls.length, count);
      expect(tester.takeException(), isNull);
      await api.close();
    },
  );
  testWidgets('managed-resource actions respect busy and candidate state', (
    tester,
  ) async {
    final api = ArtifactClient();
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: ManagedResourcesView(api: api, session: 'session-a'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('正在使用'), findsOneWidget);
    await tester.tap(find.text('固定保留'));
    await tester.pumpAndSettle();
    expect(
      api.calls
          .where((c) => c.path == '/__dsh-artifacts/resource-action')
          .single
          .body,
      {'id': 'resource-a', 'action': 'pin', 'pinned': true},
    );
    await tester.tap(find.text('标记已完成'), warnIfMissed: false);
    await tester.pump();
    expect(api.calls.where((c) => c.body?['action'] == 'release'), isEmpty);
    await tester.pumpWidget(const SizedBox());
    await api.close();
  });
  testWidgets(
    'disposed file viewer cannot repopulate a shared cache from a late response',
    (tester) async {
      final api = ArtifactClient()..pending = Completer<Json>();
      final cache = ResourceCache<String, String>(
        maxBytes: 1024,
        maxEntries: 1,
        sizeOf: (s) => s.length * 2,
      );
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: NativeFileViewer(
              api: api,
              session: 's',
              path: 'file.md',
              cache: cache,
              onPage: (_) {},
            ),
          ),
        ),
      );
      await tester.pump();
      final scope = api.calls.last.scope!;
      await tester.pumpWidget(const SizedBox());
      cache.clear();
      api.pending!.complete({'text': 'late data'});
      await tester.pumpAndSettle();
      expect(scope.cancelled, true);
      expect(cache.length, 0);
      expect(tester.takeException(), isNull);
      await api.close();
    },
  );
}
