import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';
import 'session_status.dart';

export 'trajectory_view.dart' show TraceView;

String amount(Object? value) =>
    value is num && value.isFinite && value >= 0 ? '${value.round()}' : '未提供';
String displayDate(Object? value) =>
    value is num && value >= 0 && value < 8640000000000000
    ? DateTime.fromMillisecondsSinceEpoch(value.toInt())
          .toLocal()
          .toString()
          .split('.')
          .first
    : '未提供';

class ContextView extends StatelessWidget {
  const ContextView({super.key, required this.controller});
  final DesktopController controller;
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller.projectionChanges,
    builder: (context, _) {
      final p = controller.projections,
          usage = object(p['tokenUsage']),
          insights = object(p['contextInsights']),
          pressure = object(p['contextPressure']);
      final reading = ContextOccupancy.fromJson(pressure),
          selected = object(p['modelSelection'] ?? controller.catalog?.current),
          colors = DshColors(context);
      final total = usage.isEmpty
          ? null
          : billedInputTokens(usage) + nonNegative(usage, 'outputTokens');
      final fields = <String, String>{
        '会话': controller.selected?.title ?? controller.selectedId ?? '未提供',
        '消息数':
            insights['userMessages'] is! num ||
                insights['assistantMessages'] is! num
            ? '未提供'
            : amount(
                nonNegative(insights, 'userMessages') +
                    nonNegative(insights, 'assistantMessages'),
              ),
        '提供商':
            controller.catalog?.providerNames[selected['provider']] ??
            '${selected['provider'] ?? '未提供'}',
        '模型':
            controller.catalog?.choices
                .where(
                  (m) =>
                      m.provider == selected['provider'] &&
                      m.id == selected['model'],
                )
                .firstOrNull
                ?.name ??
            '${selected['model'] ?? '未提供'}',
        '上下文限制': pressure['contextWindowEstimated'] == true
            ? '未提供（运行预算 ${amount(pressure['contextWindow'])}）'
            : amount(pressure['contextWindow']),
        '总 token': amount(total),
        '使用率': reading == null
            ? '未提供'
            : '${reading.estimated ? '≈' : ''}${(reading.used / reading.capacity * 100).toStringAsFixed(1)}%',
        '输入 token': amount(usage['uncachedInputTokens']),
        '输出 token': amount(usage['outputTokens']),
        '推理 token': amount(insights['reasoningTokens']),
        '缓存 token（读/写）': usage.isEmpty
            ? '未提供'
            : '${amount(usage['cacheReadTokens'])} / ${amount(usage['cacheWriteTokens'])}',
        '用户消息': amount(insights['userMessages']),
        '助手消息': amount(insights['assistantMessages']),
        '总成本': insights['totalCost'] is num
            ? 'USD ${(insights['totalCost'] as num).toStringAsFixed(4)}'
            : '未提供',
        '创建时间': displayDate(insights['createdAt']),
        '最后活动': displayDate(
          object(p['sessionListMetadata'])['updatedAt'] ??
              controller.selected?.updatedAt,
        ),
      };
      final roles = object(insights['roleTokens']),
          roleIds = ['user', 'assistant', 'tool', 'other'];
      final roleTotal = roleIds.fold<num>(
        0,
        (sum, id) => sum + nonNegative(roles, id),
      );
      final roleColors = [
        const Color(0xff22c55e),
        const Color(0xffd36524),
        const Color(0xff8a6118),
        colors.dark ? const Color(0xff81858c) : const Color(0xffadb2b8),
      ];
      return SingleChildScrollView(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            LayoutBuilder(
              builder: (context, constraints) => Wrap(
                spacing: 24,
                runSpacing: 18,
                children: [
                  for (final entry in fields.entries)
                    SizedBox(
                      width: constraints.maxWidth < 600
                          ? constraints.maxWidth
                          : (constraints.maxWidth - 24) / 2,
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            entry.key,
                            style: TextStyle(fontSize: 12, color: colors.muted),
                          ),
                          const SizedBox(height: 4),
                          SelectableText(
                            entry.value,
                            style: TextStyle(
                              fontSize: 14,
                              height: 1.6,
                              color: entry.value == '未提供'
                                  ? colors.muted
                                  : colors.text,
                            ),
                          ),
                        ],
                      ),
                    ),
                ],
              ),
            ),
            const SizedBox(height: 28),
            const Text(
              '上下文细分',
              style: TextStyle(fontSize: 14, fontWeight: FontWeight.w500),
            ),
            const SizedBox(height: 12),
            if (roleTotal > 0)
              ClipRRect(
                borderRadius: BorderRadius.circular(6),
                child: SizedBox(
                  height: 10,
                  child: Row(
                    children: [
                      for (var i = 0; i < roleIds.length; i++)
                        if (nonNegative(roles, roleIds[i]) > 0)
                          Expanded(
                            flex:
                                (nonNegative(roles, roleIds[i]) /
                                        roleTotal *
                                        10000)
                                    .round()
                                    .clamp(1, 10000),
                            child: ColoredBox(
                              color: roleColors[i],
                              child: const SizedBox.expand(),
                            ),
                          ),
                    ],
                  ),
                ),
              )
            else
              Text(
                '尚无上下文统计',
                style: TextStyle(color: colors.muted, fontSize: 12),
              ),
            const SizedBox(height: 12),
            Wrap(
              spacing: 20,
              runSpacing: 8,
              children: [
                for (var i = 0; i < roleIds.length; i++)
                  Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Container(width: 8, height: 8, color: roleColors[i]),
                      const SizedBox(width: 6),
                      Text(
                        '${const ['用户', '助手', '工具调用', '其他'][i]} ${roleTotal == 0 ? '—' : '${(nonNegative(roles, roleIds[i]) / roleTotal * 100).toStringAsFixed(1)}%'}',
                        style: const TextStyle(fontSize: 12),
                      ),
                    ],
                  ),
              ],
            ),
            const SizedBox(height: 12),
            Text(
              '上下文细分为当前模型可见内容的估算；总 token 为累计计费量，推理 token 包含在输出量中。未报告的用量与价格显示为“未提供”。',
              style: TextStyle(fontSize: 12, height: 1.7, color: colors.muted),
            ),
            const SizedBox(height: 28),
            ContextSummary(controller: controller),
          ],
        ),
      );
    },
  );
}

class EventDetail extends StatefulWidget {
  const EventDetail({super.key, required this.event});
  final HistoryEvent event;
  @override
  State<EventDetail> createState() => _EventDetailState();
}

class _EventDetailState extends State<EventDetail> {
  late final text = const JsonEncoder.withIndent('  ')
      .convert(widget.event.raw);
  int page = 0;
  static const pageSize = 16384;
  @override
  Widget build(BuildContext context) {
    final pages = (text.length / pageSize).ceil().clamp(1, 100000);
    var start = page * pageSize,
        end = ((page + 1) * pageSize).clamp(0, text.length);
    bool lowSurrogate(int index) =>
        index < text.length &&
        text.codeUnitAt(index) >= 0xdc00 &&
        text.codeUnitAt(index) <= 0xdfff;
    if (start > 0 && lowSurrogate(start)) start--;
    if (end < text.length && lowSurrogate(end)) end--;
    return AlertDialog(
      title: Text(
        '${widget.event.type} · #${widget.event.seq}',
        style: const TextStyle(fontSize: 16),
      ),
      content: SizedBox(
        width: 760,
        height: 500,
        child: SingleChildScrollView(
          key: ValueKey(page),
          child: SelectableText(
            text.substring(start, end),
            style: const TextStyle(fontFamily: 'Consolas', fontSize: 12),
          ),
        ),
      ),
      actions: [
        Text('${page + 1} / $pages', style: const TextStyle(fontSize: 12)),
        DshButton(
          onPressed: page == 0 ? null : () => setState(() => page--),
          child: const Text('上一页'),
        ),
        DshButton(
          onPressed: page + 1 >= pages ? null : () => setState(() => page++),
          child: const Text('下一页'),
        ),
        DshButton(
          onPressed: () => Clipboard.setData(ClipboardData(text: text)),
          child: const Text('复制事件'),
        ),
        DshButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('关闭'),
        ),
      ],
    );
  }
}
