import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../src/controller.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

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
  Object? error;
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
          error = e;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller.interactionChanges,
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
          ? DshConversationZh.approvalOnceOnly
          : grant.startsWith('write-dir:')
          ? DshConversationZh.approvalDirectoryScope
          : grant.startsWith('shell:')
          ? DshConversationZh.approvalCommandScope
          : DshConversationZh.approvalMatchingScope;
      return Container(
        key: const ValueKey('approval-card'),
        clipBehavior: Clip.antiAlias,
        decoration: BoxDecoration(
          color: colors.base,
          borderRadius: BorderRadius.circular(colors.tokens.radiusCard),
          border: Border.all(color: colors.tokens.warning.border),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
              color: colors.tokens.warning.background,
              child: Row(
                children: [
                  DshGlyph(
                    DshIcons.shieldCheck.data,
                    size: 16,
                    color: colors.warning,
                  ),
                  const SizedBox(width: 8),
                  Text(
                    busy
                        ? DshConversationZh.submittingApproval
                        : DshConversationZh.awaitingApproval,
                    style: TextStyle(
                      fontSize: DshTypography.sizeAuxiliary,
                      height: 18 / 13,
                      color: colors.warning,
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
                          DshConversationZh.approvalReason(
                            reason: data['reason'],
                            tool: data['toolName'],
                          ),
                        ),
                        style: TextStyle(
                          fontSize: DshTypography.sizeConversation,
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
                            fontFamily: DshTypography.monospaceFamily,
                            fontFamilyFallback: DshTypography.monospaceFallback,
                            fontSize: DshTypography.sizeAuxiliary,
                            height: 20 / 13,
                            color: colors.muted,
                          ),
                        ),
                      ],
                      const SizedBox(height: 6),
                      Text(
                        scope,
                        style: TextStyle(
                          fontSize: DshTypography.sizeAuxiliary,
                          height: 20 / 13,
                          color: colors.muted,
                        ),
                      ),
                      if (error != null) DshErrorView(error: error!),
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
                    child: const Text(DshConversationZh.deny),
                  ),
                  DshButton(
                    height: 36,
                    pill: true,
                    primary: true,
                    onPressed: enabled ? () => answer('allowed-once') : null,
                    child: const Text(DshConversationZh.allowOnce),
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
                      child: const Text(DshConversationZh.allowAlways),
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
