import 'dart:async';
import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/src/resource_diagnostics.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:xterm/xterm.dart';

class TerminalApi extends DshClient {
  TerminalApi() : super('http://127.0.0.1:1');
  final reads = <({String id, RequestScope? scope})>[];
  final actions = <Json>[];
  final inputScopes = <RequestScope?>[];
  Completer<Json>? slowRead, slowClose, slowInput;
  bool closedA = false;
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    final uri = Uri.parse(path);
    if (uri.path.endsWith('terminal-list')) {
      return {
        'entries': [
          if (!closedA) {'id': 'a', 'name': 'Terminal A'},
          {'id': 'b', 'name': 'Terminal B'},
        ],
      };
    }
    if (uri.path.endsWith('terminal-read')) {
      final id = uri.queryParameters['terminalId']!;
      reads.add((id: id, scope: scope));
      if (id == 'a' && slowRead != null) return slowRead!.future;
      return {'totalLines': 1, 'text': '$id> '};
    }
    actions.add(Map<String, dynamic>.from(body!));
    if (body['action'] == 'input') inputScopes.add(scope);
    if (body['action'] == 'input' && slowInput != null) {
      return slowInput!.future;
    }
    if (body['action'] == 'close' && slowClose != null) {
      final result = await slowClose!.future;
      closedA = true;
      return result;
    }
    return {'ok': true};
  }
}

Future<void> showTerminal(
  WidgetTester tester,
  TerminalApi api, {
  ValueChanged<String>? onInputError,
}) async {
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: NativeTerminalPanel(
          api: api,
          session: 'owner',
          onInputError: onInputError,
        ),
      ),
    ),
  );
  await tester.pump();
  await tester.pump();
}

Future<void> cleanup(WidgetTester tester, TerminalApi api) async {
  await tester.pumpWidget(const SizedBox());
  await tester.pump();
  await api.close();
}

void main() {
  testWidgets(
    'reopened terminal waits for the old view to finish every paste chunk',
    (tester) async {
      final api = TerminalApi()..slowInput = Completer<Json>();
      await showTerminal(tester, api);
      final old = tester
          .widget<TerminalView>(find.byType(TerminalView))
          .terminal;
      final pasted = '中文🙂' * 3000;
      old.onOutput!(pasted);
      await tester.pump(const Duration(milliseconds: 20));
      old.onOutput!('tail');
      final pending = api.slowInput!;
      await tester.pumpWidget(const SizedBox());
      await showTerminal(tester, api);
      final reopened = tester
          .widget<TerminalView>(find.byType(TerminalView))
          .terminal;
      reopened.onOutput!('\r');
      await tester.pump(const Duration(milliseconds: 20));
      expect(api.actions.where((a) => a['action'] == 'input'), hasLength(1));
      api.slowInput = null;
      pending.complete({'ok': true});
      await tester.pump();
      await tester.pump();
      final sent = api.actions.where((a) => a['action'] == 'input').toList();
      expect(sent.length, greaterThan(3));
      expect(
        sent.map((a) => a['text'] as String).join(),
        '$pasted'
        'tail\r',
      );
      expect(sent.last['text'], '\r');
      expect(
        sent.every((a) => a['terminalId'] == 'a' && a['sessionId'] == 'owner'),
        isTrue,
      );
      final resources =
          tester.state(find.byType(NativeTerminalPanel)) as ResourceDiagnostics;
      expect(resources.resourceDiagnostics['terminalInputOwners'], 0);
      await cleanup(tester, api);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'failed input after closing reports the error and cancels remaining writes',
    (tester) async {
      final api = TerminalApi()..slowInput = Completer<Json>();
      final errors = <String>[];
      await showTerminal(tester, api, onInputError: errors.add);
      final terminal = tester
          .widget<TerminalView>(find.byType(TerminalView))
          .terminal;
      terminal.onOutput!('first');
      await tester.pump(const Duration(milliseconds: 20));
      terminal.onOutput!('remaining');
      await tester.pumpWidget(const SizedBox());
      api.slowInput!.completeError(StateError('terminal disconnected'));
      await tester.pump();
      await tester.pump();
      expect(errors, hasLength(1));
      expect(errors.single, contains('terminal disconnected'));
      expect(api.actions.where((a) => a['action'] == 'input'), hasLength(1));
      expect(api.inputScopes.single!.cancelled, isTrue);
      await api.close();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'closing the view drains accepted input to its original terminal in order',
    (tester) async {
      final oldApi = TerminalApi()..slowInput = Completer<Json>();
      await showTerminal(tester, oldApi);
      final terminal = tester
          .widget<TerminalView>(find.byType(TerminalView))
          .terminal;
      terminal.onOutput!('first');
      await tester.pump(const Duration(milliseconds: 20));
      terminal.onOutput!('tail\r');
      final pending = oldApi.slowInput!;
      final inputScope = oldApi.inputScopes.single!;
      await tester.pumpWidget(const SizedBox());
      expect(inputScope.cancelled, isFalse);
      expect(terminal.onOutput, isNull);
      final newApi = TerminalApi();
      await showTerminal(tester, newApi);
      oldApi.slowInput = null;
      pending.complete({'ok': true});
      await tester.pump();
      await tester.pump();
      final sent = oldApi.actions.where((a) => a['action'] == 'input').toList();
      expect(sent.map((a) => a['text']), ['first', 'tail\r']);
      expect(
        sent.every((a) => a['terminalId'] == 'a' && a['sessionId'] == 'owner'),
        isTrue,
      );
      expect(newApi.actions.where((a) => a['action'] == 'input'), isEmpty);
      expect(inputScope.cancelled, isTrue);
      await cleanup(tester, newApi);
      await oldApi.close();
      expect(tester.takeException(), isNull);
    },
  );

  test(
    'output pages append partial lines once and keep new lines at column zero',
    () {
      final cursor = TerminalOutputWindow(),
          terminal = Terminal(maxLines: 3000);
      terminal.resize(80, 24);
      terminal.write(cursor.apply({'totalLines': 2, 'text': 'zero\npar'})!);
      terminal.write(
        cursor.apply({'totalLines': 4, 'text': 'zero\npartial\nnext\nlast'})!,
      );
      expect(
        cursor.apply({'totalLines': 4, 'text': 'zero\npartial\nnext\nlast'}),
        '',
      );
      expect(
        terminal.buffer.getText(),
        startsWith('zero\npartial\nnext\nlast'),
      );
    },
  );

  test('retention rollover, changed tail and missing overlap require a bounded reset', () {
    final cursor = TerminalOutputWindow();
    cursor.apply({'totalLines': 5000, 'text': 'old\nold-tail'});
    expect(
      cursor.apply({'totalLines': 5000, 'text': 'different\nnew-tail'}),
      isNull,
    );
    expect(
      cursor.apply({
        'totalLines': 5000,
        'text': 'different\nold-tail-extension',
      }),
      isNull,
    );
    expect(cursor.apply({'totalLines': 5002, 'text': 'one\ntwo'}), isNull);
    expect(cursor.apply({'totalLines': 10, 'text': 'new'}), isNull);
    expect(
      cursor.apply({
        'totalLines': 5003,
        'text': 'retained\nnew-tail',
      }, reset: true),
      '\x1bcretained\r\nnew-tail',
    );
    expect(
      cursor.apply({'totalLines': 5004, 'text': 'new-tail\nnext'}),
      '\r\nnext',
    );
  });
  testWidgets(
    'large multilingual paste is split by UTF-8 byte limit in order',
    (tester) async {
      final api = TerminalApi();
      await showTerminal(tester, api);
      final text = '${'中文🙂' * 3000}\r';
      tester.widget<TerminalView>(find.byType(TerminalView)).terminal.onOutput!(
        text,
      );
      await tester.pump(const Duration(milliseconds: 20));
      final chunks = api.actions
          .where((a) => a['action'] == 'input')
          .map((a) => a['text'] as String)
          .toList();
      expect(chunks.length, greaterThan(1));
      expect(chunks.join(), text);
      expect(chunks.every((c) => utf8.encode(c).length <= 8192), isTrue);
      await cleanup(tester, api);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'input writes stay ordered across a switch and reject queue overflow',
    (tester) async {
      final api = TerminalApi()..slowInput = Completer<Json>();
      await showTerminal(tester, api);
      final a = tester.widget<TerminalView>(find.byType(TerminalView)).terminal;
      a.onOutput!('first');
      await tester.pump(const Duration(milliseconds: 20));
      a.onOutput!('second');
      await tester.pump(const Duration(milliseconds: 20));
      await tester.tap(find.text('Terminal B'));
      await tester.pump();
      final b = tester.widget<TerminalView>(find.byType(TerminalView)).terminal;
      b.onOutput!('third');
      await tester.pump(const Duration(milliseconds: 20));
      expect(api.actions.where((a) => a['action'] == 'input'), hasLength(1));
      b.onOutput!('x' * 65536);
      await tester.pump();
      expect(find.textContaining('输入积压过多'), findsOneWidget);
      final delayed = api.slowInput!;
      api.slowInput = null;
      delayed.complete({'ok': true});
      await tester.pump();
      final sent = api.actions.where((a) => a['action'] == 'input').toList();
      expect(sent.map((a) => a['text']), ['first', 'second', 'third']);
      expect(sent.map((a) => a['terminalId']), ['a', 'a', 'b']);
      await cleanup(tester, api);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'hidden terminal cancels reads and resumes without replacing its buffer',
    (tester) async {
      final api = TerminalApi();
      await showTerminal(tester, api);
      final oldScope = api.reads.last.scope!;
      final oldTerminal = tester
          .widget<TerminalView>(find.byType(TerminalView))
          .terminal;
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      await tester.pump(const Duration(seconds: 2));
      expect(oldScope.cancelled, isTrue);
      final count = api.reads.length;
      await tester.pump(const Duration(seconds: 2));
      expect(api.reads.length, count);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      expect(api.reads.length, greaterThan(count));
      expect(api.reads.last.scope!.cancelled, isFalse);
      expect(
        identical(
          oldTerminal,
          tester.widget<TerminalView>(find.byType(TerminalView)).terminal,
        ),
        isTrue,
      );
      final activeScope = api.reads.last.scope!;
      await cleanup(tester, api);
      expect(activeScope.cancelled, isTrue);
      final ended = api.reads.length;
      await tester.pump(const Duration(seconds: 2));
      expect(api.reads.length, ended);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'failed close retains the selected terminal and reports the error',
    (tester) async {
      final api = TerminalApi()..slowClose = Completer<Json>();
      await showTerminal(tester, api);
      final selected = tester
          .widget<TerminalView>(find.byType(TerminalView))
          .terminal;
      await tester.tap(find.byTooltip('关闭当前终端'));
      await tester.pump();
      api.slowClose!.completeError(DshException('close-failed', '拒绝关闭'));
      await tester.pump();
      expect(
        identical(
          selected,
          tester.widget<TerminalView>(find.byType(TerminalView)).terminal,
        ),
        isTrue,
      );
      expect(find.textContaining('拒绝关闭'), findsOneWidget);
      await cleanup(tester, api);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'switch cancels the old read and starts the selected terminal immediately',
    (tester) async {
      final api = TerminalApi()..slowRead = Completer<Json>();
      await showTerminal(tester, api);
      final old = api.reads.single.scope!;
      await tester.tap(find.text('Terminal B'));
      await tester.pump();
      final cancelled = old.cancelled;
      final hasB = api.reads.any((r) => r.id == 'b');
      api.slowRead!.completeError(DshException('read', 'old terminal error'));
      await tester.pump();
      final staleError = find
          .textContaining('old terminal error')
          .evaluate()
          .isNotEmpty;
      await cleanup(tester, api);
      expect(cancelled, isTrue);
      expect(hasB, isTrue);
      expect(staleError, isFalse);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'queued input stays with its terminal when switching within the batching delay',
    (tester) async {
      final api = TerminalApi();
      await showTerminal(tester, api);
      final old = tester
          .widget<TerminalView>(find.byType(TerminalView))
          .terminal;
      old.onOutput!('alpha');
      await tester.tap(find.text('Terminal B'));
      await tester.pump(const Duration(milliseconds: 20));
      final sent = api.actions.where((a) => a['action'] == 'input').toList();
      await cleanup(tester, api);
      expect(sent, hasLength(1));
      expect(sent.single['terminalId'], 'a');
      expect(sent.single['text'], 'alpha');
      expect(old.onOutput, isNull);
      expect(old.onResize, isNull);
    },
  );

  testWidgets('late close response does not reset a newly selected terminal', (
    tester,
  ) async {
    final api = TerminalApi()..slowClose = Completer<Json>();
    await showTerminal(tester, api);
    await tester.tap(find.byTooltip('关闭当前终端'));
    await tester.pump();
    await tester.tap(find.text('Terminal B'));
    await tester.pump();
    final selected = tester
        .widget<TerminalView>(find.byType(TerminalView))
        .terminal;
    api.slowClose!.complete({'closed': true});
    await tester.pump();
    await tester.pump();
    final retained = tester
        .widget<TerminalView>(find.byType(TerminalView))
        .terminal;
    await cleanup(tester, api);
    expect(identical(selected, retained), isTrue);
    expect(
      api.actions.where((a) => a['action'] == 'close').single['terminalId'],
      'a',
    );
    expect(tester.takeException(), isNull);
  });
}
