import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/model_selector.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

ModelCatalog _catalog({Json? current, int count = 2, bool reasoning = true}) =>
    ModelCatalog.fromJson({
      'current':
          current ??
          {'provider': 'p', 'model': 'm0', 'reasoningEffort': 'high'},
      'routable': true,
      'groups': [
        {
          'id': 'p',
          'name': 'Provider Alpha',
          'models': [
            for (var i = 0; i < count; i++)
              {
                'id': 'm$i',
                'name': 'Model $i',
                if (reasoning)
                  'reasoning': {
                    'defaultEffort': 'high',
                    'efforts': [
                      {'id': 'low', 'name': 'Low'},
                      {'id': 'medium', 'name': 'Medium'},
                      {'id': 'high', 'name': 'High'},
                    ],
                  },
              },
          ],
        },
      ],
    });

class _Controller extends DesktopController {
  _Controller() : super(MemoryPreferences()) {
    catalog = _catalog();
    selectedId = 's';
  }
  bool online = true;
  Json? requested;
  Completer<void>? reading, selecting;
  Object? readFailure, selectionFailure;
  int reads = 0, creates = 0;
  String? defaultFailure;
  int retries = 0;
  bool defaultPending = false;
  final submitted = <String>[];
  @override
  bool get connected => online;
  @override
  Json? get pendingModelSelection => requested;
  @override
  bool get changingModel => requested != null;
  @override
  String? get modelDefaultError => defaultFailure;
  @override
  bool get savingModelDefault => defaultPending;
  @override
  Future<void> retryModelDefault() async {
    retries++;
    defaultFailure = null;
    emit();
  }

  @override
  Future<void> refreshModels() async {
    reads++;
    await reading?.future;
    if (readFailure != null) throw readFailure!;
    catalog ??= _catalog();
    emit();
  }

  @override
  Future<void> chooseModel(ModelChoice model) async {
    submitted.add(model.id);
    requested = {
      'provider': model.provider,
      'model': model.id,
      'reasoningEffort': model.defaultReasoningEffort,
    };
    emit();
    await selecting?.future;
    if (selectionFailure != null) {
      requested = null;
      emit();
      throw selectionFailure!;
    }
    catalog = ModelCatalog.withCurrent(catalog!, requested!);
    requested = null;
    emit();
  }

  @override
  Future<void> setReasoning(String id) async {
    submitted.add(id);
    requested = {...catalog!.current, 'reasoningEffort': id};
    emit();
    await selecting?.future;
    catalog = ModelCatalog.withCurrent(catalog!, requested!);
    requested = null;
    emit();
  }
}

void main() {
  late _Controller controller;
  setUp(() {
    controller = _Controller();
  });
  tearDown(() {
    controller.dispose();
  });

  Future<void> show(
    WidgetTester tester, {
    double scale = 1,
    double width = 600,
    VoidCallback? onReturnFocus,
    VoidCallback? onManage,
  }) async {
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: MediaQuery(
            data: MediaQueryData(
              size: const Size(800, 600),
              textScaler: TextScaler.linear(scale),
            ),
            child: Align(
              alignment: Alignment.bottomRight,
              child: SizedBox(
                width: width,
                child: ModelSelectionControls(
                  controller: controller,
                  onReturnFocus: onReturnFocus,
                  onManage: onManage,
                ),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  Future<void> open(WidgetTester tester) async {
    await tester.tap(find.byKey(const ValueKey('model-picker-trigger')));
    await tester.pumpAndSettle();
  }

  test(
    'model and reasoning captions preserve provider defaults and custom names',
    () {
      final catalog = _catalog(current: {'provider': 'p', 'model': 'm0'});
      expect(composerModelLabel(catalog), 'Model 0');
      expect(composerReasoningLabel(catalog), '模型默认（高）');
      expect(hasReasoning(catalog), isTrue);
      expect(hasReasoning(_catalog(reasoning: false)), isFalse);
      expect(composerModelLabel(null), '选择模型');
    },
  );

  testWidgets(
    'cached catalog opens immediately without reload or session creation',
    (tester) async {
      controller.reading = Completer<void>();
      await show(tester);
      await open(tester);
      expect(find.byKey(const ValueKey('model-picker')), findsOneWidget);
      expect(
        find.byKey(const ValueKey('model-choice-p\u0000m1')),
        findsOneWidget,
      );
      expect(controller.reads, 0);
      expect(controller.creates, 0);
      expect(find.byType(Dialog), findsNothing);
      expect(find.byKey(const ValueKey('reasoning-level-high')), findsNothing);
      controller.reading!.complete();
    },
  );

  testWidgets(
    'uncached catalog opens a local loader before asynchronous data arrives',
    (tester) async {
      controller.catalog = null;
      controller.reading = Completer<void>();
      await show(tester);
      await open(tester);
      expect(
        find.byKey(const ValueKey('model-catalog-loading')),
        findsOneWidget,
      );
      expect(controller.reads, 1);
      expect(controller.creates, 0);
      controller.reading!.complete();
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('model-choice-p\u0000m1')),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('refresh failure retains the catalog and supports retry', (
    tester,
  ) async {
    await show(tester);
    await open(tester);
    controller.readFailure = StateError('directory unavailable');
    await tester.tap(find.byTooltip('刷新模型列表'));
    await tester.pumpAndSettle();
    expect(find.textContaining('directory unavailable'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('model-choice-p\u0000m1')),
      findsOneWidget,
    );
    controller.readFailure = null;
    await tester.tap(find.byKey(const ValueKey('model-catalog-retry')));
    await tester.pumpAndSettle();
    expect(controller.reads, 2);
    expect(find.byKey(const ValueKey('model-catalog-error')), findsNothing);
  });

  testWidgets('choosing a model dismisses and restores focus before receipt', (
    tester,
  ) async {
    controller.selecting = Completer<void>();
    var restored = 0;
    await show(tester, onReturnFocus: () => restored++);
    await open(tester);
    await tester.tap(find.byKey(const ValueKey('model-choice-p\u0000m1')));
    await tester.pump();
    expect(find.byKey(const ValueKey('model-picker')), findsNothing);
    expect(controller.submitted, ['m1']);
    expect(controller.catalog!.current['model'], 'm0');
    expect(restored, 1);
    expect(find.text('Model 1'), findsOneWidget);
    controller.selecting!.complete();
    await tester.pumpAndSettle();
    expect(controller.catalog!.current['model'], 'm1');
  });

  testWidgets(
    'failed closed selection remains visible through controller error',
    (tester) async {
      controller.selecting = Completer<void>();
      controller.selectionFailure = StateError('selection rejected');
      await show(tester);
      await open(tester);
      await tester.tap(find.byKey(const ValueKey('model-choice-p\u0000m1')));
      await tester.pump();
      controller.selecting!.complete();
      await tester.pumpAndSettle();
      expect(controller.error, contains('selection rejected'));
      expect(controller.catalog!.current['model'], 'm0');
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'reasoning opens separately with declared default and no model list',
    (tester) async {
      controller.catalog = _catalog(current: {'provider': 'p', 'model': 'm0'});
      await show(tester);
      expect(find.text('思考：模型默认（高）'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('reasoning-picker-trigger')));
      await tester.pumpAndSettle();
      expect(find.text('默认等级：高'), findsOneWidget);
      expect(find.text('模型默认'), findsOneWidget);
      expect(find.byKey(const ValueKey('reasoning-level-low')), findsOneWidget);
      expect(find.byKey(const ValueKey('model-picker-search')), findsNothing);
    },
  );

  testWidgets('unsupported reasoning hides its toolbar entry', (tester) async {
    controller.catalog = _catalog(reasoning: false);
    await show(tester);
    expect(
      find.byKey(const ValueKey('reasoning-picker-trigger')),
      findsNothing,
    );
    expect(find.byKey(const ValueKey('model-picker-trigger')), findsOneWidget);
  });

  testWidgets('Escape closes and new session scope retires the old popover', (
    tester,
  ) async {
    var restored = 0;
    await show(tester, onReturnFocus: () => restored++);
    await open(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('model-picker')), findsNothing);
    expect(restored, 1);
    await open(tester);
    controller.selectedId = 'other';
    controller.emit();
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('model-picker')), findsNothing);
    expect(restored, 1);
  });

  testWidgets(
    'search matches provider and arrow plus Enter chooses the highlighted model',
    (tester) async {
      await show(tester);
      await open(tester);
      await tester.enterText(
        find.byKey(const ValueKey('model-picker-search')),
        'provider alpha',
      );
      await tester.pump();
      expect(
        find.byKey(const ValueKey('model-choice-p\u0000m1')),
        findsOneWidget,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(controller.submitted, ['m1']);
      expect(find.byKey(const ValueKey('model-picker')), findsNothing);
    },
  );

  testWidgets('long directories build visible rows only', (tester) async {
    controller.catalog = _catalog(count: 200);
    await show(tester);
    await open(tester);
    final built = find.byType(ListTile).evaluate().length;
    expect(built, greaterThan(0));
    expect(built, lessThan(200));
  });

  testWidgets(
    'account changes retire both popovers on the same Host and session',
    (tester) async {
      controller.subscriptionAccounts = [
        {'id': 'p', 'signedIn': true, 'activeAccountId': 'first'},
      ];
      controller.catalog = _catalog();
      await show(tester);
      await open(tester);
      final client = controller.client;
      final host = controller.host;
      final session = controller.selectedId;
      controller.subscriptionAccounts = [
        {'id': 'p', 'signedIn': true, 'activeAccountId': 'second'},
      ];
      controller.emit();
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('model-picker')), findsNothing);
      controller.catalog = _catalog();
      controller.emit();
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('reasoning-picker-trigger')));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('reasoning-level-low')), findsOneWidget);
      controller.subscriptionAccounts = [
        {'id': 'p', 'signedIn': true, 'activeAccountId': 'third'},
      ];
      controller.emit();
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('reasoning-level-low')), findsNothing);
      expect(controller.client, same(client));
      expect(controller.host, same(host));
      expect(controller.selectedId, session);
      expect(controller.submitted, isEmpty);
      expect(controller.reads, 0);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('default save failures are secondary and can be retried', (
    tester,
  ) async {
    controller.defaultFailure = '已应用，但默认设置尚未保存';
    await show(tester);
    await open(tester);
    expect(find.byKey(const ValueKey('model-default-error')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('model-default-retry')));
    await tester.pumpAndSettle();
    expect(controller.retries, 1);
    expect(find.byKey(const ValueKey('model-default-error')), findsNothing);
    expect(
      find.byKey(const ValueKey('model-choice-p\u0000m1')),
      findsOneWidget,
    );
  });

  testWidgets('narrow 200 percent layout stays bounded and scrollable', (
    tester,
  ) async {
    await show(tester, scale: 2, width: 280, onManage: () {});
    await open(tester);
    final panel = tester.getSize(find.byKey(const ValueKey('model-picker')));
    expect(panel.width, lessThanOrEqualTo(360));
    expect(panel.height, lessThanOrEqualTo(480));
    expect(find.byKey(const ValueKey('model-picker-list')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
