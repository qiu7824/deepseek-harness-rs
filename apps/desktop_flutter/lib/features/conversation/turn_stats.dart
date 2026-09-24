import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';

String turnDuration(int milliseconds) {
  final seconds = milliseconds < 0 ? 0 : milliseconds ~/ 1000;
  final minutes = seconds ~/ 60;
  return minutes == 0
      ? '$seconds秒'
      : '$minutes分${(seconds % 60).toString().padLeft(2, '0')}秒';
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

class _TurnStatButton extends StatefulWidget {
  const _TurnStatButton({
    required this.label,
    required this.title,
    required this.rows,
  });
  final String label, title;
  final List<(String, String)> rows;
  @override
  State<_TurnStatButton> createState() => _TurnStatButtonState();
}

class _TurnStatButtonState extends State<_TurnStatButton> {
  final popover = ShadPopoverController();
  @override
  void dispose() {
    popover.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): popover.hide,
      },
      child: ShadPopover(
        controller: popover,
        padding: const EdgeInsets.all(14),
        anchor: const ShadAnchorAuto(
          targetAnchor: Alignment.topRight,
          followerAnchor: Alignment.bottomRight,
          offset: Offset(0, -8),
        ),
        popover: (_) => SizedBox(
          width: 320,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                widget.title,
                style: const TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 10),
              for (final (label, value) in widget.rows)
                Padding(
                  padding: const EdgeInsets.only(bottom: 6),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      SizedBox(
                        width: 125,
                        child: Text(
                          label,
                          style: TextStyle(fontSize: 12, color: colors.muted),
                        ),
                      ),
                      Expanded(
                        child: Text(
                          value,
                          style: const TextStyle(fontSize: 12),
                        ),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
        child: Semantics(
          label: widget.label,
          button: true,
          child: ShadButton.ghost(
            height: 28,
            padding: const EdgeInsets.symmetric(horizontal: 8),
            onPressed: popover.toggle,
            child: Text(
              widget.label,
              style: TextStyle(fontSize: 14, color: colors.muted),
            ),
          ),
        ),
      ),
    );
  }
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
      ('本轮总用时', turnDuration(runMs)),
      if (tokensPerSecond != null)
        ('请求平均输出速率（发送至结束）', '${turnRate(tokensPerSecond!)} tok/s'),
      if (ttftMs != null) ('首 token 用时（TTFT）', '${turnRate(ttftMs! / 1000)}秒'),
    ];
    return _TurnStatButton(
      label: '用时 ${turnDuration(runMs)}',
      title: '本轮用时和速度',
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
    final output = usage['outputTokens'] as int? ?? 0;
    final prompt = total - output;
    final read = usage['cacheReadTokens'] as int?;
    final write = usage['cacheWriteTokens'] as int?;
    final reasoning = usage['reasoningTokens'] as int?;
    final routes = objects(usage['routes'])
        .map((route) => '${route['provider']}/${route['model']}')
        .join(', ');
    final rows = <(String, String)>[
      if (routes.isNotEmpty) ('提供方 / 模型', routes),
      if (read != null && prompt > 0)
        ('缓存命中', '${(read / prompt * 1000).round() / 10}%'),
      ('未缓存输入', _exactTokens(usage['uncachedInputTokens'] as int? ?? 0)),
      if (read != null) ('缓存读取', _exactTokens(read)),
      if (write != null) ('缓存写入', _exactTokens(write)),
      (
        '输出',
        '${_exactTokens(output)}${reasoning == null ? '' : '（其中推理 ${_exactTokens(reasoning)}）'}',
      ),
    ];
    return _TurnStatButton(
      label: '用量 ${_compactTokens(total)}',
      title: '本轮用量',
      rows: rows,
    );
  }
}
