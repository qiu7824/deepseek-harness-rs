import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

class StartupClient extends FakeClient {
  final listing = Completer<List<SessionSummary>>();
  final accounts = Completer<Json>();
  final modelList = Completer<ModelCatalog>();
  final presets = Completer<Json>();
  final plugins = Completer<Json>();
  final calls = <String>[];

  @override
  Future<List<SessionSummary>> sessions() => listing.future;
  @override
  Future<ModelCatalog> models(String id) => modelList.future;
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) {
    calls.add(path);
    return accounts.future;
  }

  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    calls.add(method);
    if (method == 'agentPreset.list') return presets.future;
    if (method == 'pluginInventory.list') return plugins.future;
    if (method == 'workspace.list') {
      return {
        'items': [
          {'workspaceId': 'first', 'sessionIds': <String>[]},
          {
            'workspaceId': 'saved-workspace',
            'sessionIds': ['saved'],
          },
        ],
      };
    }
    return {'namespaces': [], 'items': []};
  }

  void complete({bool deleted = false}) {
    if (!listing.isCompleted) {
      listing.complete(
        deleted
            ? []
            : [
                SessionSummary.fromJson({
                  'sessionId': 'saved',
                  'agentPreset': 'code',
                }),
                SessionSummary.fromJson({'sessionId': 'other'}),
              ],
      );
    }
    if (!modelList.isCompleted) {
      modelList.complete(ModelCatalog.fromJson({'groups': []}));
    }
    if (!accounts.isCompleted) {
      accounts.complete({
        'providers': [
          {'id': 'account'},
        ],
      });
    }
    if (!presets.isCompleted) {
      presets.complete({
        'presets': [
          {'id': 'blank'},
        ],
      });
    }
    if (!plugins.isCompleted) plugins.complete({'entries': []});
  }
}

Future<void> flush() async {
  for (var i = 0; i < 5; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  test(
    'saved history renders while sidebar, accounts, models and presets wait',
    () async {
      final api = StartupClient();
      final c = DesktopController(
        MemoryPreferences()..sessionId = 'saved',
        clientFactory: (_) => api,
      );
      addTearDown(() async {
        api.complete();
        await flush();
        c.dispose();
      });
      await c.connect('http://127.0.0.1');
      await flush();
      expect(c.selectedId, 'saved');
      expect(c.transcript.single.text, 'saved');
      expect(c.loading, isFalse);
      expect(api.listing.isCompleted, isFalse);
      expect(
        api.calls,
        containsAll([
          'workspace.list',
          'agentPreset.list',
          'settings.describe',
          '/provider-auth/providers',
          'pluginInventory.list',
        ]),
      );
      final reads = api.historyReads;
      api.channels.first.status.add(true);
      await flush();
      expect(
        api.historyReads,
        reads,
        reason: 'An already-ready socket must not restart restoration.',
      );
      api.complete();
      await flush();
      expect(c.workspaceId, 'saved-workspace');
      expect(c.preset, 'code');
      expect(c.subscriptionAccounts.single['id'], 'account');
      expect(c.error, isNull);
    },
  );

  test(
    'late startup inventory cannot replace a user-selected conversation',
    () async {
      final api = StartupClient();
      final c = DesktopController(
        MemoryPreferences()..sessionId = 'saved',
        clientFactory: (_) => api,
      );
      addTearDown(() async {
        api.complete();
        await flush();
        c.dispose();
      });
      await c.connect('http://127.0.0.1');
      await flush();
      final selection = c.select('other');
      api.complete(deleted: true);
      await selection;
      await flush();
      expect(c.selectedId, 'other');
      expect(c.transcript.single.text, 'other');
    },
  );

  test(
    'deleted saved session clears its failed restore without a stale error',
    () async {
      final api = StartupClient();
      final history = Completer<HistoryPage>();
      api.histories['saved'] = history.future;
      final c = DesktopController(
        MemoryPreferences()..sessionId = 'saved',
        clientFactory: (_) => api,
      );
      addTearDown(() async {
        api.complete();
        await flush();
        c.dispose();
      });
      await c.connect('http://127.0.0.1');
      await flush();
      api.complete(deleted: true);
      history.completeError(
        DshException('not_found', 'Session no longer exists'),
      );
      await flush();
      expect(c.selectedId, isNull);
      expect(c.preferences.sessionId, isNull);
      expect(c.transcript, isEmpty);
      expect(c.error, isNull);
    },
  );
}
