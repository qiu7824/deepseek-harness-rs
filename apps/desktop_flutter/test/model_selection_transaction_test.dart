import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

Json transactionCatalogValue(Json current) => {
  'current': current,
  'routable': true,
  'groups': [
    {
      'id': 'p',
      'name': 'Provider',
      'models': [
        {
          'id': 'a',
          'name': 'Model A',
          'reasoning': {
            'defaultEffort': 'high',
            'efforts': [
              {'id': 'low', 'name': 'Low'},
              {'id': 'medium', 'name': 'Medium'},
              {'id': 'high', 'name': 'High'},
            ],
          },
        },
        {
          'id': 'b',
          'name': 'Model B',
          'reasoning': {
            'defaultEffort': 'low',
            'efforts': [
              {'id': 'off', 'name': 'Off'},
              {'id': 'low', 'name': 'Low'},
            ],
          },
        },
      ],
    },
  ],
};

ModelCatalog transactionCatalog(Json current) =>
    ModelCatalog.fromJson(transactionCatalogValue(current));

class TransactionClient extends FakeClient {
  Json selected = {
    'provider': 'p',
    'model': 'a',
    'reasoningEffort': 'high',
    'executionMode': 'standard',
  };
  final selections = <Json>[];
  final defaults = <Json>[];
  final thresholdWrites = <Json>[];
  final mutationReplies = <int, Completer<void>>{};
  Completer<void>? defaultReply;
  Completer<void>? thresholdReply;
  Completer<void>? createReply;
  Future<ModelCatalog> Function(String)? read;
  Object? failure;
  bool omitSelected = false, failDefault = false, commitBeforeFailure = false;
  final thresholdFailures = <int>{};
  int modelReads = 0, directoryReads = 0, sessionCreates = 0;

  @override
  Future<ModelCatalog> models(String id) async {
    modelReads++;
    return read == null ? transactionCatalog({...selected}) : read!(id);
  }

  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    if (method == 'session.create') {
      sessionCreates++;
      await createReply?.future;
      return {'sessionId': 'created'};
    }
    if (method == 'llm.models') {
      directoryReads++;
      return transactionCatalogValue(const {});
    }
    if (method == 'session.selectModel') {
      final index = selections.length;
      selections.add({...payload});
      await mutationReplies[index]?.future;
      if (failure != null && !commitBeforeFailure) throw failure!;
      selected = {
        'provider': payload['provider'],
        'model': payload['model'],
        'reasoningEffort':
            payload['reasoningEffort'] ??
            (payload['model'] == 'b' ? 'low' : 'high'),
        'executionMode': payload['executionMode'] ?? 'standard',
      };
      if (failure != null) throw failure!;
      return {
        if (!omitSelected) 'selected': {...selected},
      };
    }
    if (method == 'settings.replace' &&
        payload['ns'] == 'agent-default-model') {
      await defaultReply?.future;
      if (failDefault) throw StateError('default unavailable');
      defaults.add({...object(payload['section'])});
      return {};
    }
    if (method == 'session.prompt') return {'accepted': true};
    if (method == 'settings.replace' && payload['ns'] == 'context-compaction') {
      final index = thresholdWrites.length;
      thresholdWrites.add({...object(payload['section'])});
      if (index == 0) await thresholdReply?.future;
      if (thresholdFailures.contains(index)) {
        throw StateError('threshold unavailable');
      }
      return {};
    }
    return super.call(method, payload, mutation);
  }
}

void main() {
  late TransactionClient api;
  late DesktopController c;
  setUp(() async {
    api = TransactionClient();
    c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    await c.select('s');
  });
  tearDown(() async {
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });

  test('accepted selection does not reload the full model catalog', () async {
    final reads = api.modelReads;
    await c.chooseModel(c.catalog!.choices.last);
    expect(c.catalog!.current['model'], 'b');
    expect(c.catalog!.current['reasoningEffort'], 'low');
    expect(api.modelReads, reads);
    expect(c.changingModel, isFalse);
    await Future<void>.delayed(Duration.zero);
    expect(api.defaults.single['model'], 'b');
  });

  test(
    'slow default persistence does not delay selection or message admission',
    () async {
      api.defaultReply = Completer<void>();
      await c.setReasoning('low');
      expect(c.changingModel, isFalse);
      expect(c.catalog!.current['reasoningEffort'], 'low');
      expect(api.defaults, isEmpty);
      expect(await c.sendParts('ordinary text', []), 's');
      api.defaultReply!.complete();
      await Future<void>.delayed(Duration.zero);
      expect(api.defaults.single['reasoningEffort'], 'low');
    },
  );

  test('rapid effort changes preview immediately and submit only the latest queued intent', () async {
    final firstReply = api.mutationReplies[0] = Completer<void>();
    final first = c.setReasoning('low');
    await Future<void>.delayed(Duration.zero);
    expect(api.selections, hasLength(1));
    expect(c.catalog!.current['reasoningEffort'], 'high');
    expect(c.modelPreviewCatalog!.current['reasoningEffort'], 'low');
    final second = c.setReasoning('medium');
    final latest = c.setReasoning('high');
    expect(c.modelPreviewCatalog!.current['reasoningEffort'], 'high');
    expect(c.changingModel, isTrue);
    expect(await c.sendParts('retained draft', []), isNull);
    firstReply.complete();
    await Future.wait([first, second, latest]);
    expect(api.selections.map((value) => value['reasoningEffort']), [
      'low',
      'high',
    ]);
    expect(c.catalog!.current['reasoningEffort'], 'high');
    expect(c.pendingModelSelection, isNull);
    expect(c.changingModel, isFalse);
    await Future<void>.delayed(Duration.zero);
    expect(api.defaults.single['reasoningEffort'], 'high');
  });

  test(
    'effort queued during model switch belongs to the preview model',
    () async {
      final firstReply = api.mutationReplies[0] = Completer<void>();
      final first = c.chooseModel(c.catalog!.choices.last);
      await Future<void>.delayed(Duration.zero);
      expect(c.modelPreviewCatalog!.current['model'], 'b');
      expect(c.modelPreviewCatalog!.current['reasoningEffort'], 'low');
      await expectLater(c.setReasoning('high'), throwsStateError);
      final latest = c.setReasoning('off');
      expect(c.modelPreviewCatalog!.current['reasoningEffort'], 'off');
      firstReply.complete();
      await Future.wait([first, latest]);
      expect(api.selections.last['model'], 'b');
      expect(api.selections.last['reasoningEffort'], 'off');
      expect(c.catalog!.current['model'], 'b');
      expect(c.catalog!.current['reasoningEffort'], 'off');
    },
  );

  test(
    'rejected selection restores confirmed effort and does not save defaults',
    () async {
      api.failure = DshException('model-unavailable', 'Unavailable');
      await expectLater(c.setReasoning('low'), throwsA(isA<DshException>()));
      expect(c.catalog!.current['reasoningEffort'], 'high');
      expect(c.modelPreviewCatalog!.current['reasoningEffort'], 'high');
      expect(c.pendingModelSelection, isNull);
      expect(api.defaults, isEmpty);
      expect(c.changingModel, isFalse);
    },
  );

  test(
    'same-session reselection waits behind an accepted earlier mutation',
    () async {
      final firstReply = api.mutationReplies[0] = Completer<void>();
      final first = c.setReasoning('low');
      await Future<void>.delayed(Duration.zero);
      c.newConversation();
      final selecting = c.select('s');
      final model = c.chooseModel(c.availableModelCatalog!.choices.first);
      final latest = c.setReasoning('medium');
      await Future<void>.delayed(Duration.zero);
      expect(api.selections, hasLength(1));
      firstReply.complete();
      await Future.wait([first, selecting, model, latest]);
      expect(api.selections.last['reasoningEffort'], 'medium');
      expect(api.selected['reasoningEffort'], 'medium');
      expect(c.catalog!.current['reasoningEffort'], 'medium');
    },
  );

  test(
    'legacy reply without selected still awaits an authoritative read',
    () async {
      api.omitSelected = true;
      final reading = Completer<ModelCatalog>();
      api.read = (_) => reading.future;
      final changing = c.chooseModel(c.catalog!.choices.last);
      await Future<void>.delayed(Duration.zero);
      expect(c.changingModel, isTrue);
      expect(c.catalog!.current['model'], 'a');
      reading.complete(transactionCatalog({...api.selected}));
      await changing;
      expect(c.catalog!.current['model'], 'b');
      expect(c.changingModel, isFalse);
    },
  );

  test('concurrent catalog refresh requests share one read', () async {
    final reads = api.modelReads;
    final reading = Completer<ModelCatalog>();
    api.read = (_) => reading.future;
    final first = c.refreshModels();
    final second = c.refreshModels();
    expect(api.modelReads, reads + 1);
    expect(c.refreshingModels, isTrue);
    reading.complete(transactionCatalog({...api.selected}));
    await Future.wait([first, second]);
    expect(c.refreshingModels, isFalse);
  });

  test('initial catalog read and an explicit refresh are coalesced', () async {
    c.newConversation();
    final reads = api.modelReads;
    final reading = Completer<ModelCatalog>();
    api.read = (_) => reading.future;
    final selecting = c.select('s');
    final refreshing = c.refreshModels();
    expect(api.modelReads, reads + 1);
    reading.complete(transactionCatalog({...api.selected}));
    await Future.wait([selecting, refreshing]);
    expect(c.catalog!.current['model'], 'a');
  });

  test(
    'background default failure reports separately without reverting selection',
    () async {
      api.failDefault = true;
      await c.setReasoning('low');
      await Future<void>.delayed(Duration.zero);
      expect(c.changingModel, isFalse);
      expect(c.catalog!.current['reasoningEffort'], 'low');
      expect(c.modelDefaultError, contains('默认'));
      expect(c.error, isNull);
    },
  );

  test(
    'failed background default can be retried without locking the selector',
    () async {
      api.failDefault = true;
      await c.setReasoning('low');
      await Future<void>.delayed(Duration.zero);
      expect(c.modelDefaultError, isNotNull);
      api.failDefault = false;
      api.defaultReply = Completer<void>();
      final retrying = c.retryModelDefault();
      expect(c.modelDefaultError, isNull);
      expect(c.savingModelDefault, isTrue);
      expect(c.changingModel, isFalse);
      api.defaultReply!.complete();
      await retrying;
      expect(c.savingModelDefault, isFalse);
      expect(api.defaults.single['reasoningEffort'], 'low');
    },
  );

  test('an unknown mutation outcome reconciles before allowing send', () async {
    api.failure = DshException('network', 'reply lost', outcomeUnknown: true);
    api.commitBeforeFailure = true;
    final reads = api.modelReads;
    await c.setReasoning('low');
    expect(api.modelReads, reads + 1);
    expect(c.catalog!.current['reasoningEffort'], 'low');
    expect(c.modelSelectionUnconfirmed, isFalse);
    expect(await c.sendParts('ordinary text', []), 's');
  });

  test(
    'an unknown outcome with a failed read blocks send until refresh',
    () async {
      api.failure = DshException('network', 'reply lost', outcomeUnknown: true);
      api.read = (_) async => throw StateError('read unavailable');
      await expectLater(c.setReasoning('low'), throwsA(isA<DshException>()));
      expect(c.changingModel, isFalse);
      expect(c.modelSelectionUnconfirmed, isTrue);
      expect(await c.sendParts('retained draft', []), isNull);
      api.read = null;
      await c.refreshModels();
      expect(c.modelSelectionUnconfirmed, isFalse);
      expect(c.catalog!.current['reasoningEffort'], 'high');
      expect(await c.sendParts('ordinary text', []), 's');
    },
  );

  test(
    'late model receipt cannot overwrite another Host on the same client',
    () async {
      final reply = api.mutationReplies[0] = Completer<void>();
      final changing = c.setReasoning('low');
      await Future<void>.delayed(Duration.zero);
      c.host = HostInfo.fromJson({
        'home': 'another',
        'version': 'test',
        'cwd': 'test',
      });
      final replacement = transactionCatalog({
        'provider': 'p',
        'model': 'b',
        'reasoningEffort': 'off',
      });
      c.catalog = replacement;
      reply.complete();
      await changing;
      expect(identical(c.catalog, replacement), isTrue);
      expect(api.defaults, isEmpty);
    },
  );

  test(
    'serialized compaction edits preserve confirmed keys across models',
    () async {
      api.thresholdReply = Completer<void>();
      final first = c.setCompactionThreshold('p/a', 0.5);
      await Future<void>.delayed(Duration.zero);
      final second = c.setCompactionThreshold('p/b', 0.7);
      await Future<void>.delayed(Duration.zero);
      expect(api.thresholdWrites, hasLength(1));
      api.thresholdReply!.complete();
      await Future.wait([first, second]);
      expect(api.thresholdWrites.last['thresholds'], {'p/a': 0.5, 'p/b': 0.7});
      expect(c.contextCompaction['thresholds'], {'p/a': 0.5, 'p/b': 0.7});
    },
  );

  test(
    'a rejected compaction edit is not carried into the next section write',
    () async {
      api.thresholdReply = Completer<void>();
      api.thresholdFailures.add(0);
      final first = c.setCompactionThreshold('p/a', 0.5);
      final rejected = expectLater(first, throwsStateError);
      final second = c.setCompactionThreshold('p/b', 0.7);
      await Future<void>.delayed(Duration.zero);
      api.thresholdReply!.complete();
      await Future.wait([rejected, second]);
      expect(api.thresholdWrites.last['thresholds'], {'p/b': 0.7});
      expect(c.contextCompaction['thresholds'], {'p/b': 0.7});
    },
  );

  test(
    'late compaction write cannot overwrite settings after a Host change',
    () async {
      api.thresholdReply = Completer<void>();
      final writing = c.setCompactionThreshold('p/a', 0.5);
      await Future<void>.delayed(Duration.zero);
      c.host = HostInfo.fromJson({
        'home': 'another',
        'version': 'test',
        'cwd': 'test',
      });
      c.contextCompaction = {
        'thresholds': {'p/b': 0.7},
      };
      api.thresholdReply!.complete();
      await writing;
      expect(c.contextCompaction['thresholds'], {'p/b': 0.7});
    },
  );

  test('cached directory is immediately available without claiming the previous session model', () async {
    c.newConversation();
    expect(c.catalog, isNull);
    expect(c.availableModelCatalog!.choices, hasLength(2));
    expect(c.availableModelCatalog!.current, isEmpty);
    expect(c.availableModelCatalog!.routable, isFalse);
    expect(c.modelPreviewCatalog!.current, isEmpty);
    expect(c.selectedId, isNull);
    await c.refreshModels();
    expect(api.directoryReads, 1);
    expect(c.selectedId, isNull);
    expect(c.catalog, isNull);
    expect(c.availableModelCatalog!.current, isEmpty);
    expect(api.selections, isEmpty);
    expect(api.defaults, isEmpty);
  });

  test('a Host reconnect discards its previous directory cache', () async {
    c.newConversation();
    expect(c.availableModelCatalog, isNotNull);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    expect(c.availableModelCatalog, isNull);
    await c.refreshModels();
    expect(c.availableModelCatalog!.choices, hasLength(2));
    expect(c.availableModelCatalog!.current, isEmpty);
  });

  test('explicit creation selection can reuse directory metadata without a preliminary session read', () async {
    final reads = api.modelReads;
    await c.select('other', loadModels: false);
    expect(c.catalog, isNull);
    expect(c.availableModelCatalog!.current, isEmpty);
    await c.chooseModel(c.availableModelCatalog!.choices.last);
    expect(api.modelReads, reads);
    expect(c.catalog!.current['model'], 'b');
    expect(c.catalog!.choices.last.defaultReasoningEffort, 'low');
  });

  test(
    'a Hero choice holds send admission throughout session creation',
    () async {
      c.workspaces = [
        {'workspaceId': 'w', 'path': 'D:/fixture'},
      ];
      c.targetWorkspace('w');
      c.newConversation();
      c.setDraft('retained draft');
      api.createReply = Completer<void>();
      final reads = api.modelReads;
      final changing = c.chooseModel(c.availableModelCatalog!.choices.last);
      expect(c.changingModel, isTrue);
      expect(c.pendingModelSelection!['model'], 'b');
      expect(await c.sendParts('retained draft', []), isNull);
      expect(api.sessionCreates, 1);
      expect(c.selectedId, isNull);
      api.createReply!.complete();
      await changing;
      expect(api.modelReads, reads);
      expect(c.selectedId, 'created');
      expect(c.catalog!.current['model'], 'b');
      expect(c.draft, 'retained draft');
      expect(c.changingModel, isFalse);
    },
  );

  test('rapid Hero model and effort choices share one creation and keep the final intent', () async {
    c.workspaces = [
      {'workspaceId': 'w', 'path': 'D:/fixture'},
    ];
    c.targetWorkspace('w');
    c.newConversation();
    api.createReply = Completer<void>();
    final first = c.chooseModel(c.availableModelCatalog!.choices.last);
    final latest = c.setReasoning('off');
    expect(c.modelPreviewCatalog!.current['reasoningEffort'], 'off');
    expect(api.sessionCreates, 1);
    api.createReply!.complete();
    await Future.wait([first, latest]);
    expect(api.selections, hasLength(1));
    expect(api.selections.single['model'], 'b');
    expect(api.selections.single['reasoningEffort'], 'off');
    expect(c.catalog!.current['reasoningEffort'], 'off');
  });

  test('changing the workspace retires a pending Hero choice and keeps the new draft', () async {
    c.workspaces = [
      {'workspaceId': 'w', 'path': 'D:/fixture'},
    ];
    c.targetWorkspace('w');
    c.newConversation();
    api.createReply = Completer<void>();
    final changing = c.chooseModel(c.availableModelCatalog!.choices.last);
    c.targetWorkspace('other');
    c.setDraft('new workspace draft');
    expect(c.changingModel, isFalse);
    api.createReply!.complete();
    await changing;
    expect(c.selectedId, isNull);
    expect(c.draft, 'new workspace draft');
    expect(api.selections, isEmpty);
    expect(api.defaults, isEmpty);
  });

  test(
    'a rejected Hero mutation reports the rejection after creation',
    () async {
      c.workspaces = [
        {'workspaceId': 'w', 'path': 'D:/fixture'},
      ];
      c.targetWorkspace('w');
      c.newConversation();
      api.failure = DshException('model-unavailable', 'Unavailable');
      await expectLater(
        c.chooseModel(c.availableModelCatalog!.choices.last),
        throwsA(isA<DshException>()),
      );
      expect(c.changingModel, isFalse);
      expect(c.selectedId, 'created');
      expect(api.defaults, isEmpty);
    },
  );

  test('a legacy receipt cannot falsely confirm a model that the read did not select', () async {
    api.omitSelected = true;
    final old = transactionCatalog({...api.selected});
    api.read = (_) async => old;
    await expectLater(c.chooseModel(c.catalog!.choices.last), throwsStateError);
    expect(c.catalog!.current['model'], 'a');
    expect(c.modelSelectionUnconfirmed, isFalse);
    expect(api.defaults, isEmpty);
  });

  test(
    'account changes invalidate both visible metadata and cached directories',
    () async {
      c.subscriptionAccounts = [
        {
          'id': 'p',
          'signedIn': true,
          'accounts': [
            {'accountId': 'a1', 'accountScope': 'scope-1', 'active': true},
            {'accountId': 'a2', 'accountScope': 'scope-2', 'active': false},
          ],
        },
      ];
      await c.refreshModels();
      final oldScope = c.modelCatalogScope;
      expect(c.availableModelCatalog, isNotNull);
      final accounts = c.subscriptionAccounts.single['accounts'] as List<Json>;
      accounts[0]['active'] = false;
      accounts[1]['active'] = true;
      expect(c.modelCatalogScope, isNot(oldScope));
      expect(c.availableModelCatalog, isNull);
      c.newConversation();
      expect(c.availableModelCatalog, isNull);
      await c.refreshModels();
      expect(c.availableModelCatalog, isNotNull);
      expect(c.availableModelCatalog!.current, isEmpty);
    },
  );

  test('provider and account row order does not invalidate the same authorization identity', () async {
    final accounts = <Json>[
      {'accountId': 'a1', 'accountScope': 'scope-1', 'active': true},
      {'accountId': 'a2', 'accountScope': 'scope-2', 'active': false},
    ];
    c.subscriptionAccounts = [
      {'id': 'p', 'signedIn': true, 'accounts': accounts},
      {'id': 'q', 'signedIn': false, 'accounts': <Json>[]},
    ];
    await c.refreshModels();
    final original = c.modelCatalogScope;
    c.subscriptionAccounts = [
      {'id': 'q', 'signedIn': false, 'accounts': <Json>[]},
      {'id': 'p', 'signedIn': true, 'accounts': accounts.reversed.toList()},
    ];
    expect(c.modelCatalogScope, original);
    expect(c.availableModelCatalog, isNotNull);
  });

  test(
    'a late catalog response cannot overwrite a new active account catalog',
    () async {
      c.subscriptionAccounts = [
        {
          'id': 'p',
          'signedIn': true,
          'accounts': [
            {'accountId': 'a1', 'accountScope': 'scope-1', 'active': true},
          ],
        },
      ];
      await c.refreshModels();
      final oldRead = Completer<ModelCatalog>();
      api.read = (_) => oldRead.future;
      final first = c.refreshModels();
      c.subscriptionAccounts = [
        {
          'id': 'p',
          'signedIn': true,
          'accounts': [
            {'accountId': 'a2', 'accountScope': 'scope-2', 'active': true},
          ],
        },
      ];
      final newest = transactionCatalog({
        'provider': 'p',
        'model': 'b',
        'reasoningEffort': 'off',
      });
      api.read = (_) async => newest;
      await c.refreshModels();
      oldRead.complete(
        transactionCatalog({
          'provider': 'p',
          'model': 'a',
          'reasoningEffort': 'high',
        }),
      );
      await first;
      expect(identical(c.catalog, newest), isTrue);
      expect(c.availableModelCatalog!.current['model'], 'b');
    },
  );

  test('configuration refresh waits for a retired accepted mutation before reading current', () async {
    final firstReply = api.mutationReplies[0] = Completer<void>();
    final changing = c.setReasoning('low');
    await Future<void>.delayed(Duration.zero);
    final reads = api.modelReads;
    final scope = c.modelCatalogScope;
    c.invalidateModelCatalog();
    expect(c.modelCatalogScope, isNot(scope));
    expect(c.availableModelCatalog, isNull);
    expect(c.changingModel, isFalse);
    final refreshing = c.refreshModels();
    expect(api.modelReads, reads);
    firstReply.complete();
    await Future.wait([changing, refreshing]);
    expect(api.modelReads, reads + 1);
    expect(c.catalog!.current['reasoningEffort'], 'low');
    expect(api.defaults, isEmpty);
  });
}
