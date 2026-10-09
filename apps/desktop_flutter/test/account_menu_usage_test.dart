import 'dart:async';
import 'dart:io';

import 'package:dsh_desktop/features/account_menu.dart';
import 'package:dsh_desktop/features/sidebar_entries.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'account_usage_panel_test.dart' as usage;
import 'desktop_product_visual_test.dart' show exportProductFrame;

void main() {
  setUpAll(() async {
    if (!Platform.isWindows) return;
    for (final name in ['Segoe UI', 'Microsoft YaHei UI', 'Microsoft YaHei']) {
      final file = File(
        'C:/Windows/Fonts/${name == 'Segoe UI' ? 'segoeui.ttf' : 'msyh.ttc'}',
      );
      if (await file.exists()) {
        await (FontLoader(
          name,
        )..addFont(file.readAsBytes().then(ByteData.sublistView))).load();
      }
    }
  });
  for (final scale in [1.0, 2.0]) {
    testWidgets(
      'menu loads real usage only after an explicit click scale=$scale',
      (tester) async {
        await tester.binding.setSurfaceSize(const Size(720, 520));
        tester.view.physicalSize = const Size(720, 520);
        tester.view.devicePixelRatio = 1;
        tester.binding.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(() {
          tester.binding.platformDispatcher.clearTextScaleFactorTestValue();
          tester.view.resetPhysicalSize();
          tester.view.resetDevicePixelRatio();
          return tester.binding.setSurfaceSize(null);
        });
        final api = usage.UsageApi();
        final c = usage.UsageController(api)..account('a');
        c.subscriptionAccounts.first['name'] = 'Devin';
        c.subscriptionAccounts.addAll([
          {
            'id': 'openai-codex',
            'name': 'ChatGPT / Codex',
            'signedIn': false,
            'accounts': [],
          },
          {
            'id': 'copilot',
            'name': 'GitHub Copilot',
            'signedIn': false,
            'accounts': [],
          },
        ]);
        final boundary = GlobalKey();
        await tester.pumpWidget(
          RepaintBoundary(
            key: boundary,
            child: ShadApp(
              home: Scaffold(
                body: Align(
                  alignment: Alignment.bottomLeft,
                  child: SizedBox(
                    width: 260,
                    child: AccountConnectionMenu(
                      controller: c,
                      onAccounts: () {},
                      onModels: () {},
                      onSettings: () {},
                      settingsShortcut: 'Ctrl+,',
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(api.requests, isEmpty);
        await tester.tap(find.byKey(const ValueKey('account-connection-menu')));
        await tester.pumpAndSettle();
        expect(find.text('Devin'), findsOneWidget);
        expect(find.text('ChatGPT / Codex'), findsNothing);
        expect(find.text('GitHub Copilot'), findsNothing);
        expect(api.requests, isEmpty);
        expect(find.byType(LinearProgressIndicator), findsNothing);
        await tester.tap(find.text('查看额度'));
        await tester.pumpAndSettle();
        expect(find.text('已用 37% · 剩余 63%'), findsOneWidget);
        expect(api.requests.single, {'provider': 'devin', 'accountScope': 'a'});
        final bar = tester.widget<LinearProgressIndicator>(
          find.byType(LinearProgressIndicator),
        );
        expect(bar.value, .37);
        final panel = tester.getRect(
          find.byKey(const ValueKey('account-menu-panel')),
        );
        expect(panel.left, greaterThanOrEqualTo(0));
        expect(panel.right, lessThanOrEqualTo(720));
        expect(panel.top, greaterThanOrEqualTo(0));
        expect(panel.bottom, lessThanOrEqualTo(520));
        await exportProductFrame(tester, boundary, 'account-usage-${scale}x');
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await tester.pumpAndSettle();
        expect(find.text('已用 37% · 剩余 63%'), findsNothing);
        await tester.tap(find.byKey(const ValueKey('account-connection-menu')));
        await tester.pumpAndSettle();
        expect(find.text('查看额度'), findsOneWidget);
        expect(api.requests, hasLength(1));
        expect(tester.takeException(), isNull);
        await usage.cleanup(tester, c, api);
      },
      variant: TargetPlatformVariant.only(TargetPlatform.windows),
    );
  }

  testWidgets(
    'account change removes usage immediately and discards delayed replies',
    (tester) async {
      final api = usage.UsageApi();
      final pending = Completer<Map<String, dynamic>>();
      api.handler = (_) => pending.future;
      final c = usage.UsageController(api)..account('a');
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 260,
              child: AccountConnectionMenu(
                controller: c,
                onAccounts: () {},
                onModels: () {},
                onSettings: () {},
                settingsShortcut: 'Ctrl+,',
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('account-connection-menu')));
      await tester.pumpAndSettle();
      expect(api.requests, isEmpty);
      await tester.tap(find.text('查看额度'));
      await tester.pump();
      expect(api.requests, hasLength(1));
      c.subscriptionAccounts = [];
      c.changed();
      await tester.pump();
      pending.complete(usage.response());
      await tester.pumpAndSettle();
      expect(find.text('已用 37% · 剩余 63%'), findsNothing);
      expect(find.text('连接已变化，请重新打开菜单'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await usage.cleanup(tester, c, api);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.windows),
  );

  testWidgets(
    'Harness new-session action has centered official icon and keyboard activation',
    (tester) async {
      var calls = 0;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 240,
              child: NewSessionButton(
                buttonKey: const Key('new-task'),
                tooltip: '新建会话',
                onPressed: () => calls++,
              ),
            ),
          ),
        ),
      );
      final button = find.byKey(const Key('new-task'));
      expect(tester.getSize(button).height, 38);
      final glyph = tester.widget<DshGlyph>(find.byType(DshGlyph));
      expect(glyph.data, DshIcons.newSession.data);
      expect(
        DshIcons.newSession.asset,
        'assets/icons/web-IconNewChatOutline16.svg',
      );
      final row = find.descendant(of: button, matching: find.byType(Row));
      expect(tester.getCenter(row).dx, tester.getCenter(button).dx);
      await tester.sendKeyEvent(LogicalKeyboardKey.tab);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(calls, 1);
      await tester.tap(button);
      expect(calls, 2);
      expect(tester.takeException(), isNull);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.windows),
  );
}
