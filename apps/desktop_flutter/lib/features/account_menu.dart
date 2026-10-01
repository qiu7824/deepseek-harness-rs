import 'dart:math' as math;
import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../design/primitives.dart';
import '../design/typography.dart';
import '../l10n/account_menu_zh.dart';
import '../src/controller.dart';

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
  final bool compact;
  final bool onlyWhenAuthorized, showSettingsAction;
  @override
  State<AccountConnectionMenu> createState() => _AccountConnectionMenuState();
}

class _AccountConnectionMenuState extends State<AccountConnectionMenu> {
  Object? menuScope;
  Object get scope => (
    widget.controller,
    widget.controller.client,
    widget.controller.host,
    jsonEncode([
      for (final provider in widget.controller.subscriptionAccounts)
        [
          provider['id'],
          provider['signedIn'],
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
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (context, _) {
      final colors = DshColors(context);
      final providers = widget.controller.subscriptionAccounts;
      final authorized = providers
          .where((provider) => provider['signedIn'] == true)
          .length;
      final needsLogin = providers.any(accountNeedsLogin);
      if (widget.onlyWhenAuthorized && authorized == 0 && !needsLogin) {
        return const SizedBox.shrink();
      }
      final accountDetails = [
        DshAccountMenuZh.title,
        for (final provider in providers)
          [
            '${provider['name'] ?? provider['id'] ?? ''}',
            maskedAccountLabel(
              (objects(provider['accounts'])
                      .where((row) => row['active'] == true)
                      .firstOrNull ??
                  objects(provider['accounts']).firstOrNull)?['label'],
            ),
            accountNeedsLogin(provider)
                ? DshAccountMenuZh.expired
                : provider['signedIn'] == true
                ? DshAccountMenuZh.authorized
                : DshAccountMenuZh.notAuthorized,
          ].where((part) => part.isNotEmpty).join(' · '),
      ].join('\n');
      return DshTooltip(
        message: accountDetails,
        child: PopupMenuButton<String>(
          key: const ValueKey('account-connection-menu'),
          tooltip: '',
          borderRadius: BorderRadius.circular(
            DshTokens.of(context).radiusControl,
          ),
          position: PopupMenuPosition.over,
          constraints: BoxConstraints(
            maxWidth: math.min(380, MediaQuery.sizeOf(context).width - 32),
          ),
          onOpened: () {
            Tooltip.dismissAllToolTips();
            menuScope = scope;
          },
          onSelected: (value) {
            if (menuScope != scope) return;
            switch (value) {
              case 'accounts':
                widget.onAccounts();
              case 'models':
                widget.onModels();
              case 'settings':
                widget.onSettings();
            }
          },
          itemBuilder: (context) {
            final capturedScope = scope;
            return [
              PopupMenuItem<String>(
                enabled: false,
                child: Text(
                  DshAccountMenuZh.subscription,
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
                ),
              ),
              if (providers.isEmpty)
                const PopupMenuItem<String>(
                  enabled: false,
                  child: Text(DshAccountMenuZh.noAccounts),
                ),
              for (final provider in providers)
                PopupMenuItem<String>(
                  enabled: false,
                  child: ListenableBuilder(
                    listenable: widget.controller,
                    builder: (context, _) {
                      if (capturedScope != scope) {
                        return const Text(DshAccountMenuZh.changed);
                      }
                      final records = objects(provider['accounts']);
                      final active = records
                          .where((row) => row['active'] == true)
                          .firstOrNull;
                      final label = maskedAccountLabel(
                        (active ?? records.firstOrNull)?['label'],
                      );
                      final state = accountNeedsLogin(provider)
                          ? DshAccountMenuZh.expired
                          : provider['signedIn'] == true
                          ? DshAccountMenuZh.authorized
                          : DshAccountMenuZh.notAuthorized;
                      return Padding(
                        padding: const EdgeInsets.symmetric(vertical: 6),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              '${provider['name'] ?? provider['id'] ?? ''}',
                              style: TextStyle(
                                color: colors.text,
                                fontSize: DshTypography.sizeBody,
                              ),
                            ),
                            const SizedBox(height: 3),
                            Text(
                              [if (label.isNotEmpty) label, state].join(' · '),
                              style: TextStyle(
                                color: colors.muted,
                                fontSize: DshTypography.sizeCaption,
                              ),
                            ),
                          ],
                        ),
                      );
                    },
                  ),
                ),
              const PopupMenuDivider(),
              const PopupMenuItem(
                value: 'accounts',
                child: Text(DshAccountMenuZh.manageAccounts),
              ),
              const PopupMenuItem(
                value: 'models',
                child: Text(DshAccountMenuZh.apiModels),
              ),
              if (widget.showSettingsAction) const PopupMenuDivider(),
              if (widget.showSettingsAction)
                PopupMenuItem(
                  value: 'settings',
                  child: Wrap(
                    spacing: 16,
                    runSpacing: 4,
                    children: [
                      const Text(DshAccountMenuZh.settings),
                      Text(
                        widget.settingsShortcut,
                        style: TextStyle(
                          color: colors.muted,
                          fontSize: DshTypography.sizeCaption,
                        ),
                      ),
                    ],
                  ),
                ),
            ];
          },
          child: SizedBox(
            width: widget.compact ? DshTokens.of(context).controlMinimum : null,
            height: widget.compact
                ? DshTokens.of(context).controlMinimum
                : DshTokens.of(context).controlHeight(context),
            child: Padding(
              padding: EdgeInsets.symmetric(
                horizontal: widget.compact ? 6 : 12,
              ),
              child: Row(
                mainAxisSize: widget.compact
                    ? MainAxisSize.min
                    : MainAxisSize.max,
                children: [
                  DshGlyph(
                    DshIcons.users.data,
                    size: 16,
                    color: needsLogin ? colors.warning : colors.text,
                  ),
                  if (!widget.compact) ...[
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(
                        DshAccountMenuZh.account,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: DshTypography.body.copyWith(color: colors.text),
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
      );
    },
  );
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
