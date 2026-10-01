import 'package:flutter/material.dart';

import '../l10n/conversation_zh.dart';
import 'tokens.dart';
import 'typography.dart';

/// Fixed row placeholders for a first read. Existing rows stay visible on refresh.
/// Static shapes remain quiet for reduced motion and never enter focus order.
class DshListSkeleton extends StatelessWidget {
  const DshListSkeleton({super.key, this.label, this.rows = 5});
  final String? label;
  final int rows;

  @override
  Widget build(BuildContext context) {
    final tokens = DshTokens.of(context);
    final message = label ?? DshConversationZh.loadingData;
    final rowHeight = tokens.controlHeight(context, minimum: 56);
    return Semantics(
      container: true,
      liveRegion: true,
      label: message,
      child: ExcludeSemantics(
        child: ExcludeFocus(
          child: ListView(
            shrinkWrap: true,
            primary: false,
            physics: const NeverScrollableScrollPhysics(),
            padding: const EdgeInsets.all(12),
            children: [
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Text(
                  message,
                  style: DshTypography.body.copyWith(color: tokens.muted),
                ),
              ),
              for (var index = 0; index < rows; index++)
                SizedBox(
                  height: rowHeight,
                  child: Row(
                    children: [
                      DecoratedBox(
                        decoration: BoxDecoration(
                          color: tokens.border.withValues(alpha: .5),
                          borderRadius: BorderRadius.circular(
                            tokens.radiusControl,
                          ),
                        ),
                        child: const SizedBox.square(dimension: 24),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          mainAxisAlignment: MainAxisAlignment.center,
                          children: [
                            FractionallySizedBox(
                              widthFactor: .72,
                              child: _line(tokens),
                            ),
                            const SizedBox(height: 8),
                            FractionallySizedBox(
                              widthFactor: .45,
                              child: _line(tokens),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _line(DshTokens tokens) => DecoratedBox(
    decoration: BoxDecoration(
      color: tokens.border.withValues(alpha: .5),
      borderRadius: BorderRadius.circular(4),
    ),
    child: const SizedBox(height: 10),
  );
}
