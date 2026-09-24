import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

class SessionApi extends FakeClient {
  Completer<List<SessionSummary>>? listing;
  String title = '快照标题';
  int seq = 100;
  @override
  Future<List<SessionSummary>> sessions() async =>
      listing?.future ??
      [
        SessionSummary.fromJson({
          'sessionId': 's',
          'blank': true,
          'running': true,
        }),
      ];
  @override
  Future<HistoryPage> history(
    String id, {
    int? before,
    int? after,
    RequestScope? scope,
  }) async => HistoryPage.fromJson({
    'events': [],
    'projections': {
      'asOfSeq': seq,
      'values': {'title': title},
    },
  });
  void event(String type, Json value) => channels.first.data.add(
    HostFrame.fromJson({
      'type': 'server-request',
      'rpcId': 'test',
      'payload': {'type': type, 'sessionId': 's', ...value},
    }),
  );
}

void main() {
  test('snapshot and live naming follow server titles; old pages cannot revert a rename', () async {
    final api = SessionApi(),
        c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    await c.select('s');
    expect(c.selected!.displayTitle, '快照标题');
    api.event('session/event', {
      'event': {
        'seq': 110,
        'type': 'session/title',
        'data': {'title': '自动生成的会话标题'},
      },
    });
    await Future<void>.delayed(Duration.zero);
    expect(c.selected!.displayTitle, '自动生成的会话标题');
    api.title = '过时标题';
    api.seq = 90;
    await c.loadHistory();
    expect(c.selected!.displayTitle, '自动生成的会话标题');
    api.event('session/event', {
      'event': {
        'seq': 111,
        'type': 'request/phase',
        'data': {'phase': 'completed', 'turn': 1, 'step': 1},
      },
    });
    api.event('session/event', {
      'event': {
        'seq': 112,
        'type': 'step/end',
        'data': {'turn': 1, 'step': 1},
      },
    });
    await Future<void>.delayed(Duration.zero);
    expect(c.running, isTrue);
    api.event('host/session-status', {'status': 'completed'});
    await Future<void>.delayed(Duration.zero);
    expect(c.running, isTrue);
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });
  test('late list response cannot overwrite newer running state and generated name', () async {
    final api = SessionApi(),
        c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    await c.select('s');
    api.listing = Completer();
    final refresh = c.refreshSessions();
    await Future<void>.delayed(Duration.zero);
    api.event('host/session-status', {'running': false});
    api.event('session/projection', {
      'key': 'title',
      'value': '最新标题',
      'seq': 200,
    });
    await Future<void>.delayed(Duration.zero);
    api.listing!.complete([
      SessionSummary.fromJson({
        'sessionId': 's',
        'running': true,
        'projections': {
          'values': {'title': '旧标题'},
        },
      }),
    ]);
    await refresh;
    expect(c.running, isFalse);
    expect(c.selected!.displayTitle, '最新标题');
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });
}
