import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

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
              ? 'plan mode 已请求开启，将在下一步生效 — 点击取消'
              : 'plan mode 已开启 — 点击关闭（/plan off）',
          child: Semantics(
            label: 'plan mode 已开启，按下关闭',
            button: true,
            toggled: true,
            child: DshIcon(
              LucideIcons.listChecks,
              asset: 'assets/icons/task-list.svg',
              size: 28,
              label: '关闭计划模式',
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
