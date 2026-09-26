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
  Future<({DesktopController c, FakeClient api})> mount(
    WidgetTester tester,
  ) async {
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
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.pump();
    });
    return (c: c, api: api);
  }

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
    },
  );

  testWidgets('starting another blank task clears the previous blank draft', (
    tester,
  ) async {
    final setup = await mount(tester);
    final dynamic state = tester.state(find.byType(Conversation));
    await tester.enterText(
      find.byKey(const Key('prompt-input')),
      'old blank draft',
    );
    await state.addFiles([XFile.fromData(png, path: 'old.png')]);
    setup.c.newConversation();
    await tester.pumpAndSettle();
    expect(state.input.text, isEmpty);
    expect(state.attachments, isEmpty);
    expect(tester.takeException(), isNull);
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
    },
  );
}
