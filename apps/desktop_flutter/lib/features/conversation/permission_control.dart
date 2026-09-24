import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../src/controller.dart';
import '../../src/conversation.dart' show ComposerAction;
import '../../design/primitives.dart';

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
  String? error;
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
                      ? LucideIcons.eye
                      : option.key == 'danger-full-access'
                      ? LucideIcons.shieldAlert
                      : LucideIcons.shieldCheck,
                  size: 16,
                ),
                const SizedBox(width: 10),
                Text(option.value, style: const TextStyle(fontSize: 14)),
                const SizedBox(width: 16),
                if (value == option.key)
                  const DshGlyph(LucideIcons.check, size: 14),
              ],
            ),
          ),
      ],
    );
    if (!mounted ||
        !current() ||
        picked == null ||
        picked == value ||
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
        throw StateError('${outcome['text'] ?? '访问模式未被接受'}');
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
        setState(() => error = '${accepted ? '模式已提交，状态刷新失败：' : ''}$e');
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(error!)));
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
          ? '工作区内可写，更大范围的操作需要审批'
          : value == 'danger-full-access'
          ? '完整文件访问，无需审批提示'
          : '${option?['description'] ?? ''}';
      return ComposerAction(
        value == 'read-only'
            ? LucideIcons.eye
            : value == 'danger-full-access'
            ? LucideIcons.shieldAlert
            : LucideIcons.shieldCheck,
        key: anchor,
        color: value == 'danger-full-access' || value == 'full-access'
            ? const Color(0xfff97316)
            : null,
        asset:
            [
              'read-only',
              'workspace-write',
              'danger-full-access',
            ].contains(value)
            ? 'assets/icons/web-permission-$value.svg'
            : null,
        label:
            error ??
            (busy
                ? '正在切换访问模式…'
                : '访问模式，当前：${c.permissionChoices[value] ?? permissionName(value)}${description.isEmpty ? '' : ' · $description'}'),
        onPressed:
            busy ||
                !c.connected ||
                c.selectedId == null ||
                c.permissionChoices.isEmpty
            ? null
            : choose,
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
    title: const Text('确认启用 Full access？'),
    content: SizedBox(
      width: 440,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text(
              '启用 Full access 后，agent 将减少确认步骤，并且可以直接执行更多操作，包括敏感操作、文件修改或外部命令。仅建议在你信任当前任务时使用。',
            ),
            const SizedBox(height: 16),
            CheckboxListTile(
              contentPadding: EdgeInsets.zero,
              controlAffinity: ListTileControlAffinity.leading,
              value: acknowledged,
              onChanged: (v) => setState(() => acknowledged = v == true),
              title: const Text('我已了解风险，并愿意继续'),
            ),
          ],
        ),
      ),
    ),
    actions: [
      DshButton(
        onPressed: () => Navigator.pop(context, false),
        child: const Text('取消'),
      ),
      DshButton(
        primary: true,
        onPressed: acknowledged ? () => Navigator.pop(context, true) : null,
        child: const Text('启用 Full access'),
      ),
    ],
  );
}
