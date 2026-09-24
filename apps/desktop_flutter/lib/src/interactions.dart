import 'package:flutter/material.dart';
import 'package:dsh_client/dsh_client.dart';

import 'controller.dart';
import '../features/conversation/question_flow.dart';
import '../features/conversation/plan_review.dart';
import '../features/conversation/approval_card.dart';

class InteractionCard extends StatelessWidget {
  const InteractionCard({
    super.key,
    required this.controller,
    required this.frame,
  });
  final DesktopController controller;
  final HostFrame frame;
  @override
  Widget build(BuildContext context) {
    if (frame.type == 'approval/requested') {
      return ApprovalCard(controller: controller, frame: frame);
    }
    if (frame.type == 'question/requested') {
      final review = PlanReviewData.fromQuestions(
        objects(frame.payload['questions']),
      );
      if (review != null) {
        return PlanReviewCard(
          controller: controller,
          frame: frame,
          review: review,
        );
      }
      return QuestionFlow(controller: controller, frame: frame);
    }
    return const SizedBox.shrink();
  }
}
