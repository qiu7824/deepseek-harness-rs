import 'dart:async';
import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

class WaitingFile extends XFile {
  WaitingFile(this.pending) : super('pending.png');
  final Completer<Uint8List> pending;
  @override
  Future<int> length() async => 70;
  @override
  Future<Uint8List> readAsBytes() => pending.future;
}

void main() {
  final png = base64Decode(
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==',
  );
  Future<
    ({DesktopController c, FakeClient api, Future<void> Function() cleanup})
  >
  mount(WidgetTester tester) async {
    final api = FakeClient();
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await tester.pump();
    c.workspaces = [
      {'workspaceId': 'w', 'path': 'E:/project', 'sessionIds': <String>[]},
    ];
    c.workspaceId = 'w';
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    var disposed = false;
    Future<void> cleanup() async {
      if (disposed) return;
      disposed = true;
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.pump();
    }

    addTearDown(cleanup);
    return (c: c, api: api, cleanup: cleanup);
  }

  testWidgets(
    'a late command dialog result cannot overwrite another session draft',
    (tester) async {
      final setup = await mount(tester), c = setup.c, api = setup.api;
      api.commandDescriptors = [
        {'name': 'inspect', 'description': '检查'},
      ];
      await c.select('first');
      await tester.pumpAndSettle();
      final dynamic state = tester.state(find.byType(Conversation));
      final Future<void> choosing = state.commandMenu();
      await tester.pumpAndSettle();
      c.preferences.drafts['second'] = 'second draft';
      await c.select('second');
      await tester.pumpAndSettle();
      await tester.tap(find.text('/inspect  检查'));
      await choosing;
      await tester.pumpAndSettle();
      expect(state.input.text, 'second draft');
      expect(c.draft, 'second draft');
      expect(tester.takeException(), isNull);
      await setup.cleanup();
    },
  );

  testWidgets(
    'a late reference dialog result cannot overwrite a new workspace draft',
    (tester) async {
      final setup = await mount(tester), c = setup.c;
      c.sessions = [
        SessionSummary.fromJson({
          'sessionId': 'reference',
          'displayTitle': '引用会话',
        }),
      ];
      c.workspaces.add({'workspaceId': 'other', 'path': 'E:/other'});
      c.targetWorkspace('other');
      c.setDraft('other workspace draft');
      c.targetWorkspace('w');
      await tester.pumpAndSettle();
      final dynamic state = tester.state(find.byType(Conversation));
      final Future<void> choosing = state.referenceMenu();
      await tester.pumpAndSettle();
      c.targetWorkspace('other');
      await tester.pumpAndSettle();
      final currentDraft = c.draft;
      await tester.tap(find.text('引用会话'));
      await choosing;
      await tester.pumpAndSettle();
      expect(state.input.text, currentDraft);
      expect(c.draft, currentDraft);
      expect(tester.takeException(), isNull);
      await setup.cleanup();
    },
  );

  testWidgets(
    'selecting an existing task never inherits a blank composer draft',
    (tester) async {
      final setup = await mount(tester), c = setup.c;
      final dynamic state = tester.state(find.byType(Conversation));
      await tester.enterText(
        find.byKey(const Key('prompt-input')),
        'private unsent draft',
      );
      await state.addFiles([XFile.fromData(png, path: 'private.png')]);
      c.preferences.drafts['existing'] = 'existing task draft';
      await c.select('existing');
      await tester.pumpAndSettle();
      expect(find.text('existing task draft'), findsOneWidget);
      expect(find.text('private unsent draft'), findsNothing);
      expect(state.attachments, isEmpty);
      expect(tester.takeException(), isNull);
      await setup.cleanup();
    },
  );

  testWidgets(
    'returning to the same Hero preserves its text and clears transient attachments',
    (tester) async {
      final setup = await mount(tester);
      final dynamic state = tester.state(find.byType(Conversation));
      await tester.enterText(
        find.byKey(const Key('prompt-input')),
        'old blank draft',
      );
      await state.addFiles([XFile.fromData(png, path: 'old.png')]);
      setup.c.newConversation();
      await tester.pumpAndSettle();
      expect(state.input.text, 'old blank draft');
      expect(state.attachments, isEmpty);
      expect(tester.takeException(), isNull);
      await setup.cleanup();
    },
  );

  testWidgets('changing Hero workspace restores only that workspace text', (
    tester,
  ) async {
    final setup = await mount(tester), c = setup.c;
    final dynamic state = tester.state(find.byType(Conversation));
    await tester.enterText(
      find.byKey(const Key('prompt-input')),
      'workspace one draft',
    );
    c.workspaces.add({'workspaceId': 'other', 'path': 'E:/other'});
    c.targetWorkspace('other');
    await tester.pumpAndSettle();
    expect(state.input.text, isEmpty);
    await tester.enterText(
      find.byKey(const Key('prompt-input')),
      'workspace two draft',
    );
    c.targetWorkspace('w');
    await tester.pumpAndSettle();
    expect(state.input.text, 'workspace one draft');
    c.targetWorkspace('other');
    await tester.pumpAndSettle();
    expect(state.input.text, 'workspace two draft');
    await setup.cleanup();
  });

  testWidgets(
    'creation adopts only its own draft and pending attachment import',
    (tester) async {
      final setup = await mount(tester), c = setup.c, api = setup.api;
      final creation = Completer<Json>();
      api.handleCall = (method, payload) async => method == 'session.create'
          ? creation.future
          : {'items': c.workspaces, 'archivedSessionIds': []};
      final starting = c.startConversation();
      await tester.pump();
      await tester.enterText(
        find.byKey(const Key('prompt-input')),
        'new task draft',
      );
      final bytes = Completer<Uint8List>();
      final dynamic state = tester.state(find.byType(Conversation));
      final Future<void> importing = state.addFiles([WaitingFile(bytes)]);
      await tester.pump();
      creation.complete({'sessionId': 'created'});
      await starting;
      bytes.complete(png);
      await importing;
      await tester.pumpAndSettle();
      expect(c.selectedId, 'created');
      expect(state.input.text, 'new task draft');
      expect(state.attachments, hasLength(1));
      expect(state.importingAttachments, 0);
      expect(tester.takeException(), isNull);
      await setup.cleanup();
    },
  );
}
