import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/statistics_popover.dart';
import '../../l10n/statistics_zh.dart';

import 'package:dsh_desktop/l10n/conversation_zh.dart';

String turnDuration(int milliseconds) {
  final seconds = milliseconds < 0 ? 0 : milliseconds ~/ 1000;
  final minutes = seconds ~/ 60;
  return minutes == 0
      ? DshConversationZh.secondsDuration(seconds: seconds)
      : DshConversationZh.minutesSecondsDuration(
          minutes: minutes,
          seconds: (seconds % 60).toString().padLeft(2, '0'),
        );
}

String turnRate(num rate) =>
    rate >= 10 ? '${rate.round()}' : '${(rate * 10).round() / 10}';

String _exactTokens(int value) {
  final digits = '$value';
  final groups = <String>[];
  for (var end = digits.length; end > 0; end -= 3) {
    final start = end - 3 < 0 ? 0 : end - 3;
    groups.insert(0, digits.substring(start, end));
  }
  return groups.join(',');
}

String _compactTokens(int value) {
  String scaled(double n) =>
      n >= 100 ? '${n.round()}' : '${(n * 10).round() / 10}';
  if (value < 1000) return '$value';
  if (value < 1000000) return '${scaled(value / 1000)}K';
  return '${scaled(value / 1000000)}M';
}

class _TurnStatButton extends StatelessWidget {
  const _TurnStatButton({
    required this.label,
    required this.title,
    required this.rows,
  });
  final String label, title;
  final List<(String, String)> rows;
  @override
  Widget build(BuildContext context) =>
      StatisticsPopover(label: label, title: title, rows: rows);
}

class TurnTimeButton extends StatelessWidget {
  const TurnTimeButton({
    super.key,
    required this.runMs,
    this.ttftMs,
    this.tokensPerSecond,
  });
  final int runMs;
  final int? ttftMs;
  final double? tokensPerSecond;
  @override
  Widget build(BuildContext context) {
    final rows = <(String, String)>[
      (DshConversationZh.turnDuration, turnDuration(runMs)),
      if (tokensPerSecond != null)
        (
          DshConversationZh.averageOutputRateHint,
          '${turnRate(tokensPerSecond!)} tok/s',
        ),
      if (ttftMs != null)
        (
          DshConversationZh.timeToFirstToken,
          DshConversationZh.secondsDuration(seconds: turnRate(ttftMs! / 1000)),
        ),
    ];
    return _TurnStatButton(
      label: DshConversationZh.elapsedTime(duration: turnDuration(runMs)),
      title: DshConversationZh.turnTiming,
      rows: rows,
    );
  }
}

class TurnUsageButton extends StatelessWidget {
  const TurnUsageButton({super.key, required this.usage});
  final Json usage;
  @override
  Widget build(BuildContext context) {
    final total = usage['totalTokens'];
    if (total is! int || total <= 0) return const SizedBox.shrink();
    final output = usage['outputTokens'] as int?;
    final prompt = output == null ? null : total - output;
    final read = usage['cacheReadTokens'] as int?;
    final write = usage['cacheWriteTokens'] as int?;
    final reasoning = usage['reasoningTokens'] as int?;
    final routes = objects(usage['routes'])
        .map((route) => '${route['provider']}/${route['model']}')
        .join(', ');
    final rows = <(String, String)>[
      (DshStatisticsZh.total, '${_exactTokens(total)} tok'),
      if (routes.isNotEmpty) (DshConversationZh.providerModelLabel, routes),
      if (read != null && prompt != null && prompt > 0)
        (DshConversationZh.cacheHit, '${(read / prompt * 1000).round() / 10}%'),
      (
        DshConversationZh.uncachedInput,
        usage['uncachedInputTokens'] is int
            ? '${_exactTokens(usage['uncachedInputTokens'] as int)} tok'
            : DshStatisticsZh.unavailable,
      ),
      if (read != null)
        (DshConversationZh.cacheRead, '${_exactTokens(read)} tok'),
      if (write != null)
        (DshConversationZh.cacheWrite, '${_exactTokens(write)} tok'),
      (
        DshConversationZh.output,
        output == null
            ? DshStatisticsZh.unavailable
            : DshConversationZh.outputWithReasoning(
                output: _exactTokens(output),
                reasoning: reasoning == null ? null : _exactTokens(reasoning),
              ),
      ),
    ];
    return _TurnStatButton(
      label: DshConversationZh.tokenUsage(amount: _compactTokens(total)),
      title: DshConversationZh.turnUsage,
      rows: rows,
    );
  }
}
