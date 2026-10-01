import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/text_document.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

class ToolMessage extends StatefulWidget {
  const ToolMessage({
    super.key,
    required this.item,
    this.cwd,
    this.onDetails,
    this.onOpenPath,
    this.onOpenPlan,
    this.hintDisplay = 'both',
  });
  final TranscriptItem item;
  final String? cwd;
  final VoidCallback? onDetails;
  final ValueChanged<String>? onOpenPath;
  final ValueChanged<TranscriptItem>? onOpenPlan;
  final String hintDisplay;
  @override
  State<ToolMessage> createState() => _ToolMessageState();
}

class _ToolMessageState extends State<ToolMessage> {
  bool expanded = false;
  String? input, output;
  String? previewError;
  @override
  void didUpdateWidget(ToolMessage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.item.id != widget.item.id) expanded = false;
    if (oldWidget.item.text != widget.item.text) input = null;
    if (oldWidget.item.output != widget.item.output) output = null;
  }

  String display(String text) {
    if (text.length > 65536) return text;
    try {
      return const JsonEncoder.withIndent('  ').convert(jsonDecode(text));
    } on FormatException {
      return text;
    }
  }

  void toggle() => setState(() {
    expanded = !expanded;
    if (!expanded) {
      input = null;
      output = null;
    }
  });

  @override
  Widget build(BuildContext context) {
    final item = widget.item, colors = DshColors(context);
    if (item.planText != null) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(
            spacing: 10,
            runSpacing: 6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text(
                item.title.isEmpty ? DshConversationZh.plan : item.title,
                style: const TextStyle(fontWeight: FontWeight.w700),
              ),
              DshButton(
                key: ValueKey('preview-plan-${item.id}'),
                outline: true,
                height: 36,
                padding: const EdgeInsets.symmetric(horizontal: 10),
                onPressed: widget.onOpenPlan == null
                    ? null
                    : () {
                        try {
                          widget.onOpenPlan!(item);
                          setState(() => previewError = null);
                        } catch (e) {
                          setState(() => previewError = '$e');
                        }
                      },
                child: const Text(DshConversationZh.previewPlan),
              ),
              Text(
                DshConversationZh.planApprovalHint,
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: colors.muted,
                ),
              ),
            ],
          ),
          if (previewError != null)
            Text(
              previewError!,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
        ],
      );
    }
    final failed = item.status == 'failed',
        stopped = item.status == 'interrupted';
    final color = failed ? Theme.of(context).colorScheme.error : colors.muted;
    final path = item.filePath;
    final summary = path == null
        ? displayPathText(item.summary)
        : toolPathLabel(path, widget.cwd);
    final icon = switch (item.iconKind) {
      'question' => DshIcons.circleHelp.data,
      'read' => DshIcons.fileText.data,
      'read_image' => DshIcons.image.data,
      'read_video' => DshIcons.video.data,
      'write' || 'edit' => DshIcons.pencil.data,
      'search' => DshIcons.search.data,
      'todo' => DshIcons.listChecks.data,
      _ => DshIcons.squareTerminal.data,
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        Semantics(
          button: true,
          expanded: expanded,
          child: InkWell(
            key: ValueKey('tool-row-${item.id}'),
            borderRadius: BorderRadius.circular(6),
            onTap: toggle,
            child: SizedBox(
              height: 24,
              child: Row(
                children: [
                  if (widget.hintDisplay != 'text') ...[
                    DshGlyph(
                      failed
                          ? DshIcons.circleAlert.data
                          : expanded
                          ? DshIcons.chevronDown.data
                          : icon,
                      size: 14,
                      color: color,
                      asset: item.iconKind == 'todo' && !failed && !expanded
                          ? 'assets/icons/task-list.svg'
                          : item.iconKind == 'generic' && !failed && !expanded
                          ? 'assets/icons/web-IconSparkle16.svg'
                          : null,
                    ),
                    const SizedBox(width: 8),
                  ],
                  if (widget.hintDisplay != 'icons')
                    Flexible(
                      flex: 0,
                      child: Text(
                        item.title.isEmpty
                            ? DshConversationZh.toolResult
                            : item.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: DshTypography.sizeBody,
                          height: 24 / 14,
                          color: color,
                        ),
                      ),
                    ),
                  if (summary.isNotEmpty) ...[
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      child: Text('·', style: TextStyle(color: colors.muted)),
                    ),
                    Expanded(
                      child: path != null && widget.onOpenPath != null
                          ? Tooltip(
                              message: displayPath(path),
                              child: InkWell(
                                key: ValueKey('tool-file-${item.id}'),
                                onTap: () => widget.onOpenPath!(path),
                                child: Text(
                                  summary,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                    fontSize: DshTypography.sizeBody,
                                    height: 24 / 14,
                                    color: colors.muted,
                                    decoration: TextDecoration.underline,
                                  ),
                                ),
                              ),
                            )
                          : Text(
                              summary,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontSize: DshTypography.sizeBody,
                                height: 24 / 14,
                                color: colors.muted,
                              ),
                            ),
                    ),
                  ],
                  if (stopped &&
                      !item.title.contains(DshConversationZh.stop) &&
                      item.summary != DshConversationZh.interrupted)
                    Padding(
                      padding: const EdgeInsets.only(left: 8),
                      child: Text(
                        DshConversationZh.stopped,
                        style: TextStyle(
                          fontSize: DshTypography.sizeCaption,
                          color: colors.muted,
                        ),
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
        if (expanded)
          Container(
            key: ValueKey('tool-body-${item.id}'),
            margin: const EdgeInsets.only(top: 6, bottom: 8),
            decoration: BoxDecoration(
              color: colors.layer,
              borderRadius: BorderRadius.circular(12),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              mainAxisSize: MainAxisSize.min,
              children: [
                SizedBox(
                  height: 280,
                  child: TextDocument(
                    sections: [
                      if (item.text.isNotEmpty)
                        (
                          title: DshConversationZh.input,
                          text: input ??= display(item.text),
                        ),
                      if (item.output.isNotEmpty)
                        (
                          title: DshConversationZh.output,
                          text: output ??= display(item.output),
                        ),
                      if (item.output.isEmpty)
                        (
                          title: DshConversationZh.output,
                          text: item.status == 'pending'
                              ? DshConversationZh.runningProgress
                              : DshConversationZh.noOutput,
                        ),
                    ],
                  ),
                ),
                if (widget.onDetails != null)
                  Align(
                    alignment: Alignment.centerRight,
                    child: DshButton(
                      height: 28,
                      onPressed: widget.onDetails,
                      child: const Text(DshConversationZh.callDetails),
                    ),
                  ),
              ],
            ),
          ),
      ],
    );
  }
}
