import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

class ApprovalCard extends StatefulWidget {
  const ApprovalCard({
    super.key,
    required this.controller,
    required this.frame,
  });
  final DesktopController controller;
  final HostFrame frame;
  @override
  State<ApprovalCard> createState() => _ApprovalCardState();
}

class _ApprovalCardState extends State<ApprovalCard> {
  bool busy = false;
  String? error;
  Future<void> answer(String outcome) async {
    final c = widget.controller, frame = widget.frame;
    if (busy || !c.connected || c.answering.contains(frame.rpcId)) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await c.answer(frame, {
        'sessionId': frame.sessionId,
        'approvalId': frame.payload['approvalId'],
        'outcome': outcome,
      });
    } catch (e) {
      if (mounted) {
        setState(() {
          busy = false;
          error = '$e';
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (_, _) {
      final c = widget.controller,
          data = widget.frame.payload,
          colors = DshColors(context);
      final enabled =
          !busy && c.connected && !c.answering.contains(widget.frame.rpcId);
      final call = data['callId'] == null
          ? null
          : c.window.events
                .where(
                  (event) =>
                      event.data['callId'] == data['callId'] &&
                      event.type == 'tool/call',
                )
                .lastOrNull;
      final args = call?.toolSummaryArguments ?? <String, dynamic>{};
      final command = args['command'] ?? args['file_path'] ?? args['path'];
      final rememberable = data['rememberable'] == true;
      final grant = '${data['grantKey'] ?? ''}';
      final scope = !rememberable
          ? '此请求只支持单次授权'
          : grant.startsWith('write-dir:')
          ? '记忆只覆盖同一目录的写入；其他目录和子目录仍需审批。'
          : grant.startsWith('shell:')
          ? '记忆只覆盖这条命令；其他命令仍需审批。'
          : '仅记住与本次请求匹配的授权范围';
      return Container(
        key: const ValueKey('approval-card'),
        clipBehavior: Clip.antiAlias,
        decoration: BoxDecoration(
          color: colors.base,
          borderRadius: BorderRadius.circular(20),
          border: Border.all(color: const Color(0xffedc46b)),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
              color: colors.dark
                  ? const Color(0xff3b321d)
                  : const Color(0xfffff6df),
              child: Row(
                children: [
                  const DshGlyph(
                    LucideIcons.shieldCheck,
                    size: 16,
                    color: Color(0xffab7300),
                  ),
                  const SizedBox(width: 8),
                  Text(
                    busy ? '正在提交审批…' : '等待审批',
                    style: const TextStyle(
                      fontSize: 13,
                      height: 18 / 13,
                      color: Color(0xffab7300),
                    ),
                  ),
                ],
              ),
            ),
            Flexible(
              child: ConstrainedBox(
                constraints: BoxConstraints(
                  maxHeight: (MediaQuery.sizeOf(context).height * .6 - 140)
                      .clamp(0, 320),
                ),
                child: SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(16, 12, 16, 0),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      SelectableText(
                        displayPathText(
                          '${data['reason'] ?? '${data['toolName']} 请求执行权限'}',
                        ),
                        style: TextStyle(
                          fontSize: 15,
                          height: 24 / 15,
                          fontWeight: FontWeight.w500,
                          color: colors.text,
                        ),
                      ),
                      if (command != null) ...[
                        const SizedBox(height: 6),
                        SelectableText(
                          displayPathText('$command'),
                          style: TextStyle(
                            fontFamily: 'Consolas',
                            fontSize: 13,
                            height: 20 / 13,
                            color: colors.muted,
                          ),
                        ),
                      ],
                      const SizedBox(height: 6),
                      Text(
                        scope,
                        style: TextStyle(
                          fontSize: 13,
                          height: 20 / 13,
                          color: colors.muted,
                        ),
                      ),
                      if (error != null)
                        Text(
                          error!,
                          style: const TextStyle(
                            fontSize: 13,
                            color: Colors.red,
                          ),
                        ),
                    ],
                  ),
                ),
              ),
            ),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
              child: Wrap(
                alignment: WrapAlignment.end,
                spacing: 8,
                runSpacing: 8,
                children: [
                  DshButton(
                    height: 36,
                    pill: true,
                    outline: true,
                    onPressed: enabled ? () => answer('rejected') : null,
                    child: const Text('拒绝'),
                  ),
                  DshButton(
                    height: 36,
                    pill: true,
                    primary: true,
                    onPressed: enabled ? () => answer('allowed-once') : null,
                    child: const Text('允许一次'),
                  ),
                  Tooltip(
                    message: scope,
                    child: DshButton(
                      height: 36,
                      pill: true,
                      primary: true,
                      onPressed: enabled && rememberable
                          ? () => answer('allowed-always')
                          : null,
                      child: const Text('始终允许'),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      );
    },
  );
}
