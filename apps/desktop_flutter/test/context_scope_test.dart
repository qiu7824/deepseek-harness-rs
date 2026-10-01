import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/conversation/context_quick_settings.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

class ContextController extends DesktopController {
  ContextController() : super(MemoryPreferences()) {
    host = HostInfo.fromJson({
      'home': 'test',
      'version': 'test',
      'cwd': 'test',
    });
    selectedId = 'session-a';
    catalog = contextCatalog('model-a');
  }

  DshClient? activeClient = FakeClient();
  @override
  DshClient? get client => activeClient;
  bool modelPending = false;
  @override
  bool get changingModel => modelPending;
  int revision = 0;
  @override
  int get selectionRevision => revision;
  final thresholdWrites = <(String, double?)>[];
  final compactions = <String?>[];
  Completer<void>? writing;

  @override
  Future<void> setCompactionThreshold(String key, double? ratio) async {
    thresholdWrites.add((key, ratio));
    await (writing?.future ?? Future<void>.value());
    final thresholds = {...object(contextCompaction['thresholds'])};
    if (ratio == null) {
      thresholds.remove(key);
    } else {
      thresholds[key] = ratio;
    }
    contextCompaction = {...contextCompaction, 'thresholds': thresholds};
    emit();
  }

  @override
  Future<void> compactNow() async {
    compactions.add(selectedId);
    await (writing?.future ?? Future<void>.value());
  }
}

ModelCatalog contextCatalog(String model) => ModelCatalog.fromJson({
  'current': {'provider': 'provider', 'model': model},
  'routable': true,
  'groups': [],
});

void main() {
  Future<void> mount(
    WidgetTester tester,
    ContextController c, {
    bool showUsage = true,
  }) async {
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SizedBox(
            width: 440,
            child: ContextQuickSettings(controller: c, showUsage: showUsage),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  Slider slider(WidgetTester tester) =>
      tester.widget<Slider>(find.byKey(const Key('compaction-threshold')));

  void preview(Slider control, double value) {
    control.onChangeStart!(control.value);
    control.onChanged!(value);
  }

  Future<void> cleanup(WidgetTester tester, ContextController c) async {
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await c.activeClient?.close();
  }

  testWidgets('embedded controls can omit duplicated usage and keep actions', (
    tester,
  ) async {
    final c = ContextController();
    c.contextCompaction = {
      'thresholds': {'provider/model-a': .7},
    };
    c.projectionWindow.apply('contextPressure', {
      'projectedTokens': 32000,
      'contextWindow': 128000,
    }, 1);
    await mount(tester, c);
    expect(find.text('25% · 3.2万 / 12.8万'), findsOneWidget);
    expect(find.byType(LinearProgressIndicator), findsOneWidget);
    await mount(tester, c, showUsage: false);
    expect(find.text('上下文'), findsNothing);
    expect(find.text('25% · 3.2万 / 12.8万'), findsNothing);
    expect(find.byType(LinearProgressIndicator), findsNothing);
    expect(slider(tester).value, 70);
    expect(find.byKey(const Key('compaction-threshold-reset')), findsOneWidget);
    expect(find.byKey(const Key('compact-now')), findsOneWidget);
    await tester.tap(find.byKey(const Key('compaction-threshold-reset')));
    await tester.pumpAndSettle();
    expect(c.thresholdWrites, [('provider/model-a', null)]);
    await tester.tap(find.byKey(const Key('compact-now')));
    await tester.pumpAndSettle();
    expect(c.compactions, ['session-a']);
    await cleanup(tester, c);
  });

  testWidgets('threshold keeps its pending target visible until confirmation', (
    tester,
  ) async {
    final c = ContextController()..writing = Completer<void>();
    await mount(tester, c);
    final control = slider(tester);
    preview(control, 65);
    control.onChangeEnd!(65);
    await tester.pump();
    expect(c.thresholdWrites, [('provider/model-a', .65)]);
    expect(slider(tester).value, 65);
    expect(slider(tester).onChanged, isNull);
    expect(find.text('保存中…'), findsOneWidget);
    c.writing!.complete();
    await tester.pumpAndSettle();
    expect(slider(tester).value, 65);
    expect(slider(tester).onChanged, isNotNull);
    expect(find.text('保存中…'), findsNothing);
    await cleanup(tester, c);
  });

  testWidgets('failed threshold save restores the confirmed value', (
    tester,
  ) async {
    final c = ContextController()..writing = Completer<void>();
    c.contextCompaction = {
      'thresholds': {'provider/model-a': .7},
    };
    await mount(tester, c);
    final control = slider(tester);
    preview(control, 55);
    control.onChangeEnd!(55);
    await tester.pump();
    expect(slider(tester).value, 55);
    c.writing!.completeError(StateError('save rejected'));
    await tester.pumpAndSettle();
    expect(slider(tester).value, 70);
    expect(find.textContaining('save rejected'), findsOneWidget);
    expect(object(c.contextCompaction['thresholds'])['provider/model-a'], .7);
    await cleanup(tester, c);
  });

  for (final change in ['model', 'session', 'revision', 'host', 'client']) {
    testWidgets('$change change retires the drag and its captured callbacks', (
      tester,
    ) async {
      final c = ContextController();
      await mount(tester, c);
      final control = slider(tester);
      preview(control, 60);
      await tester.pump();
      expect(slider(tester).value, 60);
      final previousClient = c.activeClient;
      switch (change) {
        case 'model':
          c.catalog = contextCatalog('model-b');
        case 'session':
          c.selectedId = 'session-b';
        case 'revision':
          c.revision++;
        case 'host':
          c.host = HostInfo.fromJson({
            'home': 'other',
            'version': 'test',
            'cwd': 'test',
          });
        case 'client':
          c.activeClient = FakeClient();
      }
      c.emit();
      await tester.pump();
      expect(slider(tester).value, 80);
      control.onChangeEnd!(60);
      await tester.pump();
      expect(c.thresholdWrites, isEmpty);
      // Even reselecting the former model cannot revive its retired gesture.
      if (change == 'model') {
        c.catalog = contextCatalog('model-a');
        c.emit();
        await tester.pump();
        preview(control, 55);
        control.onChangeEnd!(55);
        await tester.pump();
        expect(c.thresholdWrites, isEmpty);
        expect(slider(tester).value, 80);
      }
      await cleanup(tester, c);
      if (change == 'client') await previousClient?.close();
    });
  }

  for (final changing in ['model', 'sending']) {
    testWidgets(
      '$changing pending cancels drags and blocks all context writes',
      (tester) async {
        final c = ContextController();
        c.contextCompaction = {
          'thresholds': {'provider/model-a': .7},
        };
        await mount(tester, c);
        final control = slider(tester);
        final reset = tester.widget<DshButton>(
          find.byKey(const Key('compaction-threshold-reset')),
        );
        final compact = tester.widget<DshButton>(
          find.byKey(const Key('compact-now')),
        );
        preview(control, 55);
        if (changing == 'model') {
          c.modelPending = true;
        } else {
          c.sending = true;
        }
        c.emit();
        await tester.pump();
        expect(slider(tester).value, 70);
        expect(slider(tester).onChanged, isNull);
        expect(
          tester
              .widget<DshButton>(find.byKey(const Key('compact-now')))
              .onPressed,
          isNull,
        );
        control.onChangeEnd!(55);
        reset.onPressed!();
        compact.onPressed!();
        await tester.pump();
        expect(c.thresholdWrites, isEmpty);
        expect(c.compactions, isEmpty);
        c.modelPending = false;
        c.sending = false;
        c.emit();
        await tester.pump();
        control.onChangeEnd!(55);
        reset.onPressed!();
        compact.onPressed!();
        await tester.pump();
        expect(c.thresholdWrites, isEmpty);
        expect(c.compactions, isEmpty);
        expect(slider(tester).onChanged, isNotNull);
        await cleanup(tester, c);
      },
    );
  }

  testWidgets('saved thresholds at the Host range bounds stay accurate', (
    tester,
  ) async {
    final c = ContextController();
    for (final ratio in [.30, .98]) {
      c.contextCompaction = {
        'thresholds': {'provider/model-a': ratio},
      };
      await mount(tester, c, showUsage: false);
      await tester.pumpAndSettle();
      final control = slider(tester);
      expect(control.min, 30);
      expect(control.max, 98);
      expect(control.value, ratio * 100);
      expect(find.text('${(ratio * 100).round()}%'), findsOneWidget);
    }
    await cleanup(tester, c);
  });

  testWidgets(
    'old session reset and compact callbacks cannot target a new one',
    (tester) async {
      final c = ContextController();
      c.contextCompaction = {
        'thresholds': {'provider/model-a': .7},
      };
      await mount(tester, c);
      final reset = tester.widget<DshButton>(
        find.byKey(const Key('compaction-threshold-reset')),
      );
      final compact = tester.widget<DshButton>(
        find.byKey(const Key('compact-now')),
      );
      c.selectedId = 'session-b';
      c.emit();
      await tester.pump();
      reset.onPressed!();
      compact.onPressed!();
      await tester.pump();
      expect(c.thresholdWrites, isEmpty);
      expect(c.compactions, isEmpty);
      await cleanup(tester, c);
    },
  );

  testWidgets('returning to the old model does not revive reset or compact', (
    tester,
  ) async {
    final c = ContextController();
    c.contextCompaction = {
      'thresholds': {'provider/model-a': .7},
    };
    await mount(tester, c);
    final reset = tester.widget<DshButton>(
      find.byKey(const Key('compaction-threshold-reset')),
    );
    final compact = tester.widget<DshButton>(
      find.byKey(const Key('compact-now')),
    );
    c.catalog = contextCatalog('model-b');
    c.emit();
    await tester.pump();
    c.catalog = contextCatalog('model-a');
    c.emit();
    await tester.pump();
    reset.onPressed!();
    compact.onPressed!();
    await tester.pump();
    expect(c.thresholdWrites, isEmpty);
    expect(c.compactions, isEmpty);
    await cleanup(tester, c);
  });

  testWidgets(
    'late context failure is hidden after model changes and returns',
    (tester) async {
      final c = ContextController()..writing = Completer<void>();
      await mount(tester, c);
      final control = slider(tester);
      preview(control, 60);
      control.onChangeEnd!(60);
      await tester.pump();
      c.catalog = contextCatalog('model-b');
      c.emit();
      await tester.pump();
      expect(slider(tester).value, 80);
      expect(slider(tester).onChanged, isNull);
      c.catalog = contextCatalog('model-a');
      c.emit();
      await tester.pump();
      c.writing!.completeError(StateError('old model rejected'));
      await tester.pumpAndSettle();
      expect(find.textContaining('old model rejected'), findsNothing);
      expect(slider(tester).value, 80);
      expect(slider(tester).onChanged, isNotNull);
      await cleanup(tester, c);
    },
  );

  testWidgets(
    'replacing the controller retires callbacks from the previous one',
    (tester) async {
      final before = ContextController(), after = ContextController();
      await mount(tester, before);
      final control = slider(tester);
      preview(control, 60);
      await tester.pump();
      await mount(tester, after);
      control.onChangeEnd!(60);
      await tester.pump();
      expect(before.thresholdWrites, isEmpty);
      expect(after.thresholdWrites, isEmpty);
      expect(slider(tester).value, 80);
      await cleanup(tester, after);
      before.dispose();
      await before.activeClient?.close();
    },
  );
}
