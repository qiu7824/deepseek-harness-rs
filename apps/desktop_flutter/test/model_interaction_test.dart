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
      await Future<void>.delayed(Duration.zero);
      expect(api.defaults.single['reasoningEffort'], 'medium');
    },
  );

  test(
    'pending model choice admits its own effort and blocks chat admission',
    () async {
      api.mutation = Completer<void>();
      final changing = c.chooseModel(c.catalog!.choices.last);
      expect(c.changingModel, isTrue);
      expect(c.catalog!.current['model'], 'a');
      expect(c.modelPreviewCatalog!.current['model'], 'b');
      await expectLater(c.setReasoning('high'), throwsStateError);
      await Future<void>.delayed(Duration.zero);
      expect(await c.sendParts('retained draft', []), isNull);
      expect(api.selections, hasLength(1));
      api.mutation!.complete();
      await changing;
      expect(c.changingModel, isFalse);
      expect(c.catalog!.current['model'], 'b');
      expect(c.catalog!.current['reasoningEffort'], 'low');
      await Future<void>.delayed(Duration.zero);
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
    'late catalog read cannot restore a previous selection of the same id',
    () async {
      final reading = Completer<ModelCatalog>();
      api.read = (_) => reading.future;
      final readingModels = c.refreshModels();
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
      await readingModels;
      expect(identical(c.catalog, current), isTrue);
      expect(c.catalog!.current['model'], 'a');
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
    'confirmed selection does not depend on a full catalog refresh',
    () async {
      api.readFailure = StateError('catalog unavailable');
      await c.chooseModel(c.catalog!.choices.last);
      expect(c.catalog!.current['model'], 'b');
      expect(c.catalog!.current['reasoningEffort'], 'low');
      await Future<void>.delayed(Duration.zero);
      expect(api.defaults.single['model'], 'b');
      expect(c.changingModel, isFalse);
    },
  );

  test('default write failure does not roll back accepted effort', () async {
    api.failDefault = true;
    await c.setReasoning('low');
    expect(c.catalog!.current['reasoningEffort'], 'low');
    await Future<void>.delayed(Duration.zero);
    expect(c.modelDefaultError, contains('默认'));
    expect(c.error, isNull);
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
      await Future<void>.delayed(Duration.zero);
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
      await Future<void>.delayed(Duration.zero);
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
    'initial catalog response cannot leak into a different session',
    () async {
      c.newConversation();
      final initial = Completer<ModelCatalog>();
      final old = catalogFor({...api.selected});
      var reads = 0;
      api.read = (_) async =>
          ++reads == 1 ? initial.future : catalogFor({...api.selected});
      final selecting = c.select('s');
      await Future<void>.delayed(Duration.zero);
      await c.select('other');
      final other = c.catalog;
      initial.complete(old);
      await selecting;
      expect(identical(c.catalog, other), isTrue);
      expect(c.selectedId, 'other');
    },
  );

  Future<void> openPicker(
    WidgetTester tester, {
    double scale = 1,
    double width = 600,
    bool openModels = true,
  }) async {
    final focus = FocusNode();
    addTearDown(focus.dispose);
    await tester.pumpWidget(
      ShadApp(
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(context)
              .copyWith(textScaler: TextScaler.linear(scale)),
          child: child!,
        ),
        home: Scaffold(
          body: Column(
            children: [
              TextField(key: const Key('integration-prompt'), focusNode: focus),
              const Spacer(),
              Align(
                alignment: Alignment.bottomRight,
                child: SizedBox(
                  width: width,
                  child: ModelSelectionControls(
                    controller: c,
                    onReturnFocus: focus.requestFocus,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
    await tester.pump();
    if (openModels) {
      await tester.tap(find.byKey(const ValueKey('model-picker-trigger')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('model-menu-model-row')));
      await tester.pumpAndSettle();
    }
  }

  Future<void> reopenModels(WidgetTester tester) async {
    await tester.tap(find.byKey(const ValueKey('model-picker-trigger')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('model-menu-model-row')));
    await tester.pumpAndSettle();
  }

  Future<void> openReasoning(WidgetTester tester) async {
    await tester.tap(find.byKey(const ValueKey('model-picker-trigger')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('reasoning-picker-trigger')));
    await tester.pumpAndSettle();
  }

  testWidgets(
    'cached menu opens immediately and model selection returns to input before receipt',
    (tester) async {
      final reading = Completer<ModelCatalog>();
      var reads = 0;
      api.read = (_) {
        reads++;
        return reading.future;
      };
      api.mutation = Completer<void>();
      await openPicker(tester);
      expect(reads, 0);
      expect(find.byKey(const ValueKey('model-picker')), findsOneWidget);
      expect(find.byType(Dialog), findsNothing);
      expect(find.byKey(const Key('context-quick-settings')), findsNothing);
      expect(find.byKey(const ValueKey('reasoning-level-high')), findsNothing);
      final model = tester.getRect(
        find.byKey(const ValueKey('model-trigger-name')),
      );
      final reasoning = tester.getRect(
        find.byKey(const ValueKey('model-trigger-reasoning')),
      );
      expect(model.center.dy, closeTo(reasoning.center.dy, 1));
      expect(reasoning.left - model.right, inInclusiveRange(0, 12));
      await tester.enterText(
        find.byKey(const ValueKey('model-picker-search')),
        'Model B',
      );
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('model-choice-p\u0000b')));
      await tester.pump();
      expect(find.byKey(const ValueKey('model-picker')), findsNothing);
      expect(c.catalog!.current['model'], 'a');
      expect(c.pendingModelSelection?['model'], 'b');
      final input = tester.widget<EditableText>(
        find.descendant(
          of: find.byKey(const Key('integration-prompt')),
          matching: find.byType(EditableText),
        ),
      );
      expect(input.focusNode.hasFocus, isTrue);
      api.mutation!.complete();
      await tester.pumpAndSettle();
      expect(c.catalog!.current['model'], 'b');
      expect(c.catalog!.current['reasoningEffort'], 'low');
      expect(reads, 0);
      expect(
        tester
            .widget<Text>(find.byKey(const ValueKey('model-trigger-reasoning')))
            .data,
        '低',
      );
      reading.complete(catalogFor({...api.selected}));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'reasoning submenu dismisses before receipt and retains the model',
    (tester) async {
      await openPicker(tester, openModels: false);
      await openReasoning(tester);
      expect(find.byKey(const ValueKey('model-picker')), findsNothing);
      expect(find.byKey(const ValueKey('model-picker-search')), findsNothing);
      expect(find.byKey(const Key('context-quick-settings')), findsNothing);
      expect(find.byKey(const ValueKey('reasoning-level-low')), findsOneWidget);
      api.mutation = Completer<void>();
      await tester.tap(find.byKey(const ValueKey('reasoning-level-low')));
      await tester.pump();
      expect(find.byKey(const ValueKey('reasoning-menu')), findsNothing);
      expect(find.byKey(const ValueKey('model-selection-menu')), findsNothing);
      final input = tester.widget<EditableText>(
        find.descendant(
          of: find.byKey(const Key('integration-prompt')),
          matching: find.byType(EditableText),
        ),
      );
      expect(input.focusNode.hasFocus, isTrue);
      expect(c.catalog!.current['model'], 'a');
      expect(c.catalog!.current['reasoningEffort'], 'high');
      expect(c.pendingModelSelection?['reasoningEffort'], 'low');
      api.mutation!.complete();
      await tester.pumpAndSettle();
      expect(c.catalog!.current['model'], 'a');
      expect(c.catalog!.current['reasoningEffort'], 'low');
      expect(api.selections.single['model'], 'a');
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'pending and default-save failure remain secondary to model choices',
    (tester) async {
      api.mutation = Completer<void>();
      api.failDefault = true;
      await openPicker(tester);
      await tester.tap(find.byKey(const ValueKey('model-choice-p\u0000b')));
      await tester.pump();
      await reopenModels(tester);
      expect(
        find.byKey(const ValueKey('model-pending-p\u0000b')),
        findsOneWidget,
      );
      expect(c.catalog!.current['model'], 'a');
      expect(
        tester
            .widget<ListTile>(
              find.byKey(const ValueKey('model-choice-p\u0000a')),
            )
            .onTap,
        isNotNull,
      );
      api.mutation!.complete();
      await tester.pumpAndSettle();
      expect(c.catalog!.current['model'], 'b');
      expect(find.byKey(const ValueKey('model-default-error')), findsOneWidget);
      expect(c.changingModel, isFalse);
      await tester.tap(find.byKey(const ValueKey('model-choice-p\u0000a')));
      await tester.pumpAndSettle();
      expect(c.catalog!.current['model'], 'a');
      expect(api.selections, hasLength(2));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('session and Host changes retire open model menus', (
    tester,
  ) async {
    await openPicker(tester);
    c.newConversation();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('model-picker')), findsNothing);
    await c.select('other');
    await tester.pumpAndSettle();
    await reopenModels(tester);
    expect(find.byKey(const ValueKey('model-picker')), findsOneWidget);
    c.host = HostInfo.fromJson({
      'home': 'replacement',
      'version': 'test',
      'cwd': 'test',
    });
    c.emit();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('model-picker')), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'nested choices remain reachable without overflow at 200 percent',
    (tester) async {
      await openPicker(tester, scale: 2, width: 280);
      expect(find.byKey(const ValueKey('model-picker-done')), findsNothing);
      expect(find.byType(Dialog), findsNothing);
      final option = find.byKey(const ValueKey('model-choice-p\u0000b'));
      await tester.ensureVisible(option);
      await tester.pumpAndSettle();
      expect(option.hitTestable(), findsOneWidget);
      await tester.tap(option);
      await tester.pumpAndSettle();
      await openReasoning(tester);
      final effort = find.byKey(const ValueKey('reasoning-level-off'));
      await tester.ensureVisible(effort);
      await tester.pumpAndSettle();
      expect(effort.hitTestable(), findsOneWidget);
      await tester.tap(effort);
      await tester.pumpAndSettle();
      expect(c.catalog!.current['reasoningEffort'], 'off');
      expect(tester.takeException(), isNull);
    },
  );
}
