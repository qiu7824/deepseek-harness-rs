import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/account_menu.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;
import 'workbench_tabs_test.dart' as fixture;

void main() {
  testWidgets(
    'Settings remains direct and login details appear only after authorization',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1440, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final api = fixture.TabApi();
      final c = fixture.TabController(api);
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('account-connection-menu')),
        findsNothing,
      );
      expect(find.byKey(const Key('open-settings-direct')), findsOneWidget);
      expect(find.byKey(const Key('open-schedule-direct')), findsOneWidget);
      await tester.tap(find.byKey(const Key('open-settings-direct')));
      await tester.pumpAndSettle();
      expect(find.byType(SettingsShell), findsOneWidget);
      expect(find.text('管理订阅账号'), findsNothing);
      Navigator.pop(tester.element(find.byType(SettingsShell)));
      await tester.pumpAndSettle();
      c.subscriptionAccounts = [
        {
          'id': 'openai-codex',
          'name': 'ChatGPT / Codex',
          'signedIn': true,
          'accounts': [
            {'label': 'demo@example.test', 'active': true, 'needsLogin': false},
          ],
        },
      ];
      c.emit();
      await tester.pumpAndSettle();
      expect(find.text('账号'), findsOneWidget);
      expect(find.byKey(const Key('open-settings-direct')), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await api.close();
    },
  );
  test('account labels are masked and authorization does not imply health', () {
    expect(maskedAccountLabel('demo@example.test'), 'de***@example.test');
    expect(maskedAccountLabel('abcdef0123456789'), 'ab…6789');
    expect(
      accountNeedsLogin({
        'signedIn': true,
        'accounts': [
          {'active': true, 'needsLogin': true},
        ],
      }),
      isTrue,
    );
    expect(
      accountNeedsLogin({
        'accounts': [
          {'active': true, 'needsLogin': false},
          {'needsLogin': true},
        ],
      }),
      isFalse,
    );
  });
  for (final compact in [false, true]) {
    testWidgets(
      'account menu stays available without authorization compact=$compact',
      (tester) async {
        final c = DesktopController(MemoryPreferences());
        var selected = '';
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: Align(
                alignment: Alignment.bottomLeft,
                child: SizedBox(
                  width: compact ? 48 : 260,
                  child: AccountConnectionMenu(
                    controller: c,
                    compact: compact,
                    settingsShortcut: 'Ctrl+,',
                    onAccounts: () => selected = 'accounts',
                    onModels: () => selected = 'models',
                    onSettings: () => selected = 'settings',
                  ),
                ),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(find.byTooltip('账号与连接'), findsOneWidget);
        await tester.tap(find.byKey(const ValueKey('account-connection-menu')));
        await tester.pumpAndSettle();
        expect(find.text('尚未授权订阅账号'), findsOneWidget);
        await tester.tap(find.text('API 连接与模型'));
        await tester.pumpAndSettle();
        expect(selected, 'models');
        await tester.tap(find.byKey(const ValueKey('account-connection-menu')));
        await tester.pumpAndSettle();
        await tester.tap(find.text('设置'));
        await tester.pumpAndSettle();
        expect(selected, 'settings');
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox());
        c.dispose();
      },
    );
  }
  testWidgets(
    'Host change masks old account rows and blocks old menu actions',
    (tester) async {
      final c = DesktopController(MemoryPreferences())
        ..subscriptionAccounts = [
          {
            'id': 'openai-codex',
            'name': 'ChatGPT / Codex',
            'signedIn': true,
            'accounts': [
              {
                'label': 'demo@example.test',
                'active': true,
                'needsLogin': true,
              },
            ],
          },
        ];
      var calls = 0;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: AccountConnectionMenu(
              controller: c,
              settingsShortcut: 'Ctrl+,',
              onAccounts: () => calls++,
              onModels: () => calls++,
              onSettings: () => calls++,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('账号'), findsOneWidget);
      expect(
        tester.widget<DshTooltip>(find.byType(DshTooltip)).message,
        contains('需重新登录'),
      );
      await tester.tap(find.byKey(const ValueKey('account-connection-menu')));
      await tester.pumpAndSettle();
      expect(find.textContaining('de***@example.test'), findsOneWidget);
      expect(find.textContaining('服务在线'), findsNothing);
      c.host = HostInfo.fromJson({
        'version': 'new',
        'home': 'new',
        'cwd': 'new',
      });
      c.emit();
      await tester.pumpAndSettle();
      expect(find.textContaining('de***@example.test'), findsNothing);
      expect(find.text('连接已变化，请重新打开菜单'), findsOneWidget);
      await tester.tap(find.text('管理订阅账号'));
      await tester.pumpAndSettle();
      expect(calls, 0);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
  for (final scale in [1.0, 2.0]) {
    for (final accountCase in ['single', 'multiple', 'login-required']) {
      testWidgets(
        'account and Settings row heights match scale=$scale case=$accountCase',
        (tester) async {
          await tester.binding.setSurfaceSize(const Size(1440, 900));
          tester.binding.platformDispatcher.textScaleFactorTestValue = scale;
          addTearDown(() {
            tester.binding.platformDispatcher.clearTextScaleFactorTestValue();
            return tester.binding.setSurfaceSize(null);
          });
          final api = fixture.TabApi();
          final c = fixture.TabController(api)
            ..subscriptionAccounts = [
              {
                'id': 'openai-codex',
                'name': 'ChatGPT / Codex',
                'signedIn': true,
                'accounts': [
                  {
                    'label': 'long-private-name@example.test',
                    'active': true,
                    'needsLogin': accountCase == 'login-required',
                  },
                ],
              },
              if (accountCase == 'multiple')
                {
                  'id': 'deepseek-account',
                  'name': 'DeepSeek',
                  'signedIn': true,
                  'accounts': [
                    {'label': 'second@example.test', 'active': true},
                  ],
                },
            ];
          await tester.pumpWidget(DesktopApp(controller: c));
          await tester.pumpAndSettle();
          final account = find.byKey(const ValueKey('account-connection-menu'));
          final settings = find.byKey(const Key('open-settings-direct'));
          final accountRect = tester.getRect(account);
          final settingsRect = tester.getRect(settings);
          expect(accountRect.height, closeTo(settingsRect.height, .01));
          expect(accountRect.height, scale == 1 ? 36 : 58);
          expect(accountRect.left, settingsRect.left);
          expect(accountRect.width, settingsRect.width);
          final rowText = tester.widget<Text>(
            find.descendant(of: account, matching: find.byType(Text)),
          );
          expect(rowText.maxLines, 1);
          expect(rowText.overflow, TextOverflow.ellipsis);
          expect(rowText.data, '账号');
          final tooltip = tester.widget<DshTooltip>(
            find.ancestor(of: account, matching: find.byType(DshTooltip)),
          );
          expect(tooltip.message, contains('lo***@example.test'));
          expect(tooltip.message, isNot(contains('long-private-name')));
          expect(
            tooltip.message,
            contains(accountCase == 'login-required' ? '需重新登录' : '已授权'),
          );
          if (accountCase == 'multiple') {
            expect(tooltip.message, contains('DeepSeek · se***@example.test'));
          }
          final mouse = await tester.createGesture(
            kind: PointerDeviceKind.mouse,
          );
          await mouse.addPointer(location: Offset.zero);
          await mouse.moveTo(accountRect.center);
          await tester.pump(const Duration(milliseconds: 600));
          await tester.pump(const Duration(milliseconds: 200));
          expect(find.text(tooltip.message), findsOneWidget);
          expect(find.textContaining('long-private-name'), findsNothing);
          await mouse.removePointer();
          await tester.pumpAndSettle();
          await tester.tap(
            find.byWidgetPredicate(
              (widget) =>
                  widget is DshIcon &&
                  widget.icon == DshIcons.panelLeftClose.data,
            ),
          );
          await tester.pumpAndSettle();
          final compactAccountRect = tester.getRect(account);
          final compactSettingsRect = tester.getRect(settings);
          expect(compactAccountRect.height, compactSettingsRect.height);
          expect(compactAccountRect.width, compactSettingsRect.width);
          expect(compactAccountRect.size, const Size(36, 36));
          expect(compactAccountRect.left, compactSettingsRect.left);
          expect(tester.takeException(), isNull);
          await tester.pumpWidget(const SizedBox());
          c.dispose();
          await api.close();
        },
      );
    }
  }
  testWidgets('tools menu keeps knowledge and scheduled execution accessible', (
    tester,
  ) async {
    var knowledge = 0, schedule = 0;
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: SidebarToolsMenu(
            onKnowledge: () => knowledge++,
            onSchedule: () => schedule++,
          ),
        ),
      ),
    );
    await tester.tap(find.byKey(const Key('open-tools')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('open-knowledge')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('open-tools')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('open-schedule')));
    await tester.pumpAndSettle();
    expect(knowledge, 1);
    expect(schedule, 1);
    expect(tester.takeException(), isNull);
  });
}
