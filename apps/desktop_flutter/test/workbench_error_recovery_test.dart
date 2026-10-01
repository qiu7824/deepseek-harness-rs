import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/error.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/l10n/zh.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:xterm/xterm.dart';

import 'controller_test.dart' show MemoryPreferences;

typedef RecoveryRequest = ({
  Uri uri,
  Json? body,
  RequestScope? scope,
  bool mutation,
});

class RecoveryApi extends DshClient {
  RecoveryApi() : super('http://127.0.0.1:1');
  final calls = <RecoveryRequest>[];
  late Future<Json> Function(RecoveryRequest) handle;

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) {
    final call = (
      uri: Uri.parse(path),
      body: body,
      scope: scope,
      mutation: mutation,
    );
    calls.add(call);
    // Intentionally ignore cancellation here to exercise the UI's late-result guard.
    return handle(call);
  }
}

class RecoveryController extends DesktopController {
  RecoveryController(this.api) : super(MemoryPreferences()) {
    selectedId = 'owner';
  }
  final RecoveryApi api;
  @override
  DshClient get client => api;
  @override
  bool get connected => true;
}

ResourceCache<String, String> documentCache() => ResourceCache(
  maxBytes: 1024 * 1024,
  maxEntries: 4,
  sizeOf: (value) => value.length * 2,
);

Future<void> showPanel(WidgetTester tester, Widget child) async {
  await tester.pumpWidget(ShadApp(home: Scaffold(body: child)));
  await tester.pump();
  await tester.pump();
}

Future<void> settleAndClose(
  WidgetTester tester,
  Iterable<RecoveryApi> apis,
) async {
  expect(tester.takeException(), isNull);
  await tester.pumpWidget(const SizedBox());
  await tester.pump();
  for (final api in apis) {
    await api.close();
  }
}

void main() {
  testWidgets(
    'file retry clears the failure, cancels its read scope and displays the recovered source',
    (tester) async {
      final api = RecoveryApi();
      api.handle = (_) async {
        if (api.calls.length == 1) {
          throw DshException('http-503', '读取失败 api_key=private-file-key');
        }
        return {'text': 'recovered source'};
      };
      await showPanel(
        tester,
        NativeFileViewer(
          api: api,
          session: 'owner',
          path: 'notes.txt',
          cache: documentCache(),
          onPage: (_) {},
        ),
      );
      expect(find.byType(DshErrorView), findsOneWidget);
      expect(find.textContaining('private-file-key'), findsNothing);
      expect(find.byType(SelectableText), findsNothing);
      await tester.tap(find.text(DshZh.retry));
      await tester.pumpAndSettle();
      expect(api.calls, hasLength(2));
      expect(api.calls.first.scope!.cancelled, isTrue);
      expect(api.calls.last.uri.queryParameters['sessionId'], 'owner');
      expect(api.calls.last.uri.queryParameters['path'], 'notes.txt');
      expect(find.text('recovered source'), findsOneWidget);
      expect(find.byType(DshErrorView), findsNothing);
      await settleAndClose(tester, [api]);
    },
  );

  testWidgets(
    'reused file viewer ignores the old Host reply and cannot reuse its cache',
    (tester) async {
      final old = RecoveryApi(), fresh = RecoveryApi();
      final late = Completer<Json>();
      old.handle = (_) => late.future;
      fresh.handle = (_) async => {'text': 'new Host source'};
      final cache = documentCache();
      Widget panel(RecoveryApi api, String session) => NativeFileViewer(
        api: api,
        session: session,
        path: 'same.txt',
        cache: cache,
        onPage: (_) {},
      );
      await showPanel(tester, panel(old, 'old-session'));
      await showPanel(tester, panel(fresh, 'new-session'));
      expect(old.calls.single.scope!.cancelled, isTrue);
      late.complete({'text': 'stale Host source'});
      await tester.pumpAndSettle();
      expect(find.text('new Host source'), findsOneWidget);
      expect(find.text('stale Host source'), findsNothing);
      expect(cache.get('same.txt'), 'new Host source');
      expect(
        fresh.calls.single.uri.queryParameters['sessionId'],
        'new-session',
      );
      await settleAndClose(tester, [old, fresh]);
    },
  );

  testWidgets(
    'directory retry repeats the failed destination rather than the previous directory',
    (tester) async {
      final api = RecoveryApi();
      var attempts = 0;
      api.handle = (call) async {
        final path = call.uri.queryParameters['path'];
        if (path == '') {
          return {
            'path': '',
            'entries': [
              {'name': 'A', 'path': 'A', 'kind': 'directory'},
            ],
          };
        }
        if (++attempts == 1) {
          throw DshException(
            'not-found',
            'directory is temporarily unavailable',
          );
        }
        return {
          'path': 'A',
          'entries': [
            {
              'name': 'recovered.txt',
              'path': 'A/recovered.txt',
              'kind': 'file',
            },
          ],
        };
      };
      await showPanel(tester, FilePanel(api: api, session: 'owner', cwd: ''));
      await tester.tap(find.text('A'));
      await tester.pumpAndSettle();
      expect(find.byType(DshErrorView), findsOneWidget);
      await tester.tap(find.text(DshZh.retry));
      await tester.pumpAndSettle();
      expect(api.calls.map((c) => c.uri.queryParameters['path']), [
        '',
        'A',
        'A',
      ]);
      expect(api.calls[1].scope!.cancelled, isTrue);
      expect(find.text('recovered.txt'), findsOneWidget);
      expect(find.byType(DshErrorView), findsNothing);
      await settleAndClose(tester, [api]);
    },
  );

  testWidgets(
    'Git retry retains the requested staged path and ignores late replies from an old Host',
    (tester) async {
      final api = RecoveryApi(), fresh = RecoveryApi();
      var reads = 0;
      final late = Completer<Json>();
      api.handle = (call) async {
        if (call.uri.path.endsWith('git-status')) {
          return {
            'branch': 'old',
            'entries': [
              {'path': 'A.txt', 'group': 'staged', 'status': 'M'},
            ],
          };
        }
        if (++reads == 1) throw DshException('http-503', 'diff unavailable');
        return late.future;
      };
      fresh.handle = (_) async => {
        'branch': 'new Host branch',
        'entries': <Json>[],
      };
      await showPanel(tester, GitPanel(api: api, session: 'old-session'));
      await tester.tap(find.text('A.txt'));
      await tester.pumpAndSettle();
      await tester.tap(find.text(DshZh.retry));
      await tester.pump();
      final diffReads = api.calls
          .where((c) => c.uri.path.endsWith('git-diff'))
          .toList();
      expect(diffReads, hasLength(2));
      expect(diffReads.first.scope!.cancelled, isTrue);
      expect(diffReads.last.uri.queryParameters['path'], 'A.txt');
      expect(diffReads.last.uri.queryParameters['staged'], '1');
      await showPanel(tester, GitPanel(api: fresh, session: 'new-session'));
      expect(diffReads.last.scope!.cancelled, isTrue);
      late.complete({'diff': '+ stale change'});
      await tester.pumpAndSettle();
      expect(find.text('new Host branch'), findsOneWidget);
      expect(find.text('+ stale change'), findsNothing);
      expect(find.byType(DshErrorView), findsNothing);
      await settleAndClose(tester, [api, fresh]);
    },
  );

  testWidgets(
    'terminal retries reads but does not retry an uncertain input mutation',
    (tester) async {
      final api = RecoveryApi();
      var reads = 0, inputs = 0;
      api.handle = (call) async {
        if (call.uri.path.endsWith('terminal-list')) {
          return {
            'entries': [
              {'id': 'a', 'name': 'Shell'},
            ],
          };
        }
        if (call.uri.path.endsWith('terminal-read')) {
          if (++reads == 1) throw DshException('http-503', 'read failed');
          return {'totalLines': 1, 'text': 'ready> '};
        }
        if (call.body?['action'] == 'input') {
          inputs++;
          throw DshException(
            'timeout',
            'input acknowledgement missing',
            outcomeUnknown: true,
            details: {'token': 'private-terminal-token'},
          );
        }
        return {'ok': true};
      };
      await showPanel(tester, NativeTerminalPanel(api: api, session: 'owner'));
      await tester.pumpAndSettle();
      expect(find.text(DshZh.retry), findsOneWidget);
      final firstRead = api.calls.firstWhere(
        (c) => c.uri.path.endsWith('terminal-read'),
      );
      await tester.tap(find.text(DshZh.retry));
      await tester.pumpAndSettle();
      expect(firstRead.scope!.cancelled, isTrue);
      expect(reads, greaterThanOrEqualTo(2));
      expect(find.byType(DshErrorView), findsNothing);
      tester.widget<TerminalView>(find.byType(TerminalView)).terminal.onOutput!(
        'echo test\r',
      );
      await tester.pump(const Duration(milliseconds: 30));
      await tester.pump(const Duration(milliseconds: 600));
      expect(inputs, 1);
      expect(find.text(DshZh.outcomeUnknown), findsOneWidget);
      expect(find.text(DshZh.retry), findsNothing);
      expect(find.textContaining('private-terminal-token'), findsNothing);
      await settleAndClose(tester, [api]);
    },
  );

  testWidgets(
    'background job retry clears the old error and resumes with a new read scope',
    (tester) async {
      final api = RecoveryApi();
      api.handle = (_) async {
        if (api.calls.length == 1) {
          throw DshException('http-503', 'jobs unavailable');
        }
        return {
          'entries': [
            {
              'id': 'job',
              'title': 'recovered background job',
              'status': 'running',
            },
          ],
        };
      };
      final c = RecoveryController(api);
      await showPanel(tester, TaskPanel(controller: c));
      expect(find.text(DshZh.retry), findsOneWidget);
      await tester.tap(find.text(DshZh.retry));
      await tester.pumpAndSettle();
      expect(api.calls.first.scope!.cancelled, isTrue);
      expect(api.calls, hasLength(2));
      expect(find.byType(DshErrorView), findsNothing);
      expect(find.text('recovered background job'), findsOneWidget);
      await settleAndClose(tester, [api]);
      c.dispose();
    },
  );

  testWidgets(
    'transient errors offer explicit redacted details without an automatic mutation retry',
    (tester) async {
      await showPanel(
        tester,
        Builder(
          builder: (context) => TextButton(
            onPressed: () => showDshError(
              context,
              DshException(
                'timeout',
                'Authorization: Bearer private-key',
                outcomeUnknown: true,
              ),
            ),
            child: const Text('fail'),
          ),
        ),
      );
      await tester.tap(find.text('fail'));
      await tester.pumpAndSettle();
      expect(find.text(DshZh.viewDetails), findsOneWidget);
      expect(find.textContaining('private-key'), findsNothing);
      await tester.tap(find.text(DshZh.viewDetails));
      await tester.pumpAndSettle();
      expect(find.byType(DshErrorView), findsOneWidget);
      expect(find.byType(SelectableText), findsNothing);
      await tester.tap(find.text(DshZh.details));
      await tester.pumpAndSettle();
      final details = tester
          .widget<SelectableText>(find.byType(SelectableText))
          .data!;
      expect(details, contains('timeout'));
      expect(details, isNot(contains('private-key')));
      expect(find.text(DshZh.retry), findsNothing);
      await settleAndClose(tester, []);
    },
  );
}
