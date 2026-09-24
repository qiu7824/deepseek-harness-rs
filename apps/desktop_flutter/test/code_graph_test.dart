import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/code_graph_controller.dart';
import 'package:dsh_desktop/features/conversation/code_graph_view.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

Json graphData([String suffix = '']) => {
  'status': 'ready',
  'files': 3,
  'totalSymbols': 2,
  'totalCalls': 1,
  'symbols': [
    {
      'id': 'run$suffix',
      'name': 'run$suffix',
      'path': 'main.py',
      'line': 10,
      'kind': 'function',
    },
    {
      'id': 'work$suffix',
      'name': 'work$suffix',
      'path': 'util.py',
      'line': 20,
      'kind': 'function',
    },
  ],
  'calls': [
    {
      'source': 'run$suffix',
      'target': 'work$suffix',
      'resolution': 'name-match',
      'path': 'main.py',
      'line': 11,
      'name': 'work',
    },
  ],
  'deps': [
    {
      'source': 'main.py',
      'target': 'util.py',
      'path': 'main.py',
      'line': 2,
      'kind': 'import',
      'name': 'util.py',
    },
    {
      'source': 'main.py',
      'target': 'io.py',
      'path': 'main.py',
      'line': 3,
      'kind': 'import',
      'name': 'io.py',
    },
  ],
};

class GraphApi extends DshClient {
  GraphApi() : super('http://127.0.0.1');
  final calls = <({Uri uri, Json? body, RequestScope? scope})>[];
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
    calls.add((uri: uri, body: body, scope: scope));
    if (uri.path.endsWith('code-graph-cancel')) return {};
    if (uri.path.endsWith('/source')) {
      return {'text': List.generate(150, (i) => 'line ${i + 1}').join('\n')};
    }
    final query = uri.queryParameters['q'] ?? '';
    return pending[query]?.future ?? Future.value(graphData(query));
  }
}

void main() {
  test('late graph response cannot overwrite a newer search and disposal cancels reads', () async {
    final api = GraphApi();
    api.pending[''] = Completer<Json>();
    api.pending['new'] = Completer<Json>();
    final c = CodeGraphController(api, 's');
    final first = c.refresh();
    await Future<void>.delayed(Duration.zero);
    final oldScope = api.calls.last.scope!;
    c.query = 'new';
    final newer = c.refresh();
    await Future<void>.delayed(Duration.zero);
    expect(oldScope.cancelled, true);
    api.pending['new']!.complete(graphData('new'));
    await newer;
    api.pending['']!.complete(graphData('old'));
    await first;
    expect(c.graph['symbols'][0]['id'], 'runnew');
    api.pending['last'] = Completer<Json>();
    c.query = 'last';
    final last = c.refresh();
    await Future<void>.delayed(Duration.zero);
    final finalScope = api.calls.last.scope!;
    c.dispose();
    expect(finalScope.cancelled, true);
    api.pending['last']!.complete(graphData('late'));
    await last;
    expect(c.graph, isEmpty);
    await api.close();
  });
  test(
    'query modes and explicit index pause use the correct host contract',
    () async {
      final api = GraphApi(), c = CodeGraphController(GraphApi(), 'unused');
      c.dispose();
      final graph = CodeGraphController(api, 'session-a');
      graph.mode(false);
      await Future<void>.delayed(Duration.zero);
      final node = graph.model.catalog.firstWhere((n) => n.id == 'run');
      graph.focusNode(node, relation: 'callees');
      await Future<void>.delayed(Duration.zero);
      expect(api.calls.last.uri.queryParameters['mode'], 'callees');
      expect(api.calls.last.uri.queryParameters['selected'], 'run');
      await graph.refresh(resume: true);
      expect(api.calls.last.uri.queryParameters['resume'], '1');
      await graph.pause();
      expect(
        api.calls.any(
          (r) =>
              r.uri.path.endsWith('code-graph-cancel') &&
              r.body?['sessionId'] == 'session-a',
        ),
        true,
      );
      final count = api.calls.length;
      graph.setActive(false);
      await graph.refresh();
      expect(api.calls.length, count);
      graph.dispose();
      await api.close();
    },
  );
  testWidgets(
    'canvas supports selection, zoom, symbol focus and source line opening',
    (tester) async {
      final api = GraphApi();
      await tester.binding.setSurfaceSize(const Size(1280, 900));
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: CodeGraphView(api: api, session: 's'),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('3 节点 · 2 关联'), findsOneWidget);
      await tester.tap(find.byTooltip('放大画布'));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      await tester.tap(find.text('符号调用'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('graph-node-run')));
      await tester.pump(const Duration(milliseconds: 350));
      await tester.pumpAndSettle();
      expect(find.text('节点详情'), findsOneWidget);
      expect(find.textContaining('10  line 10'), findsOneWidget);
      await tester.tap(find.text('被调用者'));
      await tester.pumpAndSettle();
      expect(
        api.calls.any((r) => r.uri.queryParameters['mode'] == 'callees'),
        true,
      );
      await tester.tap(find.text('打开源码'));
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<NativeFileViewer>(find.byType(NativeFileViewer))
            .initialLine,
        10,
      );
      expect(find.text('line 10'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      await api.close();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets('narrow graph pane has bounded cards and no overflow', (
    tester,
  ) async {
    final api = GraphApi();
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 360,
              height: 520,
              child: CodeGraphView(api: api, session: 's'),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('code-graph-stage')), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    await api.close();
  });
}
