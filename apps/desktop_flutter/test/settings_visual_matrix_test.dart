import 'dart:io';
import 'dart:ui' as ui;

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/features/settings/resource_page.dart';
import 'package:dsh_desktop/l10n/zh.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

Json settingsNamespace(
  String name,
  Json value, [
  Map<String, List<String>> choices = const {},
]) => {
  'ns': name,
  'revision': 1,
  'value': value,
  'schema': {
    'uid': 'root',
    'refs': {
      'root': {
        'type': 'object',
        'dict': {for (final key in value.keys) key: key},
      },
      for (final entry in value.entries)
        entry.key: choices.containsKey(entry.key)
            ? {
                'type': 'union',
                'list': [
                  for (final item in choices[entry.key]!) '${entry.key}:$item',
                ],
              }
            : {
                'type': entry.value is bool
                    ? 'boolean'
                    : entry.value is num
                    ? 'number'
                    : 'string',
              },
      for (final entry in choices.entries)
        for (final item in entry.value)
          '${entry.key}:$item': {'type': 'const', 'value': item},
    },
  },
};

class SettingsVisualClient extends DshClient {
  SettingsVisualClient() : super('http://127.0.0.1:58080');

  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    if (mutation) throw StateError('Visual fixtures must not submit changes');
    return switch (method) {
      'settings.describe' => {
        'namespaces': [
          settingsNamespace('uu-remote', {
            'cliPath': r'C:\Program Files\Remote Desktop\remote-client.exe',
            'account': 'desktop-preview-account',
            'deviceId': 'desktop-preview-device',
          }),
          settingsNamespace(
            'computer-use',
            {
              'nativeProtocol': false,
              'nativeTarget': 'local',
              'enabled': true,
              'adapter': 'native-browser',
              'command': '',
              'browserExecutable':
                  r'C:\Program Files\Browser\Application\browser.exe',
              'browserHeadless': false,
              'maxBrowserSessions': 4,
              'timeoutSeconds': 60,
            },
            {
              'nativeTarget': ['local', 'browser'],
              'adapter': [
                'native-browser',
                'native-desktop',
                'uu-desktop',
                'command',
              ],
            },
          ),
        ],
      },
      'pluginInventory.list' => {
        'entries': [
          for (final item in [
            ('noop', 'cordis:noop', true),
            ('review', 'dsh-auto-review', false),
            ('clock', 'dsh-time-context', false),
            ('skin', 'dsh-skin-center', true),
            ('voice', 'dsh-voice-input', true),
          ])
            {
              'entryId': item.$1,
              'moduleName': item.$2,
              'name': item.$2,
              'enabled': item.$3,
            'fiberPhase': item.$3 ? 'active' : null,
            },
        ],
      },
      'pluginInventory.getConfig' => {
        'entryId': payload['entryId'],
        'moduleName': 'dsh-time-context',
        'revision': 'preview-r1',
        'enabled': false,
        'config': {'refreshIntervalMs': 600000},
      },
      _ => <String, dynamic>{},
    };
  }
}

class SettingsVisualController extends DesktopController {
  SettingsVisualController() : super(DesktopPreferences(writer: (_) async {}));
  final api = SettingsVisualClient();
  @override
  DshClient get client => api;
}

void main() {
  var fontsLoaded = false;
  for (final dark in [false, true]) {
    for (final viewport in [
      (size: const Size(1100, 860), scale: 1.0),
      (size: const Size(720, 520), scale: 2.0),
      (size: const Size(1280, 900), scale: 1.5),
    ]) {
      for (final page in ['environment', 'plugins']) {
        testWidgets(
          'settings $page dark=$dark width=${viewport.size.width} text=${viewport.scale}',
          (tester) async {
            final output = Platform.environment['DSH_SETTINGS_VISUAL_DIR'];
            await tester.binding.setSurfaceSize(viewport.size);
            tester.view.devicePixelRatio = 1;
            tester.view.physicalSize = viewport.size;
            addTearDown(tester.view.resetDevicePixelRatio);
            addTearDown(tester.view.resetPhysicalSize);
            tester.platformDispatcher.textScaleFactorTestValue = viewport.scale;
            addTearDown(
              tester.platformDispatcher.clearTextScaleFactorTestValue,
            );
            addTearDown(() => tester.binding.setSurfaceSize(null));
            // Font metrics must be identical with and without screenshot export.
            if (!fontsLoaded) {
              await tester.runAsync(() async {
                final fonts = Platform.environment['WINDIR'] ?? 'C:/Windows';
                for (final entry in {
                  'Segoe UI': 'segoeui.ttf',
                  'Microsoft YaHei UI': 'msyh.ttc',
                  'Microsoft YaHei': 'msyh.ttc',
                  'Cascadia Mono': 'CascadiaMono.ttf',
                  'Consolas': 'consola.ttf',
                }.entries) {
                  final file = File('$fonts/Fonts/${entry.value}');
                  if (await file.exists()) {
                    await (FontLoader(entry.key)..addFont(
                          file.readAsBytes().then(ByteData.sublistView),
                        ))
                        .load();
                  }
                }
                fontsLoaded = true;
              });
            }
            final controller = SettingsVisualController()
              ..preferences.dark = dark;
            final boundary = GlobalKey();
            await tester.pumpWidget(
              RepaintBoundary(
                key: boundary,
                child: DesktopApp(controller: controller),
              ),
            );
            await tester.pumpAndSettle();
            final context = tester.element(find.byType(Workbench));
            showDialog<void>(
              context: context,
              builder: (_) =>
                  SettingsShell(controller: controller, initialPage: page),
            );
            await tester.pumpAndSettle();
            expect(find.byType(SettingsShell), findsOneWidget);
            expect(tester.takeException(), isNull);
            if (page == 'environment') {
              await tester.ensureVisible(
                find.text(DshSettingsZh.enableComputerUse),
              );
              await tester.pumpAndSettle();
              final scroll = find.byKey(const ValueKey('settings-form-scroll'));
              expect(scroll, findsOneWidget);
              if (scroll.evaluate().isNotEmpty) {
                final right = tester.getRect(scroll).right;
                for (final element
                    in find
                        .descendant(
                          of: scroll,
                          matching: find.byType(DshSwitch),
                        )
                        .evaluate()) {
                  final box = element.renderObject! as RenderBox;
                  expect(
                    box.localToGlobal(Offset(box.size.width, 0)).dx,
                    lessThanOrEqualTo(right - 12),
                  );
                }
              }
            }
            if (page == 'plugins') {
              expect(find.text('dsh-skin-center'), findsNothing);
              expect(find.text('active'), findsNothing);
              final toggle = find.byKey(
                const ValueKey('plugin-config-toggle-clock'),
              );
              final list = find
                  .descendant(
                    of: find.byType(SettingsResourcePage),
                    matching: find.byType(ListView),
                  )
                  .first;
              final scrollable = find
                  .descendant(of: list, matching: find.byType(Scrollable))
                  .first;
              await tester.scrollUntilVisible(
                toggle,
                160,
                scrollable: scrollable,
              );
              await tester.pumpAndSettle();
              await tester.tap(toggle);
              await tester.pumpAndSettle();
              final interval = find.byKey(
                const ValueKey('time-context-interval'),
              );
              expect(interval, findsOneWidget);
              await tester.ensureVisible(interval);
              await tester.pumpAndSettle();
            }
            expect(tester.takeException(), isNull);
            if (output != null) {
              await tester.runAsync(() async {
                final image =
                    await (boundary.currentContext!.findRenderObject()!
                            as RenderRepaintBoundary)
                        .toImage(pixelRatio: 1);
                final bytes = await image.toByteData(
                  format: ui.ImageByteFormat.png,
                );
                await Directory(output).create(recursive: true);
                await File(
                  '$output/$page-${dark ? 'dark' : 'light'}-${viewport.size.width.toInt()}-${viewport.scale}.png',
                ).writeAsBytes(bytes!.buffer.asUint8List());
                image.dispose();
              });
            }
            await tester.pumpWidget(const SizedBox());
            controller.dispose();
            await controller.api.close();
          },
          variant: TargetPlatformVariant({TargetPlatform.windows}),
          skip: !Platform.isWindows,
        );
      }
    }
  }
}
