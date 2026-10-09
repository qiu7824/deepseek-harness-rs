import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../src/controller.dart';
import '../../src/conversation.dart' show ComposerAction;
import '../../design/primitives.dart';
import '../../design/error.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

class PermissionControl extends StatefulWidget {
  const PermissionControl({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<PermissionControl> createState() => _PermissionControlState();
}

class _PermissionControlState extends State<PermissionControl> {
  final anchor = GlobalKey();
  RequestScope? request;
  bool busy = false;
  Object? error;
  DesktopController get c => widget.controller;
  @override
  void dispose() {
    request?.cancel();
    super.dispose();
  }

  Future<void> choose() async {
    final api = c.client,
        session = c.selectedId,
        revision = c.selectionRevision;
    if (api == null || session == null || busy || !c.connected) return;
    Tooltip.dismissAllToolTips();
    bool current() =>
        mounted &&
        c.client == api &&
        c.selectedId == session &&
        c.selectionRevision == revision;
    final value = object(c.projections['permissions'])['currentValue'];
    final options = c.permissionChoices;
    final box = anchor.currentContext!.findRenderObject()! as RenderBox;
    final overlay =
        Overlay.of(context).context.findRenderObject()! as RenderBox;
    final point = box.localToGlobal(Offset.zero, ancestor: overlay);
    final picked = await showMenu<String>(
      context: context,
      position: RelativeRect.fromRect(
        Rect.fromLTWH(
          point.dx,
          point.dy - options.length * 40,
          box.size.width,
          0,
        ),
        Offset.zero & overlay.size,
      ),
      items: [
        for (final option in options.entries.where((e) => e.key != 'custom'))
          PopupMenuItem(
            value: option.key,
            height: 40,
            child: Row(
              children: [
                DshGlyph(
                  option.key == 'read-only'
                      ? DshIcons.eye.data
                      : option.key == 'danger-full-access' ||
                            option.key == 'full-access'
                      ? DshIcons.shieldAlert.data
                      : DshIcons.shieldCheck.data,
                  size: 16,
                ),
                const SizedBox(width: 10),
                Text(
                  option.value,
                  style: const TextStyle(fontSize: DshTypography.sizeBody),
                ),
                const SizedBox(width: 16),
                if (value == option.key)
                  DshGlyph(DshIcons.check.data, size: 14),
              ],
            ),
          ),
      ],
    );
    if (!mounted ||
        !current() ||
        picked == null ||
        !c.permissionChoices.containsKey(picked) ||
        picked == object(c.projections['permissions'])['currentValue'] ||
        !c.connected) {
      return;
    }
    if (picked == 'danger-full-access' || picked == 'full-access') {
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (_) => const FullAccessConfirmation(),
      );
      if (confirmed != true || !current() || !c.connected) return;
    }
    setState(() {
      busy = true;
      error = null;
    });
    final scope = request = RequestScope();
    var accepted = false;
    try {
      final result = await api.rpc(
        'commands.execute',
        payload: {
          'args': {'agentId': session, 'line': '/permission $picked'},
        },
        mutation: true,
        scope: scope,
      );
      if (!current()) return;
      final outcome = object(result['result']);
      if (outcome['kind'] != 'success') {
        throw StateError(
          '${outcome['text'] ?? DshConversationZh.accessModeRejected}',
        );
      }
      accepted = true;
      final version = c.projectionWindow.version;
      final page = await api.rpc(
        'session.history',
        payload: {'sessionId': session, 'maxMessages': 1},
        scope: scope,
      );
      if (!current()) return;
      c.projectionWindow.snapshot(
        object(page['projections']),
        requestVersion: version,
      );
      c.projectionChanges.value++;
    } catch (e) {
      if (mounted && current()) {
        final described = DshError.describe(e);
        setState(
          () => error = accepted
              ? DshError(
                  title: described.title,
                  message:
                      '${DshConversationZh.accessRefreshFailedPrefix}\n${described.message}',
                  details: described.details,
                  code: described.code,
                  cancelled: described.cancelled,
                  outcomeUnknown: described.outcomeUnknown,
                )
              : e,
        );
        showDshError(context, error!);
      }
    } finally {
      scope.cancel();
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: c.projectionChanges,
    builder: (_, _) {
      if (c.permissionChoices.isEmpty) return const SizedBox.shrink();
      final value =
          '${object(c.projections['permissions'])['currentValue'] ?? ''}';
      final option = objects(object(c.projections['permissions'])['options'])
          .where((o) => o['value'] == value)
          .firstOrNull;
      final description = value == 'workspace-write'
          ? DshConversationZh.workspaceAccessHint
          : value == 'danger-full-access' || value == 'full-access'
          ? DshConversationZh.fullAccessHint
          : value == 'read-only'
          ? '只读取文件，不写入工作区'
          : '${option?['description'] ?? ''}';
      final label = error != null
          ? DshError.describe(error!).message
          : (busy
                ? DshConversationZh.changingAccessMode
                : DshConversationZh.accessModeLabel(
                    mode: c.permissionChoices[value] ?? permissionName(value),
                    details: description.isEmpty ? '' : ' · $description',
                  ));
      return KeyedSubtree(
        key: const ValueKey('permission-mode-control'),
        child: ComposerAction(
          value == 'read-only'
              ? DshIcons.eye.data
              : value == 'danger-full-access' || value == 'full-access'
              ? DshIcons.shieldAlert.data
              : DshIcons.shieldCheck.data,
          key: anchor,
          color: value == 'danger-full-access' || value == 'full-access'
              ? const Color(0xfff97316)
              : null,
          asset: value == 'full-access'
              ? 'assets/icons/web-permission-danger-full-access.svg'
              : [
                  'read-only',
                  'workspace-write',
                  'danger-full-access',
                ].contains(value)
              ? 'assets/icons/web-permission-$value.svg'
              : null,
          label: label,
          onPressed:
              busy ||
                  !c.connected ||
                  c.selectedId == null ||
                  c.permissionChoices.isEmpty
              ? null
              : choose,
        ),
      );
    },
  );
}

class FullAccessConfirmation extends StatefulWidget {
  const FullAccessConfirmation({super.key});
  @override
  State<FullAccessConfirmation> createState() => _FullAccessConfirmationState();
}

class _FullAccessConfirmationState extends State<FullAccessConfirmation> {
  bool acknowledged = false;
  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text(DshConversationZh.confirmFullAccess),
    content: SizedBox(
      width: 440,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text(DshConversationZh.fullAccessRiskHint),
            const SizedBox(height: 16),
            CheckboxListTile(
              contentPadding: EdgeInsets.zero,
              controlAffinity: ListTileControlAffinity.leading,
              value: acknowledged,
              onChanged: (v) => setState(() => acknowledged = v == true),
              title: const Text(DshConversationZh.acknowledgeFullAccess),
            ),
          ],
        ),
      ),
    ),
    actions: [
      DshButton(
        onPressed: () => Navigator.pop(context, false),
        child: const Text(DshConversationZh.cancel),
      ),
      DshButton(
        primary: true,
        onPressed: acknowledged ? () => Navigator.pop(context, true) : null,
        child: const Text(DshConversationZh.enableFullAccess),
      ),
    ],
  );
}
