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

String accountProviderName(Json provider) =>
    '${provider['name'] ?? provider['id'] ?? ''}';

Json? activeAccount(Json provider) {
  final records = objects(provider['accounts']);
  return records.where((row) => row['active'] == true).firstOrNull ??
      records.firstOrNull;
}

/// Providers the person has connected, those needing a new sign-in first.
/// Providers that were never used stay in the settings catalog.
List<Json> linkedAccountProviders(List<Json> providers) {
  final attention = <Json>[], healthy = <Json>[];
  for (final provider in providers) {
    if (accountNeedsLogin(provider)) {
      attention.add(provider);
    } else if (provider['signedIn'] == true) {
      healthy.add(provider);
    }
  }
  return [...attention, ...healthy];
}

/// The sidebar account entry. With nothing connected it opens settings
/// directly; otherwise it anchors a short panel above itself that lists only
/// connected providers and offers a direct re-login where one is needed.
class AccountConnectionMenu extends StatefulWidget {
  const AccountConnectionMenu({
    super.key,
    required this.controller,
    required this.onManageAccounts,
    required this.onModels,
    this.onLogin,
    this.compact = false,
  });
  final DesktopController controller;

  /// Opens subscription settings, focused on one provider when given.
  final ValueChanged<String?> onManageAccounts;
  final VoidCallback onModels;

  /// Starts a new sign-in for one provider; settings open when absent.
  final ValueChanged<String>? onLogin;
  final bool compact;
  @override
  State<AccountConnectionMenu> createState() => _AccountConnectionMenuState();
}

class _AccountConnectionMenuState extends State<AccountConnectionMenu> {
  final popover = ShadPopoverController();
  final trigger = FocusNode(debugLabel: 'account-trigger');
  final panelFocus = FocusNode(debugLabel: 'account-panel');
  Object? openedScope;
  bool keyboardActive = false;
  double triggerWidth = 0;

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
    popover.addListener(syncKeyboard);
  }

  @override
  void didUpdateWidget(AccountConnectionMenu oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, c)) {
      oldWidget.controller.removeListener(changed);
      c.addListener(changed);
      changed();
    }
  }

  @override
  void dispose() {
    c.removeListener(changed);
    popover.removeListener(syncKeyboard);
    if (keyboardActive) {
      FocusManager.instance.removeEarlyKeyEventHandler(handleKey);
    }
    popover.dispose();
    trigger.dispose();
    panelFocus.dispose();
    super.dispose();
  }

  void changed() {
    if (!mounted) return;
    // Rows belong to the connection and accounts they were opened for.
    if (popover.isOpen && openedScope != scope) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) popover.hide();
      });
    }
    setState(() {});
  }

  void syncKeyboard() {
    if (popover.isOpen == keyboardActive) return;
    keyboardActive = popover.isOpen;
    if (keyboardActive) {
      FocusManager.instance.addEarlyKeyEventHandler(handleKey);
    } else {
      FocusManager.instance.removeEarlyKeyEventHandler(handleKey);
    }
  }

  KeyEventResult handleKey(KeyEvent event) {
    if (!mounted ||
        !popover.isOpen ||
        event is! KeyDownEvent ||
        event.logicalKey != LogicalKeyboardKey.escape ||
        ModalRoute.of(context)?.isCurrent == false) {
      return KeyEventResult.ignored;
    }
    close();
    return KeyEventResult.handled;
  }

  void close({bool restoreFocus = true}) {
    popover.hide();
    if (!restoreFocus) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && ModalRoute.of(context)?.isCurrent != false) {
        trigger.requestFocus();
      }
    });
  }

  void toggle() {
    if (popover.isOpen) {
      close();
      return;
    }
    if (linkedAccountProviders(c.subscriptionAccounts).isEmpty) {
      widget.onManageAccounts(null);
      return;
    }
    openedScope = scope;
    popover.show();
  }

  /// Runs a panel action only for the accounts the panel was opened with.
  void act(VoidCallback action) {
    if (!popover.isOpen || openedScope != scope) return;
    close(restoreFocus: false);
    action();
  }

  Widget row({
    required Key key,
    required Widget child,
    required VoidCallback onTap,
    String? tooltip,
  }) {
    final colors = DshColors(context);
    final radius = BorderRadius.circular(DshTokens.of(context).radiusControl);
    final body = Material(
      color: Colors.transparent,
      borderRadius: radius,
      child: InkWell(
        key: key,
        onTap: onTap,
        borderRadius: radius,
        hoverColor: colors.hover,
        focusColor: colors.hover,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
          child: child,
        ),
      ),
    );
    return tooltip == null ? body : DshTooltip(message: tooltip, child: body);
  }

  Widget providerRow(Json provider) {
    final colors = DshColors(context);
    final tokens = DshTokens.of(context);
    final id = '${provider['id'] ?? ''}';
    final name = accountProviderName(provider);
    final attention = accountNeedsLogin(provider);
    final label = maskedAccountLabel(activeAccount(provider)?['label']);
    return row(
      key: ValueKey('account-provider-$id'),
      tooltip: DshAccountMenuZh.providerDetails(name),
      onTap: () => act(() => widget.onManageAccounts(id)),
      child: Row(
        children: [
          _ProviderMark(name: name),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: DshTypography.body.copyWith(color: colors.text),
                ),
                Text(
                  label.isEmpty
                      ? (attention
                            ? DshAccountMenuZh.expired
                            : DshAccountMenuZh.connected)
                      : label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: DshTypography.caption.copyWith(color: colors.muted),
                ),
              ],
            ),
          ),
          const SizedBox(width: 8),
          if (attention)
            DshButton(
              key: ValueKey('account-relogin-$id'),
              outline: true,
              height: 28,
              fontSize: DshTypography.sizeCaption,
              padding: const EdgeInsets.symmetric(horizontal: 8),
              tooltip: DshAccountMenuZh.reloginProvider(name),
              onPressed: () => act(() {
                final login = widget.onLogin;
                if (login == null) {
                  widget.onManageAccounts(id);
                } else {
                  login(id);
                }
              }),
              child: Text(
                DshAccountMenuZh.relogin,
                style: TextStyle(color: tokens.warning.foreground),
              ),
            )
          else
            Semantics(
              label: DshAccountMenuZh.connected,
              child: _StatusDot(color: tokens.success.foreground),
            ),
        ],
      ),
    );
  }

  Widget actionRow(Key key, IconData icon, String title, VoidCallback onTap) {
    final colors = DshColors(context);
    return row(
      key: key,
      onTap: () => act(onTap),
      child: Row(
        children: [
          SizedBox(
            width: 28,
            child: DshGlyph(icon, size: 16, color: colors.muted),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: DshTypography.body.copyWith(color: colors.text),
            ),
          ),
        ],
      ),
    );
  }

  Widget panel(BuildContext context) {
    if (!popover.isOpen || openedScope != scope) {
      return const SizedBox.shrink();
    }
    final colors = DshColors(context);
    final size = MediaQuery.sizeOf(context);
    final linked = linkedAccountProviders(c.subscriptionAccounts);
    // Large text widens the panel before names start to truncate.
    final scale = MediaQuery.textScalerOf(context).scale(1).clamp(1.0, 1.4);
    final width = math.min(
      math.max(120.0, size.width - 24),
      math
          .max(widget.compact ? 280.0 : triggerWidth, 248.0 * scale)
          .clamp(0, 360),
    );
    return DefaultTextStyle(
      style: DshTypography.body.copyWith(color: colors.text),
      child: Shortcuts(
        shortcuts: const {
          SingleActivator(LogicalKeyboardKey.arrowDown): NextFocusIntent(),
          SingleActivator(LogicalKeyboardKey.arrowUp): PreviousFocusIntent(),
        },
        child: Focus(
          focusNode: panelFocus,
          autofocus: true,
          child: FocusTraversalGroup(
            child: ConstrainedBox(
              constraints: BoxConstraints(
                maxHeight: math.max(160, math.min(440, size.height - 120)),
              ),
              child: SizedBox(
                key: const ValueKey('account-menu-panel'),
                width: width.toDouble(),
                child: SingleChildScrollView(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Padding(
                        padding: const EdgeInsets.fromLTRB(8, 4, 8, 4),
                        child: Text(
                          DshAccountMenuZh.subscription,
                          style: DshTypography.caption.copyWith(
                            color: colors.muted,
                          ),
                        ),
                      ),
                      for (final provider in linked) providerRow(provider),
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 4),
                        child: Divider(height: 1, color: colors.border),
                      ),
                      actionRow(
                        const ValueKey('account-menu-manage'),
                        DshIcons.users.data,
                        DshAccountMenuZh.manageAccounts,
                        () => widget.onManageAccounts(null),
                      ),
                      actionRow(
                        const ValueKey('account-menu-models'),
                        DshIcons.database.data,
                        DshAccountMenuZh.apiModels,
                        widget.onModels,
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget triggerButton(BuildContext context) {
    final colors = DshColors(context);
    final tokens = DshTokens.of(context);
    final linked = linkedAccountProviders(c.subscriptionAccounts);
    final attention = linked.where(accountNeedsLogin).length;
    final dot = linked.isEmpty
        ? null
        : attention > 0
        ? tokens.warning.foreground
        : tokens.success.foreground;
    final tooltip = linked.isEmpty
        ? DshAccountMenuZh.signInHint
        : '${DshAccountMenuZh.subscription}：'
              '${DshAccountMenuZh.summary(linked.length, attention)}';
    final mark = SizedBox(
      width: 20,
      height: 20,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Center(
            child: DshGlyph(
              DshIcons.users.data,
              size: 16,
              color: linked.isEmpty ? colors.muted : colors.text,
            ),
          ),
          if (dot != null)
            Positioned(
              right: -1,
              top: -1,
              child: _StatusDot(color: dot, ring: colors.sidebar),
            ),
        ],
      ),
    );
    if (widget.compact) {
      // Icon-only like the other rail buttons; text scale does not grow it.
      return Semantics(
        label: tooltip,
        button: true,
        child: SizedBox.square(
          key: const ValueKey('account-connection-menu'),
          dimension: 36,
          child: DshButton(
            focusNode: trigger,
            tooltip: tooltip,
            width: 36,
            height: 36,
            padding: EdgeInsets.zero,
            active: popover.isOpen,
            onPressed: toggle,
            child: mark,
          ),
        ),
      );
    }
    final title = linked.isEmpty
        ? DshAccountMenuZh.signIn
        : linked.length == 1
        ? accountProviderName(linked.single)
        : DshAccountMenuZh.subscription;
    // Large text keeps the title; the status dot and tooltip still carry
    // the sign-in warning.
    final roomy = MediaQuery.textScalerOf(context).scale(1) < 1.5;
    final Widget? trailing = attention > 0 && roomy
        ? Text(
            DshAccountMenuZh.expired,
            maxLines: 1,
            style: DshTypography.caption.copyWith(
              color: tokens.warning.foreground,
            ),
          )
        : linked.length > 1
        ? _CountBadge(count: linked.length)
        : linked.isNotEmpty
        ? DshGlyph(DshIcons.chevronUp.data, size: 14, color: colors.muted)
        : null;
    return Semantics(
      label: tooltip,
      button: true,
      child: DshButton(
        key: const ValueKey('account-connection-menu'),
        focusNode: trigger,
        tooltip: tooltip,
        width: double.infinity,
        padding: const EdgeInsets.symmetric(horizontal: 10),
        active: popover.isOpen,
        onPressed: toggle,
        child: Expanded(
          child: Row(
            children: [
              mark,
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  title,
                  key: const ValueKey('account-connection-title'),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  textAlign: TextAlign.start,
                  style: DshTypography.body.copyWith(
                    color: linked.isEmpty ? colors.muted : colors.text,
                  ),
                ),
              ),
              if (trailing != null) ...[const SizedBox(width: 6), trailing],
            ],
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      if (constraints.maxWidth.isFinite) triggerWidth = constraints.maxWidth;
      return ShadPopover(
        controller: popover,
        padding: const EdgeInsets.all(6),
        // Above the trigger, aligned with its left edge; never covering it.
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
        child: triggerButton(context),
      );
    },
  );
}

class _ProviderMark extends StatelessWidget {
  const _ProviderMark({required this.name});
  final String name;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final initial = name.trim().isEmpty
        ? '?'
        : name.trim().characters.first.toUpperCase();
    return Container(
      width: 28,
      height: 28,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: colors.layer,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: colors.border),
      ),
      // A decorative initial; the provider name beside it carries the text.
      child: ExcludeSemantics(
        child: Text(
          initial,
          textScaler: TextScaler.noScaling,
          style: DshTypography.auxiliary.copyWith(
            color: colors.text,
            fontWeight: FontWeight.w600,
            height: 1,
          ),
        ),
      ),
    );
  }
}

class _CountBadge extends StatelessWidget {
  const _CountBadge({required this.count});
  final int count;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Container(
      key: const ValueKey('account-connection-count'),
      constraints: const BoxConstraints(minWidth: 20),
      padding: const EdgeInsets.symmetric(horizontal: 6),
      decoration: BoxDecoration(
        color: colors.layer,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: colors.border),
      ),
      child: Text(
        '$count',
        textAlign: TextAlign.center,
        style: DshTypography.caption.copyWith(
          color: colors.muted,
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      ),
    );
  }
}

class _StatusDot extends StatelessWidget {
  const _StatusDot({required this.color, this.ring});
  final Color color;
  final Color? ring;
  @override
  Widget build(BuildContext context) => Container(
    width: ring == null ? 8 : 9,
    height: ring == null ? 8 : 9,
    decoration: BoxDecoration(
      color: color,
      shape: BoxShape.circle,
      border: ring == null ? null : Border.all(color: ring!, width: 1.5),
    ),
  );
}
