import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/context_quick_settings.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

void main() {
  test('token counts use 万 above ten thousand', () {
    expect(compactTokens(9500), '9500');
    expect(compactTokens(32000), '3.2万');
    expect(compactTokens(1280000), '128万');
  });

  testWidgets(
    'shows usage, stores a per-model threshold, resets it and compacts',
    (tester) async {
      final calls = <(String, Json)>[];
      final api = FakeClient()
        ..handleCall = (method, payload) async {
          calls.add((method, payload));
          if (method == 'commands.execute') {
            return {
              'result': {'kind': 'success'},
            };
          }
          return {'items': [], 'archivedSessionIds': []};
        };
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await tester.pump();
      c.selectedId = 's';
      c.catalog = ModelCatalog.fromJson({
        'current': {'provider': 'deepseek', 'model': 'v4'},
        'routable': true,
        'groups': [],
      });
      c.projectionWindow.apply('contextPressure', {
        'projectedTokens': 32000,
        'contextWindow': 128000,
      }, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 440,
              child: ContextQuickSettings(controller: c),
            ),
          ),
        ),
      );
      await tester.pump();
      expect(find.text('25% · 3.2万 / 12.8万'), findsOneWidget);
      expect(find.text('80%'), findsOneWidget, reason: 'default threshold');
      expect(find.text('使用默认阈值'), findsOneWidget);

      final slider = find.byKey(const Key('compaction-threshold'));
      final left =
          tester.getTopLeft(slider) +
          Offset(4, tester.getSize(slider).height / 2);
      await tester.tapAt(left);
      await tester.pumpAndSettle();
      final write = calls.lastWhere((call) => call.$1 == 'settings.replace').$2;
      expect(write['ns'], 'context-compaction');
      expect(write['section'], {
        'thresholds': {'deepseek/v4': 0.3},
      });
      expect(find.text('当前模型使用自定义阈值'), findsOneWidget);
      expect(find.text('30%'), findsOneWidget);

      await tester.tap(find.byKey(const Key('compaction-threshold-reset')));
      await tester.pumpAndSettle();
      expect(
        calls.lastWhere((call) => call.$1 == 'settings.replace').$2['section'],
        {'thresholds': <String, Object?>{}},
      );
      expect(find.text('80%'), findsOneWidget);

      await tester.tap(find.byKey(const Key('compact-now')));
      await tester.pumpAndSettle();
      expect(calls.lastWhere((call) => call.$1 == 'commands.execute').$2, {
        'args': {'agentId': 's', 'line': '/compact'},
      });
      expect(find.text('已开始压缩'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
}
