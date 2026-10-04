import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

const address = 'http://127.0.0.1:58080';
const workspaceList = {
  'items': [
    {'workspaceId': 'one', 'path': 'E:/one'},
    {'workspaceId': 'two', 'path': 'E:/two'},
  ],
};

class CancellationChannel extends EventChannel {
  CancellationChannel(Completer<void> gate)
    : data = StreamController<HostFrame>(onCancel: () => gate.future),
      super(Uri.parse('ws://127.0.0.1'));
  final StreamController<HostFrame> data;
  final status = StreamController<bool>.broadcast();
  @override
  Stream<HostFrame> get frames => data.stream;
  @override
  Stream<bool> get states => status.stream;
  @override
  void start() => status.add(true);
  @override
  Future<void> close() async {
    await data.close();
    await status.close();
  }
}

class RaceClient extends FakeClient {
  RaceClient({this.cancellation, this.description});
  final Completer<void>? cancellation;
  final Completer<HostInfo>? description;
  final gatedChannels = <CancellationChannel>[];
  int closes = 0, eventCalls = 0, describeCalls = 0;
  @override
  Future<HostInfo> describe() {
    describeCalls++;
    return description?.future ?? super.describe();
  }

  @override
  EventChannel events(String name) {
    eventCalls++;
    final gate = cancellation;
    if (gate == null) return super.events(name);
    final channel = CancellationChannel(gate);
    gatedChannels.add(channel);
    return channel;
  }

  @override
  Future<void> close() async {
    closes++;
    await Future.wait(gatedChannels.map((channel) => channel.close()));
    await super.close();
  }
}

Future<void> settle() => Future<void>.delayed(Duration.zero);

void main() {
  test(
    'typing while the initial workspace list loads stays accessible',
    () async {
      final listed = Completer<Json>();
      final api = RaceClient()
        ..handleCall = (method, _) async =>
            method == 'workspace.list' ? listed.future : <String, dynamic>{};
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      addTearDown(c.dispose);
      await c.connect(address);
      await settle();
      expect(c.workspaceId, isNull);
      c.setDraft('typed while connecting');
      final provisionalKey = c.draftScopeKey;
      listed.complete(workspaceList);
      await settle();
      expect(c.workspaceId, 'one');
      expect(c.preferences.drafts[provisionalKey], 'typed while connecting');
      expect(c.draft, 'typed while connecting');
    },
  );

  test('automatic target never overwrites an existing draft and explicit same-target navigation wins', () async {
    final listed = Completer<Json>();
    var api = RaceClient()
      ..handleCall = (method, _) async =>
          method == 'workspace.list' ? listed.future : <String, dynamic>{};
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    addTearDown(c.dispose);
    c.workspaceId = 'one';
    final targetKey = c.unnamedDraftKey;
    c.preferences.drafts[targetKey] = 'existing workspace draft';
    await c.connect(address);
    await settle();
    c.setDraft('new provisional draft');
    final provisionalKey = c.draftScopeKey;
    listed.complete(workspaceList);
    await settle();
    expect(c.workspaceId, 'one');
    expect(c.draft, 'new provisional draft');
    expect(c.preferences.drafts[targetKey], 'existing workspace draft');
    c.targetWorkspace('one');
    expect(c.draft, 'existing workspace draft');
    c.newConversation();
    expect(c.draft, 'existing workspace draft');
    expect(c.preferences.drafts[provisionalKey], 'new provisional draft');

    api = RaceClient()..handleCall = (_, _) async => workspaceList;
    await c.connect(address);
    await settle();
    expect(c.draft, 'new provisional draft');
    c.newConversation();
    expect(c.draft, 'new provisional draft');
    api.handleCall = (method, _) async {
      if (method == 'session.create') return {'sessionId': 'created'};
      if (method == 'session.prompt') return {'accepted': true};
      return workspaceList;
    };
    expect(await c.sendParts(c.draft, []), 'created');
    expect(c.preferences.drafts.containsKey(provisionalKey), isFalse);
    expect(c.preferences.drafts[targetKey], 'existing workspace draft');
    c.newConversation();
    expect(c.draft, 'existing workspace draft');
  });

  test(
    'first manual target preserves unassigned drafts, existing target drafts and other Hosts',
    () async {
      final prefs = MemoryPreferences();
      var api = RaceClient()..handleCall = (_, _) async => workspaceList;
      final c = DesktopController(prefs, clientFactory: (_) => api);
      addTearDown(c.dispose);
      c.setDraft('before any Host is selected');
      final unassigned = c.draftScopeKey;
      c.targetWorkspace('one');
      c.setDraft('unassigned workspace one');
      final unassignedOne = c.draftScopeKey;
      c.targetWorkspace('two');
      c.setDraft('unassigned workspace two');
      final unassignedTwo = c.draftScopeKey;
      final existingOne =
          '${DesktopController.unnamedDraftPrefix}${jsonEncode([address, 'one'])}';
      prefs.drafts[existingOne] = 'saved for the chosen Host';
      prefs.drafts['saved-session'] = 'saved session draft';

      await c.connect(address);
      await settle();
      expect(prefs.automaticHost, isFalse);
      expect(c.draft, 'before any Host is selected');
      expect(c.draftScopeKey, isNot(unassigned));
      c.targetWorkspace('one');
      expect(c.draftScopeKey, existingOne);
      expect(c.draft, 'saved for the chosen Host');
      c.targetWorkspace('two');
      expect(c.draft, 'unassigned workspace two');
      expect(prefs.drafts[unassigned], 'before any Host is selected');
      expect(prefs.drafts[unassignedOne], 'unassigned workspace one');
      expect(prefs.drafts[unassignedTwo], 'unassigned workspace two');
      expect(prefs.drafts['saved-session'], 'saved session draft');

      api = RaceClient()..handleCall = (_, _) async => workspaceList;
      await c.connect('http://127.0.0.1:59080');
      await settle();
      expect(c.draft, isEmpty);
      c.targetWorkspace('two');
      expect(c.draft, isEmpty);
      c.setDraft('the other Host');

      api = RaceClient()..handleCall = (_, _) async => workspaceList;
      await c.connect(address);
      await settle();
      expect(c.draft, 'before any Host is selected');
      c.targetWorkspace('two');
      expect(c.draft, 'unassigned workspace two');
      c.targetWorkspace('one');
      expect(c.draft, 'saved for the chosen Host');
    },
  );

  test(
    'a previously assigned automatic Host does not donate drafts to a manual Host',
    () async {
      final prefs = MemoryPreferences()
        ..address = 'http://127.0.0.1:61234';
      final api = RaceClient()..handleCall = (_, _) async => workspaceList;
      final c = DesktopController(prefs, clientFactory: (_) => api);
      addTearDown(c.dispose);
      c.targetWorkspace('one');
      c.setDraft('automatic Host draft');
      final automaticKey = c.draftScopeKey;
      await c.connect(address);
      await settle();
      c.targetWorkspace('one');
      expect(c.draft, isEmpty);
      expect(prefs.drafts[automaticKey], 'automatic Host draft');
    },
  );

  test('explicit navigation before the late list stays isolated from provisional input', () async {
    final listed = Completer<Json>();
    final api = RaceClient()
      ..handleCall = (method, _) async =>
          method == 'workspace.list' ? listed.future : <String, dynamic>{};
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    addTearDown(c.dispose);
    await c.connect(address);
    await settle();
    c.setDraft('unassigned');
    final provisional = c.draftScopeKey;
    c.targetWorkspace('two');
    expect(c.draft, isEmpty);
    c.setDraft('explicit workspace');
    listed.complete(workspaceList);
    await settle();
    expect(c.workspaceId, 'two');
    expect(c.draft, 'explicit workspace');
    expect(c.preferences.drafts[provisional], 'unassigned');
    c.newConversation();
    expect(c.draft, 'explicit workspace');
  });

  test('provisional input and existing target draft survive restart without sharing their slots', () async {
    final directory = await Directory.systemTemp.createTemp('dsh-provisional-');
    final file = File('${directory.path}/preferences.json');
    Future<void> writer(String value) async {
      await file.writeAsString(value, flush: true);
    }

    final prefs = DesktopPreferences(address: address, writer: writer);
    final before = DesktopController(prefs);
    DesktopController? after;
    DesktopPreferences? restored;
    try {
      before.setDraft('restart provisional');
      final provisional = before.draftScopeKey;
      before.targetWorkspace('one');
      before.setDraft('saved target');
      before.dispose();
      await prefs.save();
      restored = await DesktopPreferences.load(fromFile: file, writer: writer);
      final listed = Completer<Json>();
      final api = RaceClient()
        ..handleCall = (method, _) async =>
            method == 'workspace.list' ? listed.future : <String, dynamic>{};
      after = DesktopController(restored, clientFactory: (_) => api);
      await after.connect(address);
      await settle();
      expect(after.draft, 'restart provisional');
      listed.complete(workspaceList);
      await settle();
      expect(after.workspaceId, 'one');
      expect(after.draftScopeKey, provisional);
      expect(after.draft, 'restart provisional');
      after.targetWorkspace('one');
      expect(after.draft, 'saved target');
    } finally {
      after?.dispose();
      if (restored != null) await restored.save();
      expect(
        directory.parent.absolute.path,
        Directory.systemTemp.absolute.path,
      );
      await directory.delete(recursive: true);
    }
  });

  test('overlapping connection attempts drain independent subscriptions and keep the newest Host', () async {
    final gate = Completer<void>();
    var api = RaceClient(cancellation: gate);
    final original = api;
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect(address);
    await settle();
    api = RaceClient();
    final abandoned = api;
    final a = c.connect('http://127.0.0.1:59080');
    api = RaceClient();
    final wanted = api;
    final b = c.connect('http://127.0.0.1:60080');
    await b;
    expect(c.client, same(wanted));
    expect(wanted.eventCalls, 2);
    gate.complete();
    await a;
    await settle();
    expect(c.client, same(wanted));
    expect(c.resourceDiagnostics['controllerSubscriptions'], 4);
    expect(original.closes, 1);
    expect(abandoned.closes, 1);
    expect(abandoned.describeCalls, 0);
    expect(wanted.closes, 0);
    c.dispose();
    await settle();
    expect(wanted.closes, 1);
  });

  test(
    'a save completion from an older connect cannot register stale events',
    () async {
      final saving = Completer<void>(), entered = Completer<void>();
      final prefs = DesktopPreferences(
        writer: (content) async {
          if (object(jsonDecode(content))['address'] ==
              'http://127.0.0.1:59080') {
            if (!entered.isCompleted) entered.complete();
            await saving.future;
          }
        },
      );
      var api = RaceClient();
      final c = DesktopController(prefs, clientFactory: (_) => api);
      await c.connect(address);
      await settle();
      api = RaceClient();
      final first = api;
      final a = c.connect('http://127.0.0.1:59080');
      await entered.future;
      api = RaceClient();
      final wanted = api;
      final b = c.connect('http://127.0.0.1:60080');
      await settle();
      saving.complete();
      await Future.wait([a, b]);
      expect(first.eventCalls, 0);
      expect(first.closes, 1);
      expect(c.client, same(wanted));
      expect(wanted.eventCalls, 2);
      expect(prefs.address, 'http://127.0.0.1:60080');
      c.dispose();
      await settle();
    },
  );

  test('dispose during subscription retirement closes both owned clients without late publication', () async {
    final gate = Completer<void>();
    var api = RaceClient(cancellation: gate);
    final original = api;
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect(address);
    await settle();
    api = RaceClient();
    final pendingClient = api;
    final connecting = c.connect('http://127.0.0.1:59080');
    c.dispose();
    gate.complete();
    await connecting;
    await settle();
    expect(c.client, isNull);
    expect(original.closes, 1);
    expect(pendingClient.closes, 1);
    expect(pendingClient.eventCalls, 0);
    expect(c.resourceDiagnostics['controllerSubscriptions'], 0);
  });

  test(
    'dispose while preferences are saving does not reopen a closed client',
    () async {
      final saving = Completer<void>(), entered = Completer<void>();
      final api = RaceClient();
      var firstWrite = true;
      final c = DesktopController(
        DesktopPreferences(
          writer: (_) async {
            if (!firstWrite) return;
            firstWrite = false;
            entered.complete();
            await saving.future;
          },
        ),
        clientFactory: (_) => api,
      );
      final connecting = c.connect(address);
      await entered.future;
      c.dispose();
      saving.complete();
      await connecting;
      await settle();
      expect(api.eventCalls, 0);
      expect(api.closes, 1);
      expect(c.client, isNull);
    },
  );
}
