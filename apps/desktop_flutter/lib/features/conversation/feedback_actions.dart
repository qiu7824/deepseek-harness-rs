import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../design/select.dart';
import 'feedback_controller.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

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
              positive ? DshIcons.thumbsUp.data : DshIcons.thumbsDown.data,
              label: row?['rating'] == (positive ? 'positive' : 'negative')
                  ? DshConversationZh.clearFeedback
                  : positive
                  ? DshConversationZh.goodAnswer
                  : DshConversationZh.problematicAnswer,
              size: 28,
              active: row?['rating'] == (positive ? 'positive' : 'negative'),
              onPressed: c.busy
                  ? null
                  : () => choose(positive ? 'positive' : 'negative'),
            ),
          ],
          if (row != null)
            DshIcon(
              DshIcons.pencil.data,
              label: DshConversationZh.addFeedbackNote,
              size: 28,
              onPressed: c.busy
                  ? null
                  : () => choose('${row['rating']}', edit: true),
            ),
          if (c.error != null)
            DshIcon(
              DshIcons.circleAlert.data,
              label: DshConversationZh.feedbackReloadHint(
                error: DshError.describe(c.error!).message,
              ),
              size: 28,
              color: Theme.of(context).colorScheme.error,
              onPressed: c.busy
                  ? null
                  : () {
                      showDshError(context, c.error!);
                      c.ensure(refresh: true);
                    },
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
  Object? error;
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
      error = c.error ?? DshConversationZh.feedbackNotSaved;
      version = c.item(widget.messageId)?['version'] as String?;
    });
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: AlertDialog(
      title: Text(
        widget.rating == 'positive'
            ? DshConversationZh.confirmPositiveFeedback
            : DshConversationZh.confirmNegativeFeedback,
      ),
      content: SizedBox(
        width: 500,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                DshConversationZh.feedbackHint,
                style: TextStyle(fontSize: DshTypography.sizeAuxiliary),
              ),
              const SizedBox(height: 14),
              DshSelect<String>(
                options: const {
                  '': DshConversationZh.optionalFeedbackCategory,
                  ...feedbackCategories,
                },
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
                  labelText: DshConversationZh.feedbackNote,
                  hintText: DshConversationZh.feedbackNoteHint,
                ),
              ),
              if (error != null) DshErrorView(error: error!),
            ],
          ),
        ),
      ),
      actions: [
        DshButton(
          onPressed: busy ? null : () => Navigator.pop(context),
          child: const Text(DshConversationZh.cancel),
        ),
        DshButton(
          primary: true,
          onPressed: busy ? null : save,
          child: Text(
            busy
                ? DshConversationZh.savingFeedback
                : DshConversationZh.confirmFeedback,
          ),
        ),
      ],
    ),
  );
}
