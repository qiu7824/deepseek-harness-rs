import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

ModelCatalog catalogFor(Json current) => ModelCatalog.fromJson({
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
            'efforts': [
              {'id': 'off', 'name': 'Off'},
              {'id': 'low', 'name': 'Low'},
            ],
          },
        },
      ],
    },
  ],
});

class ModelClient extends FakeClient {
  Json selected = {
    'provider': 'p',
    'model': 'a',
    'reasoningEffort': 'high',
    'executionMode': 'standard',
  };
  final selections = <Json>[];
  final defaults = <Json>[];
  Completer<void>? mutation;
  Future<ModelCatalog> Function(String)? read;
  Object? readFailure;
  bool failDefault = false;

  @override
  Future<ModelCatalog> models(String id) async {
    if (read != null) return read!(id);
    if (readFailure != null) throw readFailure!;
    return catalogFor({...selected});
  }

  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    if (method == 'session.selectModel') {
      selections.add({...payload});
      await this.mutation?.future;
      selected = {
        'provider': payload['provider'],
        'model': payload['model'],
        'executionMode': payload['executionMode'] ?? 'standard',
        'reasoningEffort':
            payload['reasoningEffort'] ??
            (payload['model'] == 'b' ? 'low' : 'high'),
      };
      return {
        'selected': {...selected},
      };
    }
    if (method == 'settings.replace' &&
        payload['ns'] == 'agent-default-model') {
      defaults.add(object(payload['section']));
      if (failDefault) throw StateError('default unavailable');
      return {};
    }
    return super.call(method, payload, mutation);
  }
}

class DelayedDefaultsClient extends ModelClient {
  final firstWriteStarted = Completer<void>();
  final firstWriteReply = Completer<void>();
  Json savedDefault = {};
  int defaultCount = 0;
  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    if (method == 'settings.replace' &&
        payload['ns'] == 'agent-default-model') {
      defaultCount++;
      if (defaultCount == 1) {
        firstWriteStarted.complete();
        await firstWriteReply.future;
      }
      savedDefault = {...object(payload['section'])};
      return {};
    }
    return super.call(method, payload, mutation);
  }
}

void main() {
  late ModelClient api;
  late DesktopController c;
  setUp(() async {
    api = ModelClient();
    c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    await c.select('s');
  });
  tearDown(() async {
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });

  test(
    'reasoning uses the current model and only its declared levels',
    () async {
      await c.setReasoning('high');
      expect(api.selections, isEmpty);
      await expectLater(c.setReasoning('max'), throwsStateError);
      expect(api.selections, isEmpty);
      await c.setReasoning('medium');
      expect(api.selections.single, {
        'sessionId': 's',
        'provider': 'p',
        'model': 'a',
        'reasoningEffort': 'medium',
        'executionMode': 'standard',
      });
      expect(c.catalog!.current['reasoningEffort'], 'medium');
      expect(api.defaults.single['reasoningEffort'], 'medium');
    },
  );

  test(
    'one model change blocks stale effort submission and chat admission',
    () async {
      api.mutation = Completer<void>();
      final changing = c.chooseModel(c.catalog!.choices.last);
      expect(c.changingModel, isTrue);
      expect(c.catalog!.current['model'], 'a');
      await expectLater(c.setReasoning('low'), throwsStateError);
      expect(await c.sendParts('retained draft', []), isNull);
      expect(api.selections, hasLength(1));
      api.mutation!.complete();
      await changing;
      expect(c.changingModel, isFalse);
      expect(c.catalog!.current['model'], 'b');
      expect(c.catalog!.current['reasoningEffort'], 'low');
      expect(api.defaults.single['reasoningEffort'], 'low');
      await c.chooseModel(c.catalog!.choices.last);
      expect(
        api.selections,
        hasLength(1),
        reason: 'reselecting must retain effort',
      );
    },
  );

  test(
    'late mutation never refreshes or persists after another selection',
    () async {
      api.mutation = Completer<void>();
      final changing = c.chooseModel(c.catalog!.choices.last);
      await c.select('other');
      final other = c.catalog;
      expect(c.changingModel, isFalse);
      api.mutation!.complete();
      await changing;
      expect(identical(c.catalog, other), isTrue);
      expect(c.selectedId, 'other');
      expect(api.defaults, isEmpty);
    },
  );

  test(
    'late model read cannot restore a previous selection of the same id',
    () async {
      final reading = Completer<ModelCatalog>();
      api.read = (_) => reading.future;
      final changing = c.chooseModel(c.catalog!.choices.last);
      await Future<void>.delayed(Duration.zero);
      c.newConversation();
      api.read = null;
      await c.select('s');
      final current = c.catalog;
      reading.complete(
        catalogFor({
          'provider': 'p',
          'model': 'a',
          'reasoningEffort': 'medium',
        }),
      );
      await changing;
      expect(identical(c.catalog, current), isTrue);
      expect(c.catalog!.current['model'], 'b');
      expect(api.defaults, isEmpty);
    },
  );

  test('advisory refresh cannot overwrite a later model choice', () async {
    final reading = Completer<ModelCatalog>();
    api.read = (_) => reading.future;
    final refreshing = c.refreshModels();
    api.read = null;
    await c.chooseModel(c.catalog!.choices.last);
    reading.complete(
      catalogFor({'provider': 'p', 'model': 'a', 'reasoningEffort': 'high'}),
    );
    await refreshing;
    expect(c.catalog!.current['model'], 'b');
  });

  test(
    'confirmed selection survives refresh failure and reports partial success',
    () async {
      api.readFailure = StateError('catalog unavailable');
      await expectLater(
        c.chooseModel(c.catalog!.choices.last),
        throwsA(predicate((error) => '$error'.contains('已更新'))),
      );
      expect(c.catalog!.current['model'], 'b');
      expect(c.catalog!.current['reasoningEffort'], 'low');
      expect(api.defaults.single['model'], 'b');
      expect(c.changingModel, isFalse);
    },
  );

  test('default write failure does not roll back accepted effort', () async {
    api.failDefault = true;
    await c.setReasoning('low');
    expect(c.catalog!.current['reasoningEffort'], 'low');
    expect(c.error, contains('默认'));
  });

  test(
    'new defaults wait for an accepted old write and finish in selection order',
    () async {
      final slow = DelayedDefaultsClient();
      final owner = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => slow,
      );
      await owner.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      await owner.select('s');
      final first = owner.chooseModel(owner.catalog!.choices.last);
      await slow.firstWriteStarted.future;
      await owner.select('other');
      final second = owner.chooseModel(owner.catalog!.choices.first);
      await Future<void>.delayed(Duration.zero);
      expect(owner.catalog!.current['model'], 'a');
      expect(
        slow.defaultCount,
        1,
        reason: 'new default must queue behind accepted old write',
      );
      slow.firstWriteReply.complete();
      await Future.wait([first, second]);
      expect(owner.catalog!.current['model'], 'a');
      expect(slow.savedDefault['model'], 'a');
      expect(slow.defaultCount, 2);
      owner.dispose();
    },
  );

  test(
    'queued default from a retired session is skipped before it starts',
    () async {
      final slow = DelayedDefaultsClient();
      final owner = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => slow,
      );
      await owner.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      await owner.select('s');
      final first = owner.chooseModel(owner.catalog!.choices.last);
      await slow.firstWriteStarted.future;
      await owner.select('other');
      final second = owner.chooseModel(owner.catalog!.choices.first);
      await Future<void>.delayed(Duration.zero);
      await owner.select('latest');
      final latest = owner.chooseModel(owner.catalog!.choices.last);
      await Future<void>.delayed(Duration.zero);
      slow.firstWriteReply.complete();
      await Future.wait([first, second, latest]);
      expect(
        slow.defaultCount,
        2,
        reason: 'the retired queued A write never starts',
      );
      expect(slow.savedDefault['model'], 'b');
      expect(owner.catalog!.current['model'], 'b');
      owner.dispose();
    },
  );

  test(
    'message preparation admits no competing model or effort change',
    () async {
      final commands = Completer<List<Json>>();
      api.commandListResult = commands.future;
      api.handleCall = (method, payload) async =>
          method == 'session.prompt' ? {'accepted': true} : {};
      final sending = c.sendParts('/not-a-command some text', []);
      expect(c.sending, isTrue);
      await expectLater(
        c.chooseModel(c.catalog!.choices.last),
        throwsStateError,
      );
      await expectLater(c.setReasoning('low'), throwsStateError);
      expect(api.selections, isEmpty);
      commands.complete([]);
      expect(await sending, 's');
      expect(c.catalog!.current['model'], 'a');
    },
  );

  test(
    'initial catalog response cannot overwrite a later confirmed selection',
    () async {
      c.newConversation();
      final initial = Completer<ModelCatalog>();
      final old = catalogFor({...api.selected});
      var reads = 0;
      api.read = (_) async =>
          ++reads == 1 ? initial.future : catalogFor({...api.selected});
      final selecting = c.select('s');
      await Future<void>.delayed(Duration.zero);
      await c.refreshModels();
      await c.chooseModel(c.catalog!.choices.last);
      initial.complete(old);
      await selecting;
      expect(c.catalog!.current['model'], 'b');
    },
  );

  Future<void> openPicker(WidgetTester tester, {double scale = 1}) async {
    await tester.pumpWidget(
      ShadApp(
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(context)
              .copyWith(textScaler: TextScaler.linear(scale)),
          child: child!,
        ),
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () => showDialog<void>(
                context: context,
                builder: (_) => ModelPicker(controller: c),
              ),
              child: const Text('Open'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
  }

  testWidgets(
    'model choice keeps the picker, refreshes levels, and retains search focus',
    (tester) async {
      await openPicker(tester);
      await tester.enterText(
        find.byKey(const ValueKey('model-picker-search')),
        ' Model ',
      );
      await tester.pump();
      final search = tester.widget<EditableText>(
        find.descendant(
          of: find.byKey(const ValueKey('model-picker-search')),
          matching: find.byType(EditableText),
        ),
      );
      expect(search.focusNode.hasFocus, isTrue);
      await tester.tap(find.byKey(const ValueKey('model-choice-p\u0000b')));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('model-picker')), findsOneWidget);
      expect(find.byKey(const ValueKey('reasoning-level-off')), findsOneWidget);
      expect(find.byKey(const ValueKey('reasoning-level-high')), findsNothing);
      expect(search.controller.text, ' Model ');
      expect(search.focusNode.hasFocus, isTrue);
      expect(
        find.byKey(const ValueKey('model-selection-notice')),
        findsOneWidget,
      );
      await tester.tap(find.byKey(const ValueKey('reasoning-level-off')));
      await tester.pumpAndSettle();
      expect(c.catalog!.current['reasoningEffort'], 'off');
      expect(find.byKey(const ValueKey('model-picker')), findsOneWidget);
    },
  );

  testWidgets('picker closes safely after selection clears its catalog', (
    tester,
  ) async {
    await openPicker(tester);
    c.newConversation();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('model-picker')), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'pending and failed default persistence are visible inside the picker',
    (tester) async {
      await openPicker(tester);
      api.mutation = Completer<void>();
      api.failDefault = true;
      await tester.tap(find.byKey(const ValueKey('reasoning-level-low')));
      await tester.pump();
      expect(
        find.byKey(const ValueKey('model-selection-pending')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('model-selection-notice')),
        findsNothing,
      );
      expect(c.catalog!.current['reasoningEffort'], 'high');
      api.mutation!.complete();
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('model-selection-pending')),
        findsNothing,
      );
      expect(find.textContaining('默认'), findsWidgets);
      expect(c.catalog!.current['reasoningEffort'], 'low');
      expect(find.byKey(const ValueKey('model-picker')), findsOneWidget);
    },
  );

  testWidgets(
    'reasoning remains reachable without overflow at 200 percent scale',
    (tester) async {
      await openPicker(tester, scale: 2);
      expect(
        find.byKey(const ValueKey('reasoning-level-high')),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
      await tester.tap(find.byKey(const ValueKey('model-picker-done')));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('model-picker')), findsNothing);
    },
  );
}
