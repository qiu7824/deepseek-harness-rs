import 'dart:async';
import 'dart:ui';

import 'package:dsh_desktop/features/conversation/reasoning_slider.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

const _levels = [
  {'id': 'low', 'name': 'Low', 'description': '减少思考时间'},
  {'id': 'medium', 'name': 'Medium'},
  {'id': 'high', 'name': 'High'},
  {'id': 'max', 'name': 'Max'},
];

Widget _host(Widget child, {double width = 360, double scale = 1}) => ShadApp(
  home: Scaffold(
    body: MediaQuery(
      data: MediaQueryData(textScaler: TextScaler.linear(scale)),
      child: SizedBox(width: width, child: child),
    ),
  ),
);

Finder _level(String id) => find.byKey(ValueKey('reasoning-level-$id'));

void main() {
  test('standard labels are localized and provider names stay as declared', () {
    expect(reasoningLevelLabel({'id': 'high', 'name': 'High'}), '高');
    expect(reasoningLevelLabel({'id': 'xhigh', 'name': 'Extra high'}), '极高');
    expect(reasoningLevelLabel({'id': 'custom', 'name': 'Turbo'}), 'Turbo');
    expect(
      reasoningLevelLabel({'id': 'high', 'name': 'Deep think'}),
      'Deep think',
    );
  });

  testWidgets('levels commit on click without a slider or reselect request', (
    tester,
  ) async {
    final commits = <String>[];
    var value = 'high';
    await tester.pumpWidget(
      _host(
        StatefulBuilder(
          builder: (context, setState) => ReasoningSlider(
            levels: _levels,
            value: value,
            onChanged: (id) async {
              commits.add(id);
              setState(() => value = id);
            },
          ),
        ),
      ),
    );
    expect(find.byType(Slider), findsNothing);
    expect(find.text('推理强度'), findsOneWidget);
    await tester.tap(_level('low'));
    await tester.pumpAndSettle();
    expect(commits, ['low']);
    expect(
      find.byKey(const ValueKey('reasoning-selected-low')),
      findsOneWidget,
    );
    await tester.tap(_level('low'));
    await tester.pumpAndSettle();
    expect(commits, ['low']);
  });

  testWidgets(
    'model default and unknown values do not select the first level',
    (tester) async {
      Future<void> show(String? value) => tester.pumpWidget(
        _host(
          ReasoningSlider(
            levels: _levels,
            value: value,
            showHeading: false,
            onChanged: (_) async {},
          ),
        ),
      );
      await show(null);
      expect(find.text('模型默认'), findsOneWidget);
      expect(
        find.byKey(const ValueKey('reasoning-selected-low')),
        findsNothing,
      );
      expect(_level('low'), findsOneWidget);
      await show('custom');
      expect(find.text('custom（未列出）'), findsOneWidget);
      expect(
        find.byKey(const ValueKey('reasoning-selected-low')),
        findsNothing,
      );
    },
  );

  testWidgets('models without levels explain that reasoning is unavailable', (
    tester,
  ) async {
    await tester.pumpWidget(
      _host(
        ReasoningSlider(
          levels: const [],
          value: null,
          showHeading: false,
          onChanged: (_) async {},
        ),
      ),
    );
    expect(find.text('当前模型未提供可调思考等级'), findsOneWidget);
    expect(find.byType(InkWell), findsNothing);
  });

  testWidgets(
    'pending choices stay distinct and the latest click stays usable',
    (tester) async {
      final requests = <String, Completer<void>>{};
      await tester.pumpWidget(
        _host(
          ReasoningSlider(
            levels: _levels,
            value: 'high',
            onChanged: (id) => (requests[id] = Completer<void>()).future,
          ),
        ),
      );
      await tester.tap(_level('low'));
      await tester.pump();
      expect(find.text('正在切换：低'), findsOneWidget);
      expect(
        find.byKey(const ValueKey('reasoning-selected-high')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('reasoning-selected-low')),
        findsNothing,
      );
      await tester.tap(_level('medium'));
      await tester.pump();
      expect(requests.keys, ['low', 'medium']);
      expect(find.text('正在切换：中'), findsOneWidget);
      requests['low']!.completeError(StateError('old request failed'));
      await tester.pump();
      expect(find.textContaining('old request failed'), findsNothing);
      expect(find.text('正在切换：中'), findsOneWidget);
      requests['medium']!.complete();
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('reasoning-pending-value')),
        findsNothing,
      );
      expect(
        find.byKey(const ValueKey('reasoning-selected-high')),
        findsOneWidget,
      );
    },
  );

  testWidgets('clicking the confirmed level can supersede a pending intent', (
    tester,
  ) async {
    final requests = <Completer<void>>[];
    final commits = <String>[];
    await tester.pumpWidget(
      _host(
        ReasoningSlider(
          levels: _levels,
          value: 'high',
          onChanged: (id) {
            commits.add(id);
            final result = Completer<void>();
            requests.add(result);
            return result.future;
          },
        ),
      ),
    );
    await tester.tap(_level('low'));
    await tester.pump();
    await tester.tap(_level('high'));
    await tester.pump();
    expect(commits, ['low', 'high']);
    for (final request in requests) {
      request.complete();
    }
    await tester.pumpAndSettle();
  });

  testWidgets('controller pending value remains visible until confirmation', (
    tester,
  ) async {
    var value = 'high';
    String? pending = 'low';
    late StateSetter update;
    await tester.pumpWidget(
      _host(
        StatefulBuilder(
          builder: (context, setState) {
            update = setState;
            return ReasoningSlider(
              levels: _levels,
              value: value,
              pendingValue: pending,
              onChanged: (_) async {},
            );
          },
        ),
      ),
    );
    expect(find.text('正在切换：低'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('reasoning-selected-high')),
      findsOneWidget,
    );
    update(() {
      value = 'low';
      pending = null;
    });
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('reasoning-pending-value')), findsNothing);
    expect(
      find.byKey(const ValueKey('reasoning-selected-low')),
      findsOneWidget,
    );
  });

  testWidgets(
    'late failures cannot leak into a new scope with the same levels',
    (tester) async {
      final request = Completer<void>();
      var scope = 'first-host';
      late StateSetter update;
      await tester.pumpWidget(
        _host(
          StatefulBuilder(
            builder: (context, setState) {
              update = setState;
              return ReasoningSlider(
                scope: scope,
                levels: _levels,
                value: 'high',
                onChanged: (_) => request.future,
              );
            },
          ),
        ),
      );
      await tester.tap(_level('low'));
      await tester.pump();
      update(() => scope = 'second-host');
      await tester.pump();
      request.completeError(StateError('wrong Host'));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('reasoning-update-error')),
        findsNothing,
      );
      expect(
        find.byKey(const ValueKey('reasoning-pending-value')),
        findsNothing,
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('a late failure does not contradict a new confirmed value', (
    tester,
  ) async {
    final request = Completer<void>();
    var value = 'high';
    late StateSetter update;
    await tester.pumpWidget(
      _host(
        StatefulBuilder(
          builder: (context, setState) {
            update = setState;
            return ReasoningSlider(
              levels: _levels,
              value: value,
              onChanged: (_) => request.future,
            );
          },
        ),
      ),
    );
    await tester.tap(_level('low'));
    await tester.pump();
    update(() => value = 'low');
    await tester.pump();
    request.completeError(StateError('old confirmation'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('reasoning-update-error')), findsNothing);
    expect(
      find.byKey(const ValueKey('reasoning-selected-low')),
      findsOneWidget,
    );
  });

  testWidgets('latest failure keeps the confirmed value and permits retry', (
    tester,
  ) async {
    var calls = 0;
    await tester.pumpWidget(
      _host(
        ReasoningSlider(
          levels: _levels,
          value: 'high',
          onChanged: (_) async {
            if (++calls == 1) throw StateError('rejected');
          },
        ),
      ),
    );
    await tester.tap(_level('low'));
    await tester.pumpAndSettle();
    expect(find.textContaining('rejected'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('reasoning-selected-high')),
      findsOneWidget,
    );
    await tester.tap(_level('low'));
    await tester.pumpAndSettle();
    expect(calls, 2);
    expect(find.byKey(const ValueKey('reasoning-update-error')), findsNothing);
  });

  testWidgets('external pending intent replaces an older local failure', (
    tester,
  ) async {
    final request = Completer<void>();
    String? pending;
    late StateSetter update;
    await tester.pumpWidget(
      _host(
        StatefulBuilder(
          builder: (context, setState) {
            update = setState;
            return ReasoningSlider(
              levels: _levels,
              value: 'high',
              pendingValue: pending,
              onChanged: (_) => request.future,
            );
          },
        ),
      ),
    );
    await tester.tap(_level('low'));
    await tester.pump();
    update(() => pending = 'medium');
    await tester.pump();
    request.completeError(StateError('superseded outside this control'));
    await tester.pumpAndSettle();
    expect(find.text('正在切换：中'), findsOneWidget);
    expect(find.byKey(const ValueKey('reasoning-update-error')), findsNothing);
  });

  testWidgets('intermediate confirmation keeps the latest queued error valid', (
    tester,
  ) async {
    final requests = <String, Completer<void>>{};
    var value = 'high';
    String? pending;
    late StateSetter update;
    await tester.pumpWidget(
      _host(
        StatefulBuilder(
          builder: (context, setState) {
            update = setState;
            return ReasoningSlider(
              levels: _levels,
              value: value,
              pendingValue: pending,
              onChanged: (id) {
                setState(() => pending = id);
                return (requests[id] = Completer<void>()).future;
              },
            );
          },
        ),
      ),
    );
    await tester.tap(_level('low'));
    await tester.pump();
    await tester.tap(_level('medium'));
    await tester.pump();
    requests['low']!.complete();
    update(() => value = 'low');
    await tester.pump();
    expect(find.text('正在切换：中'), findsOneWidget);
    requests['medium']!.completeError(
      StateError('latest queued intent failed'),
    );
    await tester.pumpAndSettle();
    expect(find.textContaining('latest queued intent failed'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('reasoning-selected-low')),
      findsOneWidget,
    );
  });

  for (final scale in [1.0, 2.0]) {
    testWidgets('eight levels wrap without truncation at scale $scale', (
      tester,
    ) async {
      final ids = [
        'none',
        'off',
        'minimal',
        'low',
        'medium',
        'high',
        'xhigh',
        'max',
      ];
      await tester.pumpWidget(
        _host(
          ReasoningSlider(
            levels: [
              for (final id in ids) {'id': id, 'name': id},
            ],
            value: 'high',
            showHeading: false,
            onChanged: (_) async {},
          ),
          width: 280,
          scale: scale,
        ),
      );
      final tops = <double>{};
      for (final id in ids) {
        final option = _level(id);
        final rect = tester.getRect(option);
        tops.add(rect.top);
        expect(rect.width, lessThanOrEqualTo(280));
        expect(rect.height, greaterThanOrEqualTo(36));
        final label = find.descendant(of: option, matching: find.byType(Text));
        final text = tester.widget<Text>(label);
        expect(text.maxLines, isNull);
        expect(text.overflow, isNot(TextOverflow.ellipsis));
        expect(
          tester.renderObject<RenderParagraph>(label).didExceedMaxLines,
          isFalse,
        );
        expect(rect.contains(tester.getCenter(label)), isTrue);
      }
      expect(tops.length, greaterThan(1));
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('long provider labels wrap and retain descriptions in the Tip', (
    tester,
  ) async {
    const custom = 'Very detailed provider reasoning mode';
    await tester.pumpWidget(
      _host(
        ReasoningSlider(
          levels: const [
            {
              'id': 'custom',
              'name': custom,
              'description': 'Provider capability',
            },
          ],
          value: null,
          showHeading: false,
          onChanged: (_) async {},
        ),
        width: 280,
        scale: 2,
      ),
    );
    final label = find.descendant(
      of: _level('custom'),
      matching: find.text(custom),
    );
    expect(label, findsOneWidget);
    expect(tester.getSize(_level('custom')).height, greaterThan(54));
    expect(
      tester.renderObject<RenderParagraph>(label).didExceedMaxLines,
      isFalse,
    );
    expect(tester.takeException(), isNull);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: tester.getCenter(_level('custom')));
    await tester.pump(const Duration(milliseconds: 600));
    expect(find.text('$custom\nProvider capability'), findsOneWidget);
    await mouse.removePointer();
    await tester.pumpAndSettle();
  });

  testWidgets('options can be selected from the keyboard', (tester) async {
    final commits = <String>[];
    await tester.pumpWidget(
      _host(
        ReasoningSlider(
          levels: _levels,
          value: 'high',
          onChanged: (id) async => commits.add(id),
        ),
      ),
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(commits, ['low']);
  });

  testWidgets('disconnected or sending controls never submit changes', (
    tester,
  ) async {
    var calls = 0;
    await tester.pumpWidget(
      _host(
        ReasoningSlider(
          levels: _levels,
          value: 'high',
          enabled: false,
          onChanged: (_) async => calls++,
        ),
      ),
    );
    await tester.tap(_level('low'));
    await tester.pumpAndSettle();
    expect(calls, 0);
    expect(
      find.byKey(const ValueKey('reasoning-selected-high')),
      findsOneWidget,
    );
  });
}
