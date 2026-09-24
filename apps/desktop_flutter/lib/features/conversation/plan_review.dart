import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/rich_content.dart';
import '../../src/controller.dart';

class PlanReviewData {
  const PlanReviewData({
    required this.id,
    required this.question,
    required this.plan,
    required this.approve,
    this.decline,
  });
  final String id, question, plan;
  final Json approve;
  final Json? decline;

  /// Specialization must preserve every choice declared by the request.
  static PlanReviewData? fromQuestions(List<Json> questions) {
    if (questions.length != 1) return null;
    final q = questions.single, intent = object(questions.single['intent']);
    if (intent['kind'] != 'plan-review' ||
        q['detail'] is! String ||
        q['id'] is! String ||
        q['question'] is! String ||
        q['multiSelect'] == true ||
        intent['approve'] is! String) {
      return null;
    }
    final raw = q['options'];
    if (raw is! List ||
        raw.isEmpty ||
        raw.length > 2 ||
        raw.any((o) => o is! Map || o['label'] is! String)) {
      return null;
    }
    final options = objects(raw),
        approve = options
            .where((o) => o['label'] == intent['approve'])
            .firstOrNull;
    if (approve == null ||
        options.map((o) => o['label']).toSet().length != options.length) {
      return null;
    }
    return PlanReviewData(
      id: q['id'] as String,
      question: q['question'] as String,
      plan: q['detail'] as String,
      approve: approve,
      decline: options
          .where((o) => o['label'] != intent['approve'])
          .firstOrNull,
    );
  }
}

class PlanReviewCard extends StatefulWidget {
  const PlanReviewCard({
    super.key,
    required this.controller,
    required this.frame,
    required this.review,
  });
  final DesktopController controller;
  final HostFrame frame;
  final PlanReviewData review;
  @override
  State<PlanReviewCard> createState() => _PlanReviewCardState();
}

class _PlanReviewCardState extends State<PlanReviewCard> {
  final scroll = ScrollController();
  bool busy = false;
  String? error;
  bool get enabled =>
      !busy &&
      widget.controller.connected &&
      !widget.controller.answering.contains(widget.frame.rpcId);
  @override
  void dispose() {
    scroll.dispose();
    super.dispose();
  }

  Future<void> decide(String? label) async {
    if (!enabled) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      if (label == null) {
        await widget.controller.cancelQuestion(widget.frame);
      } else {
        await widget.controller.answer(widget.frame, {
          'sessionId': widget.frame.sessionId,
          'answer': {
            'answers': [
              {
                'id': widget.review.id,
                'selected': [label],
              },
            ],
          },
        });
      }
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
      final colors = DshColors(context),
          narrow = MediaQuery.sizeOf(context).width <= 720;
      final radius = BorderRadius.circular(narrow ? 16 : 20),
          review = widget.review;
      final actions = Wrap(
        spacing: 8,
        runSpacing: 8,
        alignment: WrapAlignment.end,
        children: [
          DshButton(
            height: 36,
            pill: true,
            padding: const EdgeInsets.symmetric(horizontal: 14),
            icon: LucideIcons.pencil,
            onPressed: enabled ? () => decide(null) : null,
            child: const Text(
              '去聊天里说',
              style: TextStyle(fontWeight: FontWeight.w400),
            ),
          ),
          if (review.decline != null)
            Tooltip(
              message: '${review.decline!['description'] ?? ''}',
              child: DshButton(
                height: 36,
                pill: true,
                padding: const EdgeInsets.symmetric(horizontal: 14),
                outline: true,
                onPressed: enabled
                    ? () => decide(review.decline!['label'] as String)
                    : null,
                child: const Text(
                  '拒绝',
                  style: TextStyle(fontWeight: FontWeight.w400),
                ),
              ),
            ),
          Tooltip(
            message: '${review.approve['description'] ?? ''}',
            child: DshButton(
              key: const Key('approve-plan'),
              height: 36,
              pill: true,
              padding: const EdgeInsets.symmetric(horizontal: 14),
              primary: true,
              onPressed: enabled
                  ? () => decide(review.approve['label'] as String)
                  : null,
              child: const Text(
                '确认执行',
                style: TextStyle(fontWeight: FontWeight.w400),
              ),
            ),
          ),
        ],
      );
      return Semantics(
        label: review.question,
        child: Container(
          key: const Key('plan-review-card'),
          constraints: BoxConstraints(
            maxHeight: (MediaQuery.sizeOf(context).height * .6).clamp(0, 520),
          ),
          decoration: BoxDecoration(
            color: colors.dark ? const Color(0xff2c2c2e) : Colors.white,
            border: Border.all(color: const Color(0xfff7ad31)),
            borderRadius: radius,
            boxShadow: const [
              BoxShadow(
                color: Color(0x05000000),
                blurRadius: 12,
                offset: Offset(0, 4),
              ),
              BoxShadow(
                color: Color(0x0a000000),
                blurRadius: 8,
                offset: Offset(0, 2),
              ),
            ],
          ),
          clipBehavior: Clip.antiAlias,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 16,
                  vertical: 10,
                ),
                color: colors.dark
                    ? const Color(0xff27241f)
                    : const Color(0xfffef5e7),
                child: Row(
                  children: [
                    Container(
                      width: 8,
                      height: 8,
                      decoration: const BoxDecoration(
                        color: Color(0xfff59e0b),
                        shape: BoxShape.circle,
                      ),
                    ),
                    const SizedBox(width: 8),
                    const Text(
                      '计划待审',
                      style: TextStyle(
                        fontSize: 13,
                        height: 18 / 13,
                        color: Color(0xfff59e0b),
                      ),
                    ),
                  ],
                ),
              ),
              Flexible(
                child: SingleChildScrollView(
                  controller: scroll,
                  padding: EdgeInsets.fromLTRB(
                    narrow ? 12 : 16,
                    narrow ? 10 : 12,
                    narrow ? 12 : 16,
                    4,
                  ),
                  child: DshMarkdown(data: review.plan),
                ),
              ),
              Padding(
                padding: EdgeInsets.fromLTRB(
                  narrow ? 12 : 16,
                  8,
                  narrow ? 12 : 16,
                  12,
                ),
                child: LayoutBuilder(
                  builder: (_, box) {
                    final feedback = Text(
                      error ?? '',
                      style: TextStyle(
                        fontSize: 11,
                        color: Theme.of(context).colorScheme.error,
                      ),
                    );
                    if (box.maxWidth < 440) {
                      return Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          if (error != null)
                            Padding(
                              padding: const EdgeInsets.only(bottom: 8),
                              child: feedback,
                            ),
                          actions,
                        ],
                      );
                    }
                    return Row(
                      children: [
                        Expanded(child: feedback),
                        const SizedBox(width: 12),
                        actions,
                      ],
                    );
                  },
                ),
              ),
            ],
          ),
        ),
      );
    },
  );
}
