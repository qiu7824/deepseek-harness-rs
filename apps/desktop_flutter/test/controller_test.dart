import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter_test/flutter_test.dart';

class MemoryPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class FakeChannel extends EventChannel {
  FakeChannel() : super(Uri.parse('ws://127.0.0.1'));
  final status = StreamController<bool>.broadcast();
  final data = StreamController<HostFrame>.broadcast();
  @override
  Stream<bool> get states => status.stream;
  @override
  Stream<HostFrame> get frames => data.stream;
  @override
  void start() {
    status.add(true);
  }

  @override
  Future<void> close() async {
    await status.close();
    await data.close();
  }
}

class FakeClient extends DshClient {
  FakeClient() : super('http://127.0.0.1');
  final channels = <FakeChannel>[];
  List<SessionSummary> liveSessions = [];
  List<Json> commandDescriptors = [];
  Future<List<Json>>? commandListResult;
  @override
  Future<List<Json>> availableCommands(String sessionId) async =>
      await (commandListResult ?? Future.value(commandDescriptors));
  final histories = <String, Future<HistoryPage>>{};
  Completer<Json>? admission;
  Completer<bool>? interactionReceipt;
  int responseCalls = 0;
  @override
  Future<bool> respond(HostFrame frame, Json value) {
    responseCalls++;
    return interactionReceipt?.future ?? Future.value(true);
  }

  Future<Json> Function(String, Json)? handleCall;
  Future<Json> Function(String, Json, RequestScope?)? handleRpc;
  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) =>
      handleRpc?.call(method, payload, scope) ??
      call(method, payload, mutation);
  int historyReads = 0;
  @override
  Future<HostInfo> describe() async =>
      HostInfo.fromJson({'home': 'test', 'version': 'test', 'cwd': 'test'});
  @override
  Future<List<SessionSummary>> sessions() async => liveSessions;
  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async => handleCall != null
      ? await handleCall!(method, payload)
      : {'items': [], 'archivedSessionIds': []};
  @override
  EventChannel events(String name) {
    final channel = FakeChannel();
    channels.add(channel);
    return channel;
  }

  @override
  Future<HistoryPage> history(
    String id, {
    int? before,
    int? after,
    RequestScope? scope,
  }) {
    historyReads++;
    return histories[id] ?? Future.value(historyPage(id));
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async => {'providers': []};
  @override
  Future<ModelCatalog> models(String id) async => ModelCatalog.fromJson({
    'current': {'provider': 'p', 'model': id},
    'routable': true,
    'groups': [],
  });
  @override
  Future<Json> prompt(String id, String text, {required String requestId}) =>
      admission?.future ?? Future.value({'accepted': true});
  @override
  Future<void> close() async {
    for (final channel in channels) {
      await channel.close();
    }
    await super.close();
  }
}

HistoryPage historyPage(String text) => HistoryPage.fromJson({
  'events': [
    {
      'event': {
        'seq': 0,
        'time': 0,
        'type': 'user/message',
        'data': {
          'content': [
            {'type': 'text', 'text': text},
          ],
        },
      },
    },
  ],
  'firstSeq': 0,
  'lastSeq': 0,
  'hasMoreBefore': false,
  'hasMoreAfter': false,
});

void main() {
  late FakeClient api;
  late DesktopController c;
  setUp(() async {
    api = FakeClient();
    c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
  });
  tearDown(() async {
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });
  test(
    'archive metadata does not discard summaries and restore keeps selection',
    () async {
      api.liveSessions = [
        SessionSummary.fromJson({
          'sessionId': 'active',
          'displayTitle': 'Active',
        }),
        SessionSummary.fromJson({
          'sessionId': 'archived',
          'displayTitle': 'Archived',
        }),
      ];
      var archived = true;
      api.handleCall = (method, payload) async {
        if (method == 'workspace.unarchiveSession') {
          expect(payload['sessionId'], 'archived');
          archived = false;
        }
        return {
          'items': [],
          'archivedSessionIds': [if (archived) 'archived'],
        };
      };
      await c.refreshSessions();
      expect(c.sessions.map((session) => session.id), ['active', 'archived']);
      expect(c.archivedSessionIds, {'archived'});
      expect(c.archivedSessions.single['sessionId'], 'archived');
      c.selectedId = 'archived';
      expect(c.selected!.title, 'Archived');
      await c.archive('archived', restore: true);
      expect(c.selectedId, 'archived');
      expect(c.sessions, hasLength(2));
      expect(c.archivedSessionIds, isEmpty);
      expect(c.archivedSessions, isEmpty);
    },
  );
  test('failed plan command never fabricates state, and accepted command read failure is distinguished', () async {
    await c.select('s');
    c.projectionWindow.apply('plan', {'active': true, 'pending': false}, 1);
    final calls = <String>[];
    api.handleRpc = (method, payload, scope) async {
      calls.add(method);
      return {
        'result': {'kind': 'error', 'text': 'rejected'},
      };
    };
    await c.run(() => c.setPlanMode(false));
    expect(calls, ['commands.execute']);
    expect(c.planMode!.active, isTrue);
    expect(c.error, contains('rejected'));
    api.handleRpc = (method, payload, scope) async {
      if (method == 'commands.execute') {
        return {
          'result': {'kind': 'success'},
        };
      }
      throw DshException('read', 'unavailable');
    };
    await c.run(() => c.setPlanMode(false));
    expect(c.error, contains('已被接受'));
    expect(c.planMode!.active, isTrue);
    expect(c.changingPlanMode, isFalse);
  });
  test('plan toggle reads authoritative pending state without replacing historical content', () async {
    await c.select('s');
    c.holdHistory(0);
    final history = c.window;
    c.projectionWindow.apply('plan', {'active': false, 'pending': false}, 1);
    var commands = 0;
    api.handleRpc = (method, payload, scope) async {
      if (method == 'commands.execute') {
        commands++;
        expect(object(payload['args'])['line'], '/plan');
        return {
          'result': {'kind': 'success'},
        };
      }
      expect(payload['maxMessages'], 1);
      return {
        'projections': {
          'asOfSeq': 2,
          'values': {
            'plan': {'active': false, 'pending': true},
          },
        },
      };
    };
    await c.setPlanMode(true);
    expect(commands, 1);
    expect(c.planMode!.active, isFalse);
    expect(c.planMode!.requestedActive, isTrue);
    expect(c.readingHistory, isTrue);
    expect(identical(c.window, history), isTrue);
  });
  test(
    'plan mutation admission blocks duplicate changes and chat submission',
    () async {
      await c.select('s');
      final command = Completer<Json>();
      var calls = 0;
      api.handleRpc = (method, payload, scope) async {
        if (method == 'commands.execute') {
          calls++;
          return command.future;
        }
        return {
          'projections': {
            'asOfSeq': 2,
            'values': {
              'plan': {'active': true, 'pending': false},
            },
          },
        };
      };
      final changing = c.setPlanMode(true);
      await c.setPlanMode(false);
      expect(await c.sendParts('retained', []), isNull);
      expect(calls, 1);
      expect(c.changingPlanMode, isTrue);
      command.complete({
        'result': {'kind': 'success'},
      });
      await changing;
      expect(c.changingPlanMode, isFalse);
      expect(c.planMode!.active, isTrue);
    },
  );
  test('late plan state cannot overwrite a newer live projection or another session', () async {
    await c.select('s');
    final read = Completer<Json>();
    RequestScope? reading;
    api.handleRpc = (method, payload, scope) async {
      if (method == 'commands.activity') return {'active': false};
      if (method == 'commands.execute') {
        return {
          'result': {'kind': 'success'},
        };
      }
      reading = scope;
      return read.future;
    };
    final changing = c.setPlanMode(true);
    await Future<void>.delayed(Duration.zero);
    c.projectionWindow.apply('plan', {'active': true, 'pending': false}, 9);
    read.complete({
      'projections': {
        'asOfSeq': 3,
        'values': {
          'plan': {'active': false, 'pending': true},
        },
      },
    });
    await changing;
    expect(c.planMode!.active, isTrue);
    expect(c.planMode!.pending, isFalse);
    final next = Completer<Json>();
    api.handleRpc = (method, payload, scope) async {
      if (method == 'commands.activity') return {'active': false};
      if (method == 'commands.execute') {
        return {
          'result': {'kind': 'success'},
        };
      }
      reading = scope;
      return next.future;
    };
    final old = c.setPlanMode(false);
    await Future<void>.delayed(Duration.zero);
    await c.select('other');
    expect(reading!.cancelled, isTrue);
    next.complete({
      'projections': {
        'asOfSeq': 10,
        'values': {
          'plan': {'active': true, 'pending': false},
        },
      },
    });
    await old;
    expect(c.planMode, isNull);
    expect(c.changingPlanMode, isFalse);
  });
  test('plan commands dispatch through commands API, while noncommands remain prompts', () async {
    await c.select('s');
    api.commandDescriptors = [
      {'name': 'plan', 'description': '切换计划模式'},
    ];
    final calls = <String>[];
    api.handleCall = (method, payload) async {
      calls.add(method);
      if (method == 'commands.execute') {
        expect(object(payload['args'])['line'], '/plan off');
        return {
          'result': {'kind': 'success'},
        };
      }
      return {'accepted': true, 'items': [], 'archivedSessionIds': []};
    };
    expect(await c.sendParts('/plan off', []), 's');
    expect(calls, contains('commands.execute'));
    expect(calls, isNot(contains('session.prompt')));
    calls.clear();
    expect(await c.sendParts('/planet', []), 's');
    expect(calls, contains('session.prompt'));
    calls.clear();
    await expectLater(
      c.sendParts('/plan', [
        {'type': 'image'},
      ]),
      throwsStateError,
    );
    expect(calls, isEmpty);
  });
  test('registered slash commands execute while unknown lines stay ordinary prompts', () async {
    await c.select('s');
    api.commandDescriptors = [
      {'name': 'goal', 'description': '管理目标'},
    ];
    final calls = <String>[];
    api.handleCall = (method, payload) async {
      if (method == 'commands.execute') {
        expect(object(payload['args'])['line'], '/goal status');
        calls.add(method);
        return {
          'result': {'kind': 'success'},
        };
      }
      if (method == 'session.prompt') calls.add(method);
      return {'accepted': true, 'items': [], 'archivedSessionIds': []};
    };
    expect(await c.sendParts('/goal status', []), 's');
    expect(calls, ['commands.execute']);
    calls.clear();
    expect(await c.sendParts('/goalkeeper', []), 's');
    expect(calls, ['session.prompt']);
    calls.clear();
    await expectLater(
      c.sendParts('/goal status', [
        {'type': 'image'},
      ]),
      throwsStateError,
    );
    expect(calls, isEmpty);
    api.commandDescriptors = [];
    expect(
      await c.sendParts('/plan', [
        {'type': 'image'},
      ]),
      's',
    );
    expect(calls, ['session.prompt']);
  });
  test(
    'old command directory cannot submit after session selection changes',
    () async {
      await c.select('s');
      final listing = Completer<List<Json>>();
      api.commandListResult = listing.future;
      var submissions = 0;
      api.handleCall = (method, payload) async {
        if (method == 'commands.execute' || method == 'session.prompt') {
          submissions++;
        }
        return {'accepted': true, 'items': [], 'archivedSessionIds': []};
      };
      final send = c.sendParts('/goal status', []);
      await c.select('other');
      listing.complete([
        {'name': 'goal'},
      ]);
      expect(await send, isNull);
      expect(submissions, 0);
    },
  );
  test('Host switch releases old interaction locks without settling a new same-id request', () async {
    final frame = HostFrame.fromJson({
      'type': 'server-request',
      'rpcId': 'same',
      'payload': {'type': 'approval/requested', 'sessionId': 's'},
    });
    final old = api;
    old.interactionReceipt = Completer<bool>();
    final first = c.answer(frame, {});
    expect(c.answering, contains('same'));
    api = FakeClient()..interactionReceipt = Completer<bool>();
    await c.connect('http://127.0.0.1:2');
    await Future<void>.delayed(Duration.zero);
    expect(c.connected, isTrue);
    final second = c.answer(frame, {});
    expect(api.responseCalls, 1);
    old.interactionReceipt!.complete(true);
    await first;
    expect(c.answering, contains('same'));
    api.interactionReceipt!.complete(true);
    await second;
    expect(c.answering, isEmpty);
  });
  test(
    'new conversation send locks admission before creating a session',
    () async {
      final creating = Completer<Json>();
      var creates = 0, prompts = 0;
      api.handleCall = (method, payload) async {
        if (method == 'session.create') {
          creates++;
          return creating.future;
        }
        if (method == 'session.prompt') {
          prompts++;
          return {'accepted': true};
        }
        return {'items': [], 'archivedSessionIds': []};
      };
      c.workspaces = [
        {'workspaceId': 'w', 'path': r'E:\project'},
      ];
      c.workspaceId = 'w';
      final first = c.sendParts('first', []);
      final locked = c.sending;
      final second = c.sendParts('second', []);
      creating.complete({'sessionId': 'created'});
      await Future.wait([first, second]);
      expect(locked, isTrue);
      expect(creates, 1);
      expect(prompts, 1);
    },
  );
  test(
    'late session creation cannot steal selection or submit to another session',
    () async {
      final creating = Completer<Json>();
      var prompts = 0;
      api.handleCall = (method, payload) async {
        if (method == 'session.create') return creating.future;
        if (method == 'session.prompt') {
          prompts++;
          return {'accepted': true};
        }
        return {'items': [], 'archivedSessionIds': []};
      };
      c.workspaces = [
        {'workspaceId': 'w', 'path': r'E:\project'},
      ];
      c.workspaceId = 'w';
      final send = c.sendParts('original', []);
      await c.select('other');
      creating.complete({'sessionId': 'created'});
      await send;
      expect(c.selectedId, 'other');
      expect(prompts, 0);
      expect(c.sending, isFalse);
    },
  );
  test(
    'live projections are coalesced and survive a stale history response',
    () async {
      final delayed = Completer<HistoryPage>();
      api.histories['s'] = delayed.future;
      final selecting = c.select('s');
      var notifications = 0;
      c.projectionChanges.addListener(() => notifications++);
      for (var i = 1; i <= 40; i++) {
        api.channels.first.data.add(
          HostFrame.fromJson({
            'type': 'server-request',
            'rpcId': 'p-$i',
            'payload': {
              'type': 'session/projection',
              'sessionId': 's',
              'key': 'sessionStats',
              'seq': i,
              'value': {'steps': i},
            },
          }),
        );
      }
      await Future<void>.delayed(const Duration(milliseconds: 80));
      expect(notifications, 1);
      delayed.complete(
        HistoryPage.fromJson({
          'events': [],
          'projections': {
            'asOfSeq': 1,
            'values': {
              'sessionStats': {'steps': 1},
            },
          },
        }),
      );
      await selecting;
      expect(object(c.projections['sessionStats'])['steps'], 40);
      await c.select('other');
      expect(c.projections, isEmpty);
    },
  );
  test(
    'host switch during creation cannot send through the new Host',
    () async {
      final creating = Completer<Json>();
      final oldApi = api;
      var oldPrompts = 0, newPrompts = 0;
      oldApi.handleCall = (method, payload) async {
        if (method == 'session.create') return creating.future;
        if (method == 'session.prompt') oldPrompts++;
        return {'items': [], 'archivedSessionIds': []};
      };
      c.workspaces = [
        {'workspaceId': 'w', 'path': r'E:\project'},
      ];
      c.workspaceId = 'w';
      final pending = c.sendParts('old Host message', []);
      api = FakeClient()
        ..handleCall = (method, payload) async {
          if (method == 'session.prompt') newPrompts++;
          return {'items': [], 'archivedSessionIds': []};
        };
      await c.connect('http://127.0.0.1:2');
      await c.select('new-host-session');
      creating.complete({'sessionId': 'old-created'});
      expect(await pending, isNull);
      expect(c.selectedId, 'new-host-session');
      expect(oldPrompts + newPrompts, 0);
      expect(c.error, isNull);
    },
  );
  test(
    'an admitted prompt retains its receipt even when refreshing fails',
    () async {
      await c.select('s');
      c.setDraft('message');
      api.handleCall = (method, payload) async {
        if (method == 'session.prompt') return {'accepted': true};
        if (method == 'workspace.list') {
          throw DshException('read', 'refresh failed');
        }
        return {'items': [], 'archivedSessionIds': []};
      };
      expect(await c.sendParts('message', []), 's');
      expect(c.draft, '');
      expect(c.error, contains('消息已发送'));
      expect(c.sending, isFalse);
    },
  );
  test(
    'rejected multipart prompt preserves its draft and returns no receipt',
    () async {
      await c.select('s');
      api.handleCall = (method, payload) async => {'accepted': false};
      await expectLater(c.sendParts('message', []), throwsStateError);
      expect(c.draft, 'message');
      expect(c.sending, isFalse);
    },
  );

  test(
    'late history and model responses cannot replace another task',
    () async {
      final delayed = Completer<HistoryPage>();
      api.histories['first'] = delayed.future;
      final first = c.select('first');
      await c.select('second');
      delayed.complete(historyPage('stale'));
      await first;
      expect(c.selectedId, 'second');
      expect(c.transcript.single.text, 'second');
      expect(c.catalog!.current['model'], 'second');
    },
  );
  test(
    'repeated idle status does not poll history or rebuild the shell',
    () async {
      await c.select('s');
      c.sessions = [
        SessionSummary.fromJson({'sessionId': 's', 'running': false}),
      ];
      final reads = api.historyReads;
      var rebuilt = 0;
      c.addListener(() => rebuilt++);
      for (var i = 0; i < 20; i++) {
        api.channels.first.data.add(
          HostFrame.fromJson({
            'type': 'server-request',
            'rpcId': 'idle-$i',
            'payload': {
              'type': 'host/session-status',
              'sessionId': 's',
              'running': false,
            },
          }),
        );
      }
      await Future<void>.delayed(const Duration(milliseconds: 450));
      expect(api.historyReads, reads);
      expect(rebuilt, 0);
    },
  );
  test('budget eviction of a complete history page does not schedule a snapshot loop', () async {
    api.histories['large'] = Future.value(
      HistoryPage.fromJson({
        'events': [
          for (var i = 0; i < 3; i++)
            {
              'event': {
                'seq': i,
                'type': 'step/start',
                'data': {'payload': 'x' * (3 * 1024 * 1024)},
              },
            },
        ],
        'firstSeq': 0,
        'lastSeq': 2,
        'hasMoreBefore': false,
        'hasMoreAfter': false,
      }),
    );
    await c.select('large');
    final reads = api.historyReads;
    expect(c.window.hasBefore, isTrue);
    expect(c.window.retainedBytes, lessThanOrEqualTo(8 * 1024 * 1024));
    await Future<void>.delayed(const Duration(milliseconds: 900));
    expect(
      api.historyReads,
      reads,
      reason: 'intentional head eviction must not trigger tail repair',
    );
    expect(c.window.needsRefresh, isFalse);
  });
  test('draft edited while submission is pending is retained', () async {
    await c.select('s');
    c.setDraft('original');
    api.admission = Completer<Json>();
    final submission = c.send('original');
    c.setDraft('next message');
    api.admission!.complete({'accepted': true});
    await submission;
    expect(c.draft, 'next message');
    expect(c.sending, isFalse);
  });
  test('uncertain submission preserves draft and background refresh preserves error', () async {
    await c.select('s');
    c.setDraft('original');
    api.admission = Completer<Json>();
    final submission = c.run(() => c.send('original'));
    api.admission!.completeError(
      DshException('transport', 'interrupted', outcomeUnknown: true),
    );
    await submission;
    final error = c.error;
    await c.run(c.refreshSessions);
    expect(c.draft, 'original');
    expect(c.error, error);
    expect(c.error, contains('尚未确认'));
  });
}
