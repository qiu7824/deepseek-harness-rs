import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/account_menu.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;
import 'workbench_tabs_test.dart' as fixture;

/// The Host lists every supported subscription provider, signed in or not.
List<Json> hostProviders({bool attention = true}) => [
  for (final id in [
    'copilot',
    'qwen-oauth',
    'minimax-oauth',
    'minimax-cn-oauth',
    'nous',
    'xai-oauth',
  ])
    {'id': id, 'name': id.toUpperCase(), 'signedIn': false, 'accounts': []},
  {
    'id': 'openai-codex',
    'name': 'ChatGPT / Codex',
    'signedIn': true,
    'accounts': [
      {
        'accountScope': 'a',
        'label': 'demo@example.test',
        'active': true,
        'needsLogin': false,
      },
    ],
  },
  {
    'id': 'devin',
    'name': 'Devin',
    'signedIn': true,
    'accounts': [
      {
        'accountScope': 'b',
        'label': 'review@example.test',
        'active': true,
        'needsLogin': attention,
      },
    ],
  },
];

Future<void> pumpMenu(
  WidgetTester tester,
  DesktopController c, {
  required ValueChanged<String?> onManage,
  VoidCallback? onModels,
  ValueChanged<String>? onLogin,
  bool compact = false,
}) async {
  await tester.binding.setSurfaceSize(const Size(1000, 800));
  addTearDown(() => tester.binding.setSurfaceSize(null));
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: Align(
          alignment: Alignment.bottomLeft,
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: SizedBox(
              width: compact ? 36 : 260,
              child: AccountConnectionMenu(
                controller: c,
                compact: compact,
                onManageAccounts: onManage,
                onModels: onModels ?? () {},
                onLogin: onLogin,
              ),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Finder get trigger => find.byKey(const ValueKey('account-connection-menu'));
Finder get panel => find.byKey(const ValueKey('account-menu-panel'));

void main() {
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

  test('only connected providers are listed, those needing login first', () {
    final linked = linkedAccountProviders(hostProviders());
    expect(linked.map((p) => p['id']), ['devin', 'openai-codex']);
    expect(
      linkedAccountProviders(hostProviders(attention: false))
          .map((p) => p['id']),
      ['openai-codex', 'devin'],
    );
    expect(linkedAccountProviders(hostProviders().take(6).toList()), isEmpty);
  });

  testWidgets('without a connected account the entry opens settings directly', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences())
      ..subscriptionAccounts = hostProviders().take(6).toList();
    final opened = <String?>[];
    await pumpMenu(tester, c, onManage: opened.add);
    expect(find.text('登录订阅账号'), findsOneWidget);
    await tester.tap(trigger);
    await tester.pumpAndSettle();
    expect(opened, [null]);
    expect(panel, findsNothing);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });

  for (final compact in [false, true]) {
    testWidgets(
      'panel lists linked accounts above the trigger compact=$compact',
      (tester) async {
        final c = DesktopController(MemoryPreferences())
          ..subscriptionAccounts = hostProviders();
        final managed = <String?>[], logins = <String>[];
        var models = 0;
        await pumpMenu(
          tester,
          c,
          compact: compact,
          onManage: managed.add,
          onModels: () => models++,
          onLogin: logins.add,
        );
        if (!compact) {
          expect(find.text('订阅账号'), findsOneWidget);
          expect(find.text('需重新登录'), findsOneWidget);
        }
        await tester.tap(trigger);
        await tester.pumpAndSettle();
        expect(panel, findsOneWidget);
        // Never-used providers stay in settings, not in this panel.
        for (final unused in ['COPILOT', 'NOUS', 'XAI-OAUTH']) {
          expect(find.text(unused), findsNothing);
        }
        expect(find.text('ChatGPT / Codex'), findsOneWidget);
        expect(find.text('Devin'), findsOneWidget);
        expect(find.text('de***@example.test'), findsOneWidget);
        expect(find.text('re***@example.test'), findsOneWidget);
        expect(find.textContaining('demo@example.test'), findsNothing);
        final panelRect = tester.getRect(panel);
        final triggerRect = tester.getRect(trigger);
        expect(panelRect.bottom, lessThanOrEqualTo(triggerRect.top));
        expect(panelRect.left, closeTo(triggerRect.left, 8));
        expect(
          (Offset.zero & const Size(1000, 800)).contains(panelRect.topLeft),
          isTrue,
        );
        // Devin needs a new sign-in, which starts from the panel itself.
        expect(
          find.byKey(const ValueKey('account-relogin-openai-codex')),
          findsNothing,
        );
        await tester.tap(find.byKey(const ValueKey('account-relogin-devin')));
        await tester.pumpAndSettle();
        expect(logins, ['devin']);
        expect(panel, findsNothing);
        await tester.tap(trigger);
        await tester.pumpAndSettle();
        await tester.tap(
          find.byKey(const ValueKey('account-provider-openai-codex')),
        );
        await tester.pumpAndSettle();
        expect(managed, ['openai-codex']);
        await tester.tap(trigger);
        await tester.pumpAndSettle();
        await tester.tap(find.byKey(const ValueKey('account-menu-manage')));
        await tester.pumpAndSettle();
        expect(managed, ['openai-codex', null]);
        await tester.tap(trigger);
        await tester.pumpAndSettle();
        await tester.tap(find.byKey(const ValueKey('account-menu-models')));
        await tester.pumpAndSettle();
        expect(models, 1);
        await tester.tap(trigger);
        await tester.pumpAndSettle();
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
        await tester.pumpAndSettle();
        expect(panel, findsNothing);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox());
        c.dispose();
      },
    );
  }

  testWidgets('a Host change closes the panel and blocks its old actions', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences())
      ..subscriptionAccounts = hostProviders();
    var calls = 0;
    await pumpMenu(
      tester,
      c,
      onManage: (_) => calls++,
      onLogin: (_) => calls++,
    );
    await tester.tap(trigger);
    await tester.pumpAndSettle();
    expect(find.text('re***@example.test'), findsOneWidget);
    final staleRow = tester.widget<InkWell>(
      find.byKey(const ValueKey('account-provider-devin')),
    );
    c.host = HostInfo.fromJson({'version': 'new', 'home': 'new', 'cwd': 'new'});
    c.emit();
    await tester.pumpAndSettle();
    expect(panel, findsNothing);
    staleRow.onTap!();
    await tester.pumpAndSettle();
    expect(calls, 0);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });

  for (final scale in [1.0, 2.0]) {
    testWidgets('sidebar footer keeps account and Settings in one row '
        'scale=$scale', (tester) async {
      await tester.binding.setSurfaceSize(const Size(1440, 900));
      tester.binding.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(() {
        tester.binding.platformDispatcher.clearTextScaleFactorTestValue();
        return tester.binding.setSurfaceSize(null);
      });
      final api = fixture.TabApi();
      final c = fixture.TabController(api)
        ..subscriptionAccounts = hostProviders();
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      final settings = find.byKey(const Key('open-settings-direct'));
      final accountRect = tester.getRect(trigger);
      final settingsRect = tester.getRect(settings);
      expect(accountRect.center.dy, closeTo(settingsRect.center.dy, .5));
      expect(accountRect.right, lessThanOrEqualTo(settingsRect.left));
      expect(accountRect.height, scale == 1 ? 36 : 58);
      final title = tester.widget<Text>(
        find.byKey(const ValueKey('account-connection-title')),
      );
      expect(title.maxLines, 1);
      expect(title.overflow, TextOverflow.ellipsis);
      expect(find.byTooltip('设置 · Ctrl+,'), findsOneWidget);
      await tester.tap(settings);
      await tester.pumpAndSettle();
      expect(
        tester.widget<SettingsShell>(find.byType(SettingsShell)).initialPage,
        'general',
      );
      Navigator.pop(tester.element(find.byType(SettingsShell)));
      await tester.pumpAndSettle();
      await tester.tap(
        find.byWidgetPredicate(
          (widget) =>
              widget is DshIcon && widget.icon == DshIcons.panelLeftClose.data,
        ),
      );
      await tester.pumpAndSettle();
      final compactAccount = tester.getRect(trigger);
      final compactSettings = tester.getRect(settings);
      expect(compactAccount.size, const Size(36, 36));
      expect(compactAccount.size, compactSettings.size);
      expect(compactAccount.left, compactSettings.left);
      expect(compactAccount.bottom, lessThanOrEqualTo(compactSettings.top));
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await api.close();
    });
  }

  testWidgets('a provider row opens its own entry in account settings', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1440, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final api = fixture.TabApi();
    final c = fixture.TabController(api)
      ..subscriptionAccounts = hostProviders();
    await tester.pumpWidget(DesktopApp(controller: c));
    await tester.pumpAndSettle();
    await tester.tap(trigger);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('account-provider-devin')));
    await tester.pumpAndSettle();
    final shell = tester.widget<SettingsShell>(find.byType(SettingsShell));
    expect(shell.initialPage, 'models');
    expect(shell.initialModelTab, 'accounts');
    expect(shell.initialAccountProvider, 'devin');
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await api.close();
  });
}
