import 'dart:convert';
import 'dart:math' as math;

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../design/primitives.dart';
import '../design/typography.dart';
import '../l10n/account_menu_zh.dart';
import '../src/controller.dart';
import 'settings/account_usage_panel.dart';

String maskedAccountLabel(Object? value) {
  final label = value is String ? value.trim() : '';
  if (label.isEmpty) return '';
  final at = label.indexOf('@');
  if (at > 0) {
    return '${label.substring(0, math.min(2, at))}***${label.substring(at)}';
  }
  if (label.length >= 16 && RegExp(r'^[a-fA-F0-9-]+$').hasMatch(label)) {
    return '${label.substring(0, 2)}…${label.substring(label.length - 4)}';
  }
  return label.length > 40 ? '${label.substring(0, 37)}…' : label;
}

bool accountNeedsLogin(Json provider) {
  final accounts = objects(provider['accounts']);
  final active = accounts
      .where((account) => account['active'] == true)
      .toList();
  return (active.isEmpty ? accounts : active).any(
    (account) => account['needsLogin'] == true,
  );
}

Json? activeAccount(Json provider) {
  final accounts = objects(provider['accounts']);
  return accounts.where((row) => row['active'] == true).firstOrNull ??
      accounts.firstOrNull;
}

String accountUsageScope(Json provider) {
  final own = provider['accountScope'];
  if (own is String && own.isNotEmpty) return own;
  return '${activeAccount(provider)?['accountScope'] ?? ''}';
}

List<Json> linkedAccountProviders(List<Json> providers) => providers
    .where(
      (provider) => provider['signedIn'] == true || accountNeedsLogin(provider),
    )
    .toList();

class AccountConnectionMenu extends StatefulWidget {
  const AccountConnectionMenu({
    super.key,
    required this.controller,
    required this.onAccounts,
    required this.onModels,
    required this.onSettings,
    required this.settingsShortcut,
    this.compact = false,
    this.onlyWhenAuthorized = false,
    this.showSettingsAction = true,
  });
  final DesktopController controller;
  final VoidCallback onAccounts, onModels, onSettings;
  final String settingsShortcut;
  final bool compact, onlyWhenAuthorized, showSettingsAction;
  @override
  State<AccountConnectionMenu> createState() => _AccountConnectionMenuState();
}

class _AccountConnectionMenuState extends State<AccountConnectionMenu> {
  final popover = ShadPopoverController();
  final triggerFocus = FocusNode(debugLabel: 'settings-account-trigger');
  Object? openedScope;
  DesktopController get c => widget.controller;
  Object get scope => (
    c,
    c.client,
    c.host,
    jsonEncode([
      for (final provider in c.subscriptionAccounts)
        [
          provider['id'],
          provider['signedIn'],
          provider['accountScope'],
          for (final account in objects(provider['accounts']))
            [
              account['accountId'],
              account['accountScope'],
              account['active'],
              account['needsLogin'],
              account['label'],
            ],
        ],
    ]),
  );

  @override
  void initState() {
    super.initState();
    c.addListener(changed);
  }

  @override
  void didUpdateWidget(AccountConnectionMenu oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, c)) {
      oldWidget.controller.removeListener(changed);
      c.addListener(changed);
    }
  }

  @override
  void dispose() {
    c.removeListener(changed);
    popover.dispose();
    triggerFocus.dispose();
    super.dispose();
  }

  void changed() {
    if (mounted) setState(() {});
  }

  void toggle() {
    Tooltip.dismissAllToolTips();
    if (popover.isOpen) {
      popover.hide();
    } else {
      openedScope = scope;
      popover.show();
    }
  }

  void act(VoidCallback callback) {
    if (openedScope != scope || !popover.isOpen) return;
    popover.hide();
    callback();
  }

  Widget action(
    String id,
    IconData icon,
    String label,
    VoidCallback callback,
  ) => DshButton(
    key: ValueKey('account-menu-$id'),
    width: double.infinity,
    padding: const EdgeInsets.symmetric(horizontal: 8),
    onPressed: () => act(callback),
    child: Expanded(
      child: Row(
        children: [
          DshGlyph(icon, size: 16),
          const SizedBox(width: 10),
          Expanded(child: Text(label, textAlign: TextAlign.start)),
          if (id == 'settings')
            Text(
              widget.settingsShortcut,
              style: DshTypography.caption.copyWith(
                color: DshColors(context).muted,
              ),
            ),
        ],
      ),
    ),
  );

  Widget providerCard(Json provider) {
    final colors = DshColors(context);
    final id = '${provider['id']}';
    final label = maskedAccountLabel(activeAccount(provider)?['label']);
    final needsLogin = accountNeedsLogin(provider);
    return Padding(
      key: ValueKey('account-provider-$id'),
      padding: const EdgeInsets.fromLTRB(8, 8, 8, 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text('${provider['name'] ?? id}', style: DshTypography.body),
          const SizedBox(height: 3),
          Text(
            [
              if (label.isNotEmpty) label,
              needsLogin
                  ? DshAccountMenuZh.expired
                  : DshAccountMenuZh.authorized,
            ].join(' · '),
            style: DshTypography.caption.copyWith(color: colors.muted),
          ),
          if (c.client != null)
            AccountUsageDisclosure(
              key: ValueKey('menu-usage-$id-${accountUsageScope(provider)}'),
              controller: c,
              api: c.client!,
              provider: id,
              accountScope: accountUsageScope(provider),
              visible: popover.isOpen && openedScope == scope,
              needsLogin: needsLogin,
              compact: true,
            ),
        ],
      ),
    );
  }

  Widget panel(BuildContext context) => ListenableBuilder(
    listenable: c,
    builder: (context, _) {
      if (openedScope != scope) {
        return const Padding(
          padding: EdgeInsets.all(12),
          child: Text(DshAccountMenuZh.changed),
        );
      }
      final providers = linkedAccountProviders(c.subscriptionAccounts);
      final overlayBox = Overlay.of(this.context).context.findRenderObject();
      final size = overlayBox is RenderBox && overlayBox.hasSize
          ? overlayBox.size
          : MediaQuery.sizeOf(context);
      return DefaultTextStyle(
        style: DshTypography.body.copyWith(color: DshColors(context).text),
        textAlign: TextAlign.start,
        child: Shortcuts(
          shortcuts: const {
            SingleActivator(LogicalKeyboardKey.escape): DismissIntent(),
          },
          child: Actions(
            actions: {
              DismissIntent: CallbackAction<DismissIntent>(
                onInvoke: (_) {
                  popover.hide();
                  triggerFocus.requestFocus();
                  return null;
                },
              ),
            },
            child: Focus(
              autofocus: true,
              child: SizedBox(
                key: const ValueKey('account-menu-panel'),
                width: math.min(360, math.max(120, size.width - 32)),
                child: ConstrainedBox(
                  constraints: BoxConstraints(
                    maxHeight: math.max(120, size.height - 100),
                  ),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Flexible(
                        fit: FlexFit.loose,
                        child: SingleChildScrollView(
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [
                              if (providers.isEmpty)
                                const Padding(
                                  padding: EdgeInsets.all(12),
                                  child: Text(DshAccountMenuZh.noAccounts),
                                ),
                              for (final provider in providers)
                                providerCard(provider),
                            ],
                          ),
                        ),
                      ),
                      const Divider(height: 12),
                      action(
                        'manage',
                        DshIcons.users.data,
                        DshAccountMenuZh.manageAccounts,
                        widget.onAccounts,
                      ),
                      action(
                        'models',
                        DshIcons.database.data,
                        DshAccountMenuZh.apiModels,
                        widget.onModels,
                      ),
                      if (widget.showSettingsAction)
                        action(
                          'settings',
                          DshIcons.settings.data,
                          DshAccountMenuZh.settings,
                          widget.onSettings,
                        ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      );
    },
  );

  @override
  Widget build(BuildContext context) {
    final providers = linkedAccountProviders(c.subscriptionAccounts);
    if (widget.onlyWhenAuthorized && providers.isEmpty) {
      return const SizedBox.shrink();
    }
    final colors = DshColors(context);
    final details = [
      DshAccountMenuZh.title,
      for (final provider in providers)
        [
          '${provider['name'] ?? provider['id']}',
          maskedAccountLabel(activeAccount(provider)?['label']),
          accountNeedsLogin(provider)
              ? DshAccountMenuZh.expired
              : DshAccountMenuZh.authorized,
        ].where((part) => part.isNotEmpty).join(' · '),
    ].join('\n');
    return ShadPopover(
      controller: popover,
      padding: const EdgeInsets.all(6),
      anchor: const ShadAnchorAuto(
        targetAnchor: Alignment.topLeft,
        followerAnchor: Alignment.topRight,
        offset: Offset(0, -6),
        fallback: ShadAnchorAuto(
          targetAnchor: Alignment.bottomLeft,
          followerAnchor: Alignment.bottomRight,
          offset: Offset(0, 6),
        ),
      ),
      popover: panel,
      child: DshTooltip(
        message: details,
        child: SizedBox(
          key: const ValueKey('account-connection-menu'),
          width: widget.compact ? 36 : null,
          height: widget.compact
              ? 36
              : DshTokens.of(context).controlHeight(context),
          child: DshButton(
            focusNode: triggerFocus,
            onPressed: toggle,
            padding: EdgeInsets.symmetric(horizontal: widget.compact ? 6 : 12),
            child: Expanded(
              child: Row(
                mainAxisAlignment: widget.compact
                    ? MainAxisAlignment.center
                    : MainAxisAlignment.start,
                mainAxisSize: widget.compact
                    ? MainAxisSize.min
                    : MainAxisSize.max,
                children: [
                  DshGlyph(
                    DshIcons.settings.data,
                    size: 16,
                    color: colors.text,
                  ),
                  if (!widget.compact) ...[
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(
                        DshAccountMenuZh.account,
                        textAlign: TextAlign.start,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: DshTypography.body,
                      ),
                    ),
                    const SizedBox(width: 6),
                    DshGlyph(
                      DshIcons.chevronUp.data,
                      size: 14,
                      color: colors.muted,
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class SidebarToolsMenu extends StatelessWidget {
  const SidebarToolsMenu({
    super.key,
    required this.onKnowledge,
    required this.onSchedule,
    this.compact = false,
    this.showSchedule = true,
  });
  final VoidCallback onKnowledge, onSchedule;
  final bool compact;
  final bool showSchedule;
  @override
  Widget build(BuildContext context) => PopupMenuButton<String>(
    key: const Key('open-tools'),
    tooltip: DshAccountMenuZh.tools,
    onSelected: (value) => value == 'knowledge' ? onKnowledge() : onSchedule(),
    itemBuilder: (_) => [
      const PopupMenuItem(
        key: Key('open-knowledge'),
        value: 'knowledge',
        child: Text(DshAccountMenuZh.knowledge),
      ),
      if (showSchedule)
        const PopupMenuItem(
          key: Key('open-schedule'),
          value: 'schedule',
          child: Text(DshAccountMenuZh.schedule),
        ),
    ],
    child: Container(
      constraints: BoxConstraints(
        minHeight: DshTokens.of(context).controlHeight(context),
        minWidth: 36,
      ),
      padding: EdgeInsets.symmetric(horizontal: compact ? 8 : 12),
      decoration: BoxDecoration(
        border: Border.all(color: DshColors(context).border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          DshGlyph(
            DshIcons.wrench.data,
            size: 16,
            color: DshColors(context).text,
          ),
          if (!compact) ...[
            const SizedBox(width: 7),
            const Text(
              DshAccountMenuZh.tools,
              style: TextStyle(fontSize: DshTypography.sizeAuxiliary),
            ),
          ],
        ],
      ),
    ),
  );
}
