import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

ModelCatalog startupCatalog({String effort = 'high'}) => ModelCatalog.fromJson({
  'current': {
    'provider': 'p',
    'model': 'a',
    'reasoningEffort': effort,
    'executionMode': 'standard',
  },
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
              {'id': 'high', 'name': 'High'},
            ],
          },
        },
      ],
    },
  ],
});

Json startupAccounts(String id) => {
  'providers': [
    {
      'id': 'p',
      'signedIn': true,
      'accounts': [
        {'accountId': id, 'accountScope': 'scope-$id', 'active': true},
      ],
    },
  ],
};

class StartupCatalogClient extends FakeClient {
  StartupCatalogClient() {
    liveSessions = [
      SessionSummary.fromJson({'sessionId': 's', 'cwd': 'D:/fixture'}),
    ];
  }
  final firstModels = Completer<ModelCatalog>();
  final firstAccounts = Completer<Json>();
  Json accounts = startupAccounts('one');
  int modelReads = 0, accountReads = 0;
  bool failFreshRead = false;

  @override
  Future<ModelCatalog> models(String id) {
    modelReads++;
    if (modelReads == 1) return firstModels.future;
    if (failFreshRead) throw StateError('metadata unavailable');
    return Future.value(startupCatalog());
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) {
    if (path == '/provider-auth/providers') {
      accountReads++;
      return accountReads == 1 ? firstAccounts.future : Future.value(accounts);
    }
    return super.request(
      path,
      body: body,
      scope: scope,
      mutation: mutation,
      maxBytes: maxBytes,
    );
  }
}

void main() {
  late StartupCatalogClient api;
  late DesktopController c;
  setUp(() async {
    api = StartupCatalogClient();
    c = DesktopController(
      MemoryPreferences()..sessionId = 's',
      clientFactory: (_) => api,
    );
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
  });
  tearDown(() async {
    if (!api.firstModels.isCompleted) {
      api.firstModels.complete(startupCatalog());
    }
    if (!api.firstAccounts.isCompleted) {
      api.firstAccounts.complete(api.accounts);
    }
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });

  test('restored session recovers its catalog when startup authorization retires the first read', () async {
    expect(c.selectedId, 's');
    expect(api.modelReads, 1);
    api.firstAccounts.complete(api.accounts);
    await Future<void>.delayed(Duration.zero);
    api.firstModels.complete(startupCatalog(effort: 'low'));
    await Future<void>.delayed(Duration.zero);
    expect(api.modelReads, 2);
    expect(c.availableModelCatalog, isNotNull);
    expect(c.modelPreviewCatalog!.current['reasoningEffort'], 'high');
    expect(c.modelPreviewCatalog!.choices.single.reasoning, hasLength(2));
  });

  test(
    'metadata completion shares an initial read already in its valid scope',
    () async {
      api.accounts = {'providers': <Json>[]};
      api.firstAccounts.complete(api.accounts);
      await Future<void>.delayed(Duration.zero);
      expect(api.modelReads, 1);
      api.firstModels.complete(startupCatalog());
      await Future<void>.delayed(Duration.zero);
      await c.loadAccounts();
      await c.loadCatalogs();
      expect(api.modelReads, 1);
      expect(c.availableModelCatalog, isNotNull);
    },
  );

  test('a failed metadata recovery does not poll and still permits explicit refresh', () async {
    api.failFreshRead = true;
    api.firstAccounts.complete(api.accounts);
    await Future<void>.delayed(Duration.zero);
    api.firstModels.complete(startupCatalog(effort: 'low'));
    await Future<void>.delayed(Duration.zero);
    expect(api.modelReads, 2);
    expect(c.availableModelCatalog, isNull);
    await c.loadAccounts();
    await c.loadCatalogs();
    await c.loadCatalogs();
    expect(api.modelReads, 2);
    api.failFreshRead = false;
    await c.refreshModels();
    expect(api.modelReads, 3);
    expect(c.availableModelCatalog, isNotNull);
  });

  test('returning to a previous account recovers once for the effective transition', () async {
    api.firstAccounts.complete(api.accounts);
    await Future<void>.delayed(Duration.zero);
    api.firstModels.complete(startupCatalog(effort: 'low'));
    await Future<void>.delayed(Duration.zero);
    expect(api.modelReads, 2);
    api.accounts = startupAccounts('two');
    await c.loadAccounts();
    await Future<void>.delayed(Duration.zero);
    expect(api.modelReads, 3);
    api.accounts = startupAccounts('one');
    await c.loadAccounts();
    await Future<void>.delayed(Duration.zero);
    expect(api.modelReads, 4);
    expect(c.availableModelCatalog, isNotNull);
  });

  test('metadata completion does not warm or create a session after switching to the Hero', () async {
    c.newConversation();
    api.firstAccounts.complete(api.accounts);
    await Future<void>.delayed(Duration.zero);
    api.firstModels.complete(startupCatalog());
    await Future<void>.delayed(Duration.zero);
    await c.loadCatalogs();
    expect(c.selectedId, isNull);
    expect(api.modelReads, 1);
    expect(c.availableModelCatalog, isNull);
  });
}
