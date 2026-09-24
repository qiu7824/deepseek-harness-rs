import 'dart:async';

import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import 'feedback_controller.dart';

class MessageFeedbackActions extends StatefulWidget {
  const MessageFeedbackActions({
    super.key,
    required this.controller,
    required this.messageId,
  });
  final MessageFeedbackController controller;
  final String messageId;
  @override
  State<MessageFeedbackActions> createState() => _MessageFeedbackActionsState();
}

class _MessageFeedbackActionsState extends State<MessageFeedbackActions> {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) unawaited(widget.controller.ensure());
    });
  }

  Future<void> choose(String rating, {bool edit = false}) async {
    final c = widget.controller, id = widget.messageId;
    if (c.busy || !await c.ensure() || !mounted) return;
    final row = c.item(id);
    if (!edit && row?['rating'] == rating) {
      await c.save(id, null, ifVersion: row?['version'] as String?);
    } else {
      await showDialog<void>(
        context: context,
        builder: (_) => _RatingDialog(
          controller: c,
          messageId: id,
          rating: rating,
          version: row?['version'] as String?,
          note: edit ? '${row?['note'] ?? ''}' : '',
          category: edit ? '${row?['category'] ?? ''}' : '',
        ),
      );
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller,
    builder: (context, _) {
      final c = widget.controller, row = c.item(widget.messageId);
      return Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final positive in [true, false]) ...[
            if (!positive) const SizedBox(width: 10),
            DshIcon(
              positive ? LucideIcons.thumbsUp : LucideIcons.thumbsDown,
              label: row?['rating'] == (positive ? 'positive' : 'negative')
                  ? '取消标记'
                  : positive
                  ? '好的回答'
                  : '有问题的回答',
              size: 28,
              active: row?['rating'] == (positive ? 'positive' : 'negative'),
              onPressed: c.busy
                  ? null
                  : () => choose(positive ? 'positive' : 'negative'),
            ),
          ],
          if (row != null)
            DshIcon(
              LucideIcons.pencil,
              label: '补充说明',
              size: 28,
              onPressed: c.busy
                  ? null
                  : () => choose('${row['rating']}', edit: true),
            ),
          if (c.error != null)
            DshIcon(
              LucideIcons.circleAlert,
              label: '${c.error}\n点击重新读取反馈',
              size: 28,
              color: Theme.of(context).colorScheme.error,
              onPressed: c.busy ? null : () => c.ensure(refresh: true),
            ),
        ],
      );
    },
  );
}

class _RatingDialog extends StatefulWidget {
  const _RatingDialog({
    required this.controller,
    required this.messageId,
    required this.rating,
    required this.version,
    required this.note,
    required this.category,
  });
  final MessageFeedbackController controller;
  final String messageId, rating, note, category;
  final String? version;
  @override
  State<_RatingDialog> createState() => _RatingDialogState();
}

class _RatingDialogState extends State<_RatingDialog> {
  late final note = TextEditingController(text: widget.note);
  late String category = widget.category;
  late String? version = widget.version;
  bool busy = false;
  String? error;
  @override
  void dispose() {
    note.dispose();
    super.dispose();
  }

  Future<void> save() async {
    if (busy) return;
    setState(() {
      busy = true;
      error = null;
    });
    final c = widget.controller;
    final ok = await c.save(
      widget.messageId,
      widget.rating,
      ifVersion: version,
      note: note.text,
      category: category,
    );
    if (!mounted) return;
    if (ok) {
      Navigator.pop(context);
      return;
    }
    setState(() {
      busy = false;
      error = c.error ?? '反馈未保存，请重新读取状态后重试。';
      version = c.item(widget.messageId)?['version'] as String?;
    });
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: AlertDialog(
      title: Text(widget.rating == 'positive' ? '确认正面评价' : '确认负面评价'),
      content: SizedBox(
        width: 500,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('记录对这条回复的评价，分类和说明可选。', style: TextStyle(fontSize: 13)),
              const SizedBox(height: 14),
              DshSelect<String>(
                options: const {'': '选择分类（可选）', ...feedbackCategories},
                value: category,
                onChanged: busy ? null : (v) => setState(() => category = v),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: note,
                enabled: !busy,
                minLines: 4,
                maxLines: 8,
                decoration: const InputDecoration(
                  labelText: '反馈说明',
                  hintText: '这条回答哪里好，或哪里有问题？（可选）',
                ),
              ),
              if (error != null)
                Text(
                  error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
            ],
          ),
        ),
      ),
      actions: [
        DshButton(
          onPressed: busy ? null : () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        DshButton(
          primary: true,
          onPressed: busy ? null : save,
          child: Text(busy ? '正在保存…' : '确认评价'),
        ),
      ],
    ),
  );
}
