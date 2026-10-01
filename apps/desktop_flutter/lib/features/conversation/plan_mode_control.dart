import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

import 'package:dsh_desktop/l10n/conversation_zh.dart';

class PlanModeControl extends StatelessWidget {
  PlanModeControl({super.key, required this.controller})
    : changes = Listenable.merge([controller, controller.projectionChanges]);
  final DesktopController controller;
  final Listenable changes;
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: changes,
    builder: (_, _) {
      final plan = controller.planMode;
      if (plan?.requestedActive != true) return const SizedBox.shrink();
      final enabled =
          controller.connected &&
          !controller.sending &&
          !controller.changingPlanMode;
      final owner = controller.client,
          session = controller.selectedId,
          revision = controller.selectionRevision;
      return Padding(
        padding: const EdgeInsets.only(left: 6),
        child: Tooltip(
          message: plan!.pending
              ? DshConversationZh.planRequestedHint
              : DshConversationZh.planEnabledHint,
          child: Semantics(
            label: DshConversationZh.planEnabledAction,
            button: true,
            toggled: true,
            child: DshIcon(
              DshIcons.listChecks.data,
              asset: 'assets/icons/task-list.svg',
              size: 28,
              label: DshConversationZh.closePlanMode,
              active: true,
              onPressed: enabled
                  ? () {
                      if (controller.client != owner ||
                          controller.selectedId != session ||
                          controller.selectionRevision != revision ||
                          controller.planMode?.requestedActive != true) {
                        return;
                      }
                      controller.run(() => controller.setPlanMode(false));
                    }
                  : null,
            ),
          ),
        ),
      );
    },
  );
}
