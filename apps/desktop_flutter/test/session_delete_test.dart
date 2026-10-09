import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

SessionSummary row(String id) =>
    SessionSummary.fromJson({'sessionId': id, 'running': false});

class SnapshotClient extends FakeClient {
  @override
  Future<List<SessionSummary>> sessions() async => List.of(liveSessions);
}

void removed(FakeClient api, String id) => api.channels.first.data.add(
  HostFrame.fromJson({
    'type': 'server-request',
    'rpcId': 'removed-$id',
    'payload': {'type': 'host/session-removed', 'sessionId': id},
  }),
);

void main() {
  test(
    'new conversation releases buffered events from a cancelled history load',
    () async {
      final api = FakeClient()..liveSessions = [row('s')];
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      final history = Completer<HistoryPage>();
      api.histories['s'] = history.future;
      final selection = c.select('s');
      await Future<void>.delayed(Duration.zero);
      api.channels.first.data.add(
        HostFrame.fromJson({
          'type': 'server-request',
          'rpcId': 'buffered',
          'payload': {
            'type': 'session/event',
            'sessionId': 's',
            'event': {
              'seq': 1,
              'type': 'user/message',
              'data': {
                'content': [
                  {'type': 'text', 'text': 'x' * 262144},
                ],
              },
            },
          },
        }),
      );
      await Future<void>.delayed(Duration.zero);
      expect(
        c.resourceDiagnostics['liveBufferBytes'] as int,
        greaterThan(262144),
      );
      c.newConversation();
      expect(c.resourceDiagnostics['liveBufferBytes'], 0);
      expect(c.resourceDiagnostics['liveBufferEvents'], 0);
      history.complete(HistoryPage.fromJson({'events': []}));
      await selection;
      expect(c.selectedId, isNull);
      expect(c.transcript, isEmpty);
      c.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );

  test('cascade acknowledgement clears selected child and preserves independent fork', () async {
    final api = FakeClient()
      ..liveSessions = [row('parent'), row('child'), row('fork')];
    final preferences = MemoryPreferences()
      ..drafts.addAll({
        'parent': 'parent draft',
        'child': 'child draft',
        'fork': 'independent draft',
      });
    final c = DesktopController(preferences, clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    await c.select('child');
    api.handleCall = (method, payload) async {
      if (method == 'workspace.deleteSession') {
        api.liveSessions = [row('fork')];
        return {
          'deleted': true,
          'deletedSessionIds': ['parent', 'child'],
        };
      }
      return {'items': [], 'archivedSessionIds': []};
    };
    await c.deleteSession('parent');
    expect(c.selectedId, isNull);
    expect(preferences.drafts.keys, contains('fork'));
    expect(preferences.drafts.keys, isNot(contains('child')));
    expect(preferences.drafts.keys, isNot(contains('parent')));
    expect(c.sessions.single.id, 'fork');
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });

  test(
    'deletion acknowledges before clearing selected state and draft',
    () async {
      final api = FakeClient()..liveSessions = [row('s')];
      final preferences = MemoryPreferences()..drafts['s'] = 'unsent';
      final c = DesktopController(preferences, clientFactory: (_) => api);
      await c.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      await c.select('s');
      final receipt = Completer<Json>();
      api.handleCall = (method, payload) async {
        if (method == 'workspace.deleteSession') {
          expect(payload, {'sessionId': 's', 'stopSchedules': true});
          return receipt.future;
        }
        return {'items': [], 'archivedSessionIds': []};
      };
      final deletion = c.deleteSession(
        's',
        stopSchedules: true,
        expectedClient: api,
      );
      expect(c.selectedId, 's');
      expect(preferences.drafts['s'], 'unsent');
      api.liveSessions = [];
      receipt.complete({'deleted': true});
      await deletion;
      expect(c.selectedId, isNull);
      expect(c.transcript, isEmpty);
      expect(preferences.drafts.containsKey('s'), isFalse);
      c.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );

  test('failed deletion retains draft and session', () async {
    final api = FakeClient()..liveSessions = [row('s')];
    final preferences = MemoryPreferences()..drafts['s'] = 'keep';
    final c = DesktopController(preferences, clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    await c.select('s');
    api.handleCall = (method, payload) async {
      if (method == 'workspace.deleteSession') throw StateError('disk failure');
      return {};
    };
    await expectLater(c.deleteSession('s'), throwsStateError);
    expect(c.selectedId, 's');
    expect(preferences.drafts['s'], 'keep');
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });

  test(
    'removed event clears retained state and discards an older list',
    () async {
      final api = SnapshotClient()..liveSessions = [row('s')];
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      await c.select('s');
      c.rememberReadingPosition(
        's',
        seq: 1,
        itemId: 'item',
        viewportOffset: 0,
        follow: false,
      );
      // Hold workspace inventory so the request predates the removal event.
      final inventory = Completer<Json>();
      api.handleCall = (method, payload) async =>
          method == 'workspace.list' ? inventory.future : <String, dynamic>{};
      final refresh = c.refreshSessions();
      await Future<void>.delayed(Duration.zero);
      removed(api, 's');
      await Future<void>.delayed(Duration.zero);
      inventory.complete({'items': [], 'archivedSessionIds': []});
      await refresh;
      expect(c.sessions, isEmpty);
      expect(c.selectedId, isNull);
      expect(c.readingPositions, isEmpty);
      c.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );

  test(
    'metadata retention follows current session inventory through churn',
    () async {
      final api = FakeClient();
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      for (var n = 0; n < 100; n++) {
        api.liveSessions = [
          SessionSummary.fromJson({
            'sessionId': 's$n',
            'projections': {
              'asOfSeq': n,
              'values': {'title': 'title'},
            },
          }),
        ];
        await c.refreshSessions();
        expect(
          c.resourceDiagnostics['sessionTitleEntries'] as int,
          lessThanOrEqualTo(1),
        );
      }
      api.liveSessions = [];
      await c.refreshSessions();
      expect(c.resourceDiagnostics['sessionTitleEntries'], 0);
      c.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );
}
