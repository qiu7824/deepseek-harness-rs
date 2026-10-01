import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/design/select.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/l10n/settings_form_zh.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart' show ShadApp;

Json computerNamespace() {
  final values = <String, dynamic>{
    'nativeProtocol': false,
    'nativeTarget': 'browser',
    'enabled': true,
    'adapter': 'native-browser',
    'command': 'existing-controller',
    'browserExecutable': r'C:\Program Files\Browser\browser.exe',
    'browserHeadless': false,
    'maxBrowserSessions': 4,
    'timeoutSeconds': 60,
  };
  final options = {
    'nativeTarget': ['local', 'browser'],
    'adapter': [
      'auto',
      'command',
      'native-browser',
      'native-desktop',
      'uu-desktop',
    ],
  };
  return {
    'ns': 'computer-use',
    'value': values,
    'schema': {
      'uid': 'root',
      'refs': {
        'root': {
          'type': 'object',
          'dict': {for (final key in values.keys) key: key},
        },
        for (final entry in values.entries)
          entry.key: options.containsKey(entry.key)
              ? {
                  'type': 'union',
                  'list': [for (final item in options[entry.key]!) item],
                }
              : {'type': entry.value is bool ? 'boolean' : 'string'},
        for (final items in options.values)
          for (final item in items) item: {'type': 'const', 'value': item},
      },
    },
  };
}

Finder field(String key) =>
    find.byKey(ValueKey('settings-field-computer-use-$key'));

Finder control<T>(String key) =>
    find.descendant(of: field(key), matching: find.byType(T));

Future<List<Json>> pumpForm(WidgetTester tester, {double width = 820}) async {
  final namespace = computerNamespace();
  final pending = <Json>[];
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: Center(
          child: SizedBox(
            width: width,
            child: SingleChildScrollView(
              child: StatefulBuilder(
                builder: (context, setState) => NamespaceForm(
                  namespace: namespace,
                  pending: pending,
                  onChange: (path, value) => setState(() {
                    pending.removeWhere(
                      (op) => (op['path'] as List).join('/') == path.join('/'),
                    );
                    pending.add({'op': 'set', 'path': path, 'value': value});
                  }),
                ),
              ),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return pending;
}

Future<void> selectAdapter(WidgetTester tester, String adapter) async {
  tester
      .widget<DshSelect<Object>>(control<DshSelect<Object>>('adapter'))
      .onChanged!(adapter);
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('native protocol controls its target but not the adapter', (
    tester,
  ) async {
    await pumpForm(tester);
    expect(
      tester
          .widget<DshSelect<Object>>(control<DshSelect<Object>>('nativeTarget'))
          .onChanged,
      isNull,
    );
    expect(
      tester
          .widget<DshSelect<Object>>(control<DshSelect<Object>>('adapter'))
          .onChanged,
      isNotNull,
    );
    expect(find.text(DshSettingsFormZh.nativeTargetInactive), findsOneWidget);
    tester.widget<DshSwitch>(control<DshSwitch>('nativeProtocol')).onChanged!(
      true,
    );
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<DshSelect<Object>>(control<DshSelect<Object>>('nativeTarget'))
          .onChanged,
      isNotNull,
    );
    expect(
      tester
          .widget<DshSelect<Object>>(control<DshSelect<Object>>('nativeTarget'))
          .value,
      'browser',
    );
  });

  testWidgets(
    'adapter switching preserves hidden values and pending commands',
    (tester) async {
      final pending = await pumpForm(tester);
      expect(field('command'), findsNothing);
      expect(field('browserExecutable'), findsOneWidget);
      await selectAdapter(tester, 'command');
      expect(field('browserExecutable'), findsNothing);
      expect(field('browserHeadless'), findsNothing);
      expect(field('maxBrowserSessions'), findsNothing);
      expect(
        tester.widget<DshField>(control<DshField>('command')).controller!.text,
        'existing-controller',
      );
      await tester.enterText(
        control<TextField>('command'),
        'new-controller --desktop',
      );
      await selectAdapter(tester, 'native-desktop');
      expect(field('command'), findsNothing);
      expect(field('browserExecutable'), findsNothing);
      await selectAdapter(tester, 'command');
      expect(
        tester.widget<DshField>(control<DshField>('command')).controller!.text,
        'new-controller --desktop',
      );
      expect(
        pending
            .where((op) => (op['path'] as List).single == 'command')
            .single['value'],
        'new-controller --desktop',
      );
      expect(
        pending.any((op) => (op['path'] as List).single == 'browserExecutable'),
        isFalse,
      );
    },
  );

  testWidgets('auto mode shows browser settings only with an empty command', (
    tester,
  ) async {
    await pumpForm(tester);
    await selectAdapter(tester, 'auto');
    expect(field('command'), findsOneWidget);
    expect(field('browserExecutable'), findsNothing);
    await tester.enterText(control<TextField>('command'), '  ');
    await tester.pumpAndSettle();
    expect(field('browserExecutable'), findsOneWidget);
    expect(field('browserHeadless'), findsOneWidget);
    await tester.enterText(control<TextField>('command'), 'controller');
    await tester.pumpAndSettle();
    expect(field('browserExecutable'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  for (final scale in [1.0, 2.0]) {
    for (final width in [420.0, 820.0]) {
      testWidgets('fields fit width $width at text scale $scale', (
        tester,
      ) async {
        await tester.binding.setSurfaceSize(const Size(1000, 1200));
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(() => tester.binding.setSurfaceSize(null));
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        await pumpForm(tester, width: width);
        final input = control<DshField>('browserExecutable');
        final label = find.descendant(
          of: field('browserExecutable'),
          matching: find.text('浏览器可执行文件'),
        );
        final inputRect = tester.getRect(input),
            labelRect = tester.getRect(label);
        expect(inputRect.width, greaterThan(260));
        expect(
          inputRect.right,
          lessThanOrEqualTo(tester.getRect(field('browserExecutable')).right),
        );
        if (width < 600 || scale > 1.5) {
          expect(inputRect.top, greaterThan(labelRect.bottom));
          expect(inputRect.width, width);
        } else {
          expect(inputRect.left, greaterThan(labelRect.right));
        }
        expect(tester.takeException(), isNull);
      });
    }
  }
}
