import 'dart:async';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;
import 'controller_test.dart' as fixture;

const draftWorkspaces = [
  {'workspaceId': 'one', 'path': 'E:/one'},
  {'workspaceId': 'two', 'path': 'E:/two'},
];

Json inventory() => {'items': draftWorkspaces, 'archivedSessionIds': []};

void main() {
  test('workspace retarget during adopted-session history loading cannot admit a late resource', () async {
    final history = Completer<HistoryPage>();
    final api = FakeClient()
      ..handleCall = (method, _) async =>
          method == 'session.create' ? {'sessionId': 'created'} : inventory();
    api.histories['created'] = history.future;
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1:58080');
    await Future<void>.delayed(Duration.zero);
    c.targetWorkspace('one');
    c.setDraft('preserve adopted draft');
    final creating = c.create('E:/one');
    for (
      var attempt = 0;
      attempt < 10 && c.selectedId != 'created';
      attempt++
    ) {
      await Future<void>.delayed(Duration.zero);
    }
    expect(c.selectedId, 'created');
    c.targetWorkspace('two');
    c.targetWorkspace('one');
    history.complete(fixture.historyPage('loaded'));
    expect(await creating, isNull);
    expect(c.preferences.drafts['created'], 'preserve adopted draft');
    c.dispose();
  });
  test('unnamed drafts flush before debounce and survive a preferences reload by scope', () async {
    final directory = await Directory.systemTemp.createTemp('dsh-hero-draft-');
    final file = File('${directory.path}/preferences.json');
    final prefs = DesktopPreferences(
      address: 'http://127.0.0.1:58080/',
      writer: (value) async {
        await file.writeAsString(value, flush: true);
      },
    );
    final c = DesktopController(prefs)..workspaceId = 'one';
    DesktopController? restored;
    try {
      c.setDraft('尚未发送的首条消息');
      final key = c.draftScopeKey;
      expect(key, startsWith(DesktopController.unnamedDraftPrefix));
      await prefs.save();
      final reloaded = await DesktopPreferences.load(fromFile: file);
      restored = DesktopController(reloaded)..workspaceId = 'one';
      expect(restored.draft, '尚未发送的首条消息');
      reloaded.address = 'http://127.0.0.1:58080';
      expect(restored.draftScopeKey, key);
      restored.workspaceId = 'two';
      expect(restored.draft, isEmpty);
      restored.workspaceId = 'one';
      reloaded.address = 'http://127.0.0.1:59080';
      expect(restored.draft, isEmpty);
      reloaded.address = 'http://127.0.0.1:58080';
      expect(restored.draft, '尚未发送的首条消息');
    } finally {
      c.dispose();
      restored?.dispose();
      // Drain any debounce write that started before controller disposal.
      await prefs.save();
      expect(
        directory.parent.absolute.path,
        Directory.systemTemp.absolute.path,
      );
      await directory.delete(recursive: true);
    }
  });

  group('unnamed draft admission ownership', () {
    late DesktopController c;
    late FakeClient api;
    setUp(() async {
      api = FakeClient()..handleCall = (_, _) async => inventory();
      c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
      await c.connect('http://127.0.0.1:58080');
      await Future<void>.delayed(Duration.zero);
      c.targetWorkspace('one');
    });
    tearDown(() async {
      c.dispose();
      await Future<void>.delayed(Duration.zero);
    });

    test('workspace and Host switches keep separate drafts', () async {
      c.setDraft('Host A / one');
      final first = c.draftScopeKey;
      c.targetWorkspace('two');
      expect(c.draft, isEmpty);
      c.setDraft('Host A / two');
      c.targetWorkspace('one');
      expect(c.draft, 'Host A / one');
      api = FakeClient()..handleCall = (_, _) async => inventory();
      await c.connect('http://127.0.0.1:59080');
      await Future<void>.delayed(Duration.zero);
      c.targetWorkspace('one');
      expect(c.draft, isEmpty);
      c.setDraft('Host B / one');
      expect(c.draftScopeKey, isNot(first));
      expect(c.preferences.drafts[first], 'Host A / one');
      api = FakeClient()..handleCall = (_, _) async => inventory();
      await c.connect('http://127.0.0.1:58080');
      await Future<void>.delayed(Duration.zero);
      c.targetWorkspace('one');
      expect(c.draft, 'Host A / one');
    });

    test('repeated Hero navigation preserves text and an existing session never inherits it', () async {
      c.setDraft('workspace draft');
      c.newConversation();
      expect(c.draft, 'workspace draft');
      c.preferences.drafts['existing'] = 'existing draft';
      await c.select('existing');
      expect(c.draft, 'existing draft');
      c.newConversation();
      expect(c.draft, 'workspace draft');
    });

    test('creation failure leaves its unnamed draft intact', () async {
      c.setDraft('keep after failure');
      final key = c.draftScopeKey;
      api.handleCall = (method, _) async {
        if (method == 'session.create') {
          throw DshException('create-failed', '创建失败');
        }
        return inventory();
      };
      await expectLater(c.create('E:/one'), throwsA(isA<DshException>()));
      expect(c.selectedId, isNull);
      expect(c.preferences.drafts[key], 'keep after failure');
      expect(c.draft, 'keep after failure');
    });

    test('a late creation cannot consume a retargeted workspace draft, including a round trip', () async {
      final creating = Completer<Json>();
      api.handleCall = (method, _) async =>
          method == 'session.create' ? creating.future : inventory();
      c.setDraft('original');
      final original = c.draftScopeKey;
      final pending = c.create('E:/one');
      c.targetWorkspace('two');
      c.setDraft('another workspace');
      final other = c.draftScopeKey;
      c.targetWorkspace('one');
      c.setDraft('newer draft in original workspace');
      creating.complete({'sessionId': 'late-created'});
      expect(await pending, isNull);
      expect(c.selectedId, isNull);
      expect(
        c.preferences.drafts[original],
        'newer draft in original workspace',
      );
      expect(c.preferences.drafts[other], 'another workspace');
      expect(c.preferences.drafts.containsKey('late-created'), isFalse);
    });

    test(
      'a late creation on another Host cannot migrate either Host draft',
      () async {
        final creating = Completer<Json>();
        api.handleCall = (method, _) async =>
            method == 'session.create' ? creating.future : inventory();
        c.setDraft('old Host draft');
        final oldKey = c.draftScopeKey;
        final pending = c.create('E:/one');
        api = FakeClient()..handleCall = (_, _) async => inventory();
        await c.connect('http://127.0.0.1:59080');
        await Future<void>.delayed(Duration.zero);
        c.targetWorkspace('one');
        c.setDraft('new Host draft');
        creating.complete({'sessionId': 'late-created'});
        expect(await pending, isNull);
        expect(c.draft, 'new Host draft');
        expect(c.preferences.drafts[oldKey], 'old Host draft');
        expect(c.preferences.drafts.containsKey('late-created'), isFalse);
      },
    );

    test('accepted first send removes its Hero slot and never restores the submitted text', () async {
      var creates = 0;
      api.handleCall = (method, _) async {
        if (method == 'session.create') {
          creates++;
          return {'sessionId': 'created'};
        }
        if (method == 'session.prompt') return {'accepted': true};
        return inventory();
      };
      c.setDraft('first message');
      final hero = c.draftScopeKey;
      expect(await c.sendParts('first message', []), 'created');
      expect(creates, 1);
      expect(c.draft, isEmpty);
      expect(c.preferences.drafts.containsKey(hero), isFalse);
      c.newConversation();
      expect(c.draft, isEmpty);
    });

    test(
      'new typing during creation survives admission of the earlier snapshot',
      () async {
        final creating = Completer<Json>();
        String? submitted;
        api.handleCall = (method, payload) async {
          if (method == 'session.create') return creating.future;
          if (method == 'session.prompt') {
            submitted = '${object((payload['content'] as List).first)['text']}';
            return {'accepted': true};
          }
          return inventory();
        };
        final hero = c.draftScopeKey;
        final sending = c.sendParts('submitted text', []);
        c.setDraft('new typing while creation is pending');
        creating.complete({'sessionId': 'created'});
        expect(await sending, 'created');
        expect(submitted, 'submitted text');
        expect(c.draft, 'new typing while creation is pending');
        expect(c.preferences.drafts.containsKey(hero), isFalse);
      },
    );

    test('history failure after acknowledged creation preserves the named draft without creating again', () async {
      var creates = 0;
      final history = Completer<HistoryPage>();
      api.histories['created'] = history.future;
      api.handleCall = (method, _) async {
        if (method == 'session.create') {
          creates++;
          return {'sessionId': 'created'};
        }
        if (method == 'session.prompt') return {'accepted': false};
        return inventory();
      };
      c.setDraft('recover on created session');
      final creating = c.create('E:/one');
      final failure = expectLater(creating, throwsStateError);
      await Future<void>.delayed(Duration.zero);
      history.completeError(StateError('history unavailable'));
      await failure;
      expect(c.selectedId, 'created');
      expect(c.draft, 'recover on created session');
      await expectLater(c.sendParts(c.draft, []), throwsStateError);
      expect(creates, 1);
      expect(c.draft, 'recover on created session');
    });
  });
}
