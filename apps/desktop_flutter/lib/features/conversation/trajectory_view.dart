import 'dart:math' as math;
import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';
import 'session_views.dart' show EventDetail, displayDate;

Color traceColor(TraceRecord row, DshColors colors) => row.failed
    ? const Color(0xffe05252)
    : switch (row.kind) {
        'user' => colors.blue,
        'context' => const Color(0xff58a67c),
        'assistant' || 'compaction' => const Color(0xff8f70b1),
        'tool' || 'subtool' => const Color(0xffd68a27),
        _ => colors.muted,
      };

class TraceView extends StatefulWidget {
  const TraceView({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<TraceView> createState() => _TraceViewState();
}

class _TraceViewState extends State<TraceView> {
  final search = TextEditingController(), scroll = ScrollController();
  final expandedTurns = <int>{}, expandedRequests = <String>{};
  bool collapseTurns = false,
      collapseCalls = false,
      detailsOpen = false,
      follow = true;
  late bool actualDuration =
      widget.controller.preferences.layout['traceActualDuration'] == true;
  TraceSnapshot snapshot = TraceSnapshot.fromEvents([]);
  ConversationWindow? previousWindow;
  int revision = -1;
  bool previousLive = false;
  Timer? liveClock;
  String? selectedId;
  ({String id, double offset})? pendingAnchor;
  RangeValues? focusRange;
  @override
  void dispose() {
    liveClock?.cancel();
    search.dispose();
    scroll.dispose();
    super.dispose();
  }

  Future<void> loadOlder(List<TraceDisplayRow> rows) async {
    if (scroll.hasClients && rows.isNotEmpty) {
      final index = (scroll.offset / 30).floor().clamp(0, rows.length - 1);
      pendingAnchor = (id: rows[index].id, offset: scroll.offset - index * 30);
    }
    follow = false;
    await widget.controller.run(
      () => widget.controller.loadHistory(
        before: widget.controller.window.firstSeq,
        merge: true,
      ),
    );
    if (mounted) setState(() {});
  }

  List<TraceDisplayRow> get rows => traceDisplayRows(
    snapshot,
    search: search.text,
    collapseTurns: collapseTurns,
    collapseCalls: collapseCalls,
    expandedTurns: expandedTurns,
    expandedRequests: expandedRequests,
  );

  void focusRecord(TraceRecord row, {bool inspect = false}) {
    setState(() {
      selectedId = row.id;
      detailsOpen = inspect;
      expandedTurns.add(row.turn);
      expandedRequests.add('${row.turn}:${row.step}');
      follow = false;
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !scroll.hasClients) return;
      final index = rows.indexWhere((e) => e.record.id == row.id);
      if (index >= 0) {
        scroll.jumpTo((index * 30.0).clamp(0, scroll.position.maxScrollExtent));
      }
    });
  }

  Widget toggle(
    String label,
    String tooltip,
    IconData icon,
    bool value,
    VoidCallback change,
  ) => Tooltip(
    message: tooltip,
    child: Semantics(
      checked: value,
      label: tooltip,
      child: ShadButton.ghost(
        height: 24,
        padding: const EdgeInsets.symmetric(horizontal: 5),
        onPressed: change,
        backgroundColor: value ? DshColors(context).hover : null,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            DshGlyph(icon, size: 12, color: DshColors(context).muted),
            const SizedBox(width: 4),
            Text(
              label,
              style: TextStyle(fontSize: 12, color: DshColors(context).muted),
            ),
          ],
        ),
      ),
    ),
  );

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller.messageChanges,
    builder: (context, _) {
      final c = widget.controller, colors = DshColors(context);
      final live = c.running && !c.window.hasAfter;
      if (!identical(previousWindow, c.window) ||
          revision != c.window.revision ||
          previousLive != live) {
        previousLive = live;
        previousWindow = c.window;
        revision = c.window.revision;
        snapshot = TraceSnapshot.fromEvents(c.window.events, live: live);
        final turns = snapshot.records.map((r) => r.turn).toSet();
        expandedTurns.removeWhere((t) => !turns.contains(t));
        final requests = snapshot.records
            .map((r) => '${r.turn}:${r.step}')
            .toSet();
        expandedRequests.removeWhere((r) => !requests.contains(r));
        if (!snapshot.records.any((r) => r.id == selectedId)) {
          selectedId = null;
          detailsOpen = false;
        }
        if (follow && search.text.isEmpty) {
          WidgetsBinding.instance.addPostFrameCallback((_) {
            if (mounted && scroll.hasClients) {
              scroll.jumpTo(scroll.position.maxScrollExtent);
            }
          });
        }
      }
      if (live &&
          actualDuration &&
          snapshot.records.any((r) => r.status == 'running')) {
        liveClock ??= Timer.periodic(const Duration(milliseconds: 500), (_) {
          if (mounted) setState(() {});
        });
      } else {
        liveClock?.cancel();
        liveClock = null;
      }
      final display = rows,
          spans = snapshot.spans(
            actualDuration: actualDuration,
            now: live ? DateTime.now().millisecondsSinceEpoch : null,
          );
      final selected = snapshot.records
          .where((r) => r.id == selectedId)
          .firstOrNull;
      if (pendingAnchor != null && !c.loading) {
        final anchor = pendingAnchor!,
            index = display.indexWhere((r) => r.id == anchor.id);
        pendingAnchor = null;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted && scroll.hasClients && index >= 0) {
            scroll.jumpTo(
              (index * 30 + anchor.offset).clamp(
                0,
                scroll.position.maxScrollExtent,
              ),
            );
          }
        });
      }
      final positions = {
        for (final span in spans) span.record.id: (span.start + span.end) / 2,
      };
      Widget ledger() => NotificationListener<UserScrollNotification>(
        onNotification: (n) {
          if (n.direction != ScrollDirection.idle) {
            follow = n.metrics.extentAfter < 40;
          }
          return false;
        },
        child: display.isEmpty
            ? const DshEmpty('没有匹配的轨迹')
            : ListView.builder(
                key: const PageStorageKey('trajectory-ledger'),
                controller: scroll,
                itemExtent: 30,
                itemCount: display.length,
                itemBuilder: (context, index) {
                  final item = display[index],
                      row = item.record,
                      color = traceColor(row, colors);
                  final position = positions[row.id],
                      dim =
                          focusRange != null &&
                          position != null &&
                          (position < focusRange!.start ||
                              position > focusRange!.end);
                  return Semantics(
                    button: true,
                    label: '${row.role} ${row.preview}',
                    selected: row.id == selectedId,
                    child: Material(
                      color: row.id == selectedId ? colors.hover : colors.base,
                      child: InkWell(
                        onTap: () {
                          if (item.summaryCount > 0) {
                            setState(() {
                              if (item.summaryKind == 'turn') {
                                expandedTurns.add(row.turn);
                              } else {
                                expandedRequests.add(item.requestKey);
                              }
                            });
                          } else {
                            setState(() {
                              selectedId = row.id;
                              detailsOpen = true;
                            });
                          }
                        },
                        child: Opacity(
                          opacity: dim ? .24 : 1,
                          child: Container(
                            decoration: BoxDecoration(
                              border: Border(
                                bottom: BorderSide(
                                  color: colors.border.withValues(alpha: .04),
                                ),
                                top: BorderSide(
                                  color: item.turnStart && row.turn > 0
                                      ? colors.border
                                      : Colors.transparent,
                                ),
                              ),
                            ),
                            child: Row(
                              children: [
                                SizedBox(
                                  width: 68,
                                  child: Text(
                                    item.turnStart && row.turn > 0
                                        ? '第 ${row.turn} 轮'
                                        : '',
                                    style: TextStyle(
                                      fontSize: 10,
                                      color: colors.muted,
                                    ),
                                    textAlign: TextAlign.center,
                                  ),
                                ),
                                SizedBox(
                                  width: 54,
                                  child: Align(
                                    alignment: Alignment.centerLeft,
                                    child: Container(
                                      padding: const EdgeInsets.symmetric(
                                        horizontal: 5,
                                        vertical: 1,
                                      ),
                                      color: color.withValues(alpha: .10),
                                      child: Text(
                                        row.role,
                                        style: TextStyle(
                                          fontSize: 11,
                                          color: color,
                                        ),
                                      ),
                                    ),
                                  ),
                                ),
                                Expanded(
                                  child: Padding(
                                    padding: const EdgeInsets.only(right: 8),
                                    child:
                                        item.summaryCount == 0 &&
                                            row.lane == 2 &&
                                            row.result != null
                                        ? Row(
                                            children: [
                                              ConstrainedBox(
                                                constraints:
                                                    const BoxConstraints(
                                                      maxWidth: 140,
                                                    ),
                                                child: Text(
                                                  row.title,
                                                  maxLines: 1,
                                                  overflow:
                                                      TextOverflow.ellipsis,
                                                  style: const TextStyle(
                                                    fontFamily: 'Consolas',
                                                    fontSize: 12,
                                                  ),
                                                ),
                                              ),
                                              const SizedBox(width: 6),
                                              Expanded(
                                                child: Text(
                                                  row.argumentsPreview,
                                                  maxLines: 1,
                                                  overflow:
                                                      TextOverflow.ellipsis,
                                                  style: TextStyle(
                                                    fontFamily: 'Consolas',
                                                    fontSize: 12,
                                                    color: colors.muted,
                                                  ),
                                                ),
                                              ),
                                              Text(
                                                ' → ',
                                                style: TextStyle(
                                                  fontSize: 12,
                                                  color: colors.muted,
                                                ),
                                              ),
                                              Expanded(
                                                flex: 2,
                                                child: Text(
                                                  row.resultPreview,
                                                  maxLines: 1,
                                                  overflow:
                                                      TextOverflow.ellipsis,
                                                  style: const TextStyle(
                                                    fontFamily: 'Consolas',
                                                    fontSize: 12,
                                                  ),
                                                ),
                                              ),
                                            ],
                                          )
                                        : Text(
                                            item.summaryCount > 0
                                                ? '${item.summaryKind == 'turn' ? '轮次' : '调用'}已收起 · ${item.summaryCount} 项'
                                                : row.preview,
                                            maxLines: 1,
                                            overflow: TextOverflow.ellipsis,
                                            style: TextStyle(
                                              fontSize: 12,
                                              fontFamily: 'Consolas',
                                              color: colors.text,
                                            ),
                                          ),
                                  ),
                                ),
                                if (row.status == 'running')
                                  Padding(
                                    padding: const EdgeInsets.only(right: 8),
                                    child: Text(
                                      '运行中',
                                      style: TextStyle(
                                        fontSize: 10,
                                        color: colors.muted,
                                      ),
                                    ),
                                  ),
                              ],
                            ),
                          ),
                        ),
                      ),
                    ),
                  );
                },
              ),
      );
      return Column(
        children: [
          Container(
            height: 32,
            padding: const EdgeInsets.symmetric(horizontal: 8),
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: colors.border)),
            ),
            child: Row(
              children: [
                toggle(
                  '耗时',
                  actualDuration ? '按等宽操作显示' : '按实际耗时显示',
                  LucideIcons.clock3,
                  actualDuration,
                  () {
                    setState(() {
                      actualDuration = !actualDuration;
                      focusRange = null;
                    });
                    c.preferences.layout['traceActualDuration'] =
                        actualDuration;
                    c.run(c.preferences.save);
                  },
                ),
                toggle(
                  '轮次',
                  collapseTurns ? '展开轮次' : '收起轮次',
                  LucideIcons.panelLeft,
                  collapseTurns,
                  () => setState(() {
                    collapseTurns = !collapseTurns;
                    expandedTurns.clear();
                  }),
                ),
                toggle(
                  '调用',
                  collapseCalls ? '展开调用' : '收起调用',
                  LucideIcons.square,
                  collapseCalls,
                  () => setState(() {
                    collapseCalls = !collapseCalls;
                    expandedRequests.clear();
                  }),
                ),
                Expanded(
                  child: Align(
                    alignment: Alignment.centerRight,
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 164),
                      child: SizedBox(
                        height: 22,
                        child: TextField(
                          controller: search,
                          inputFormatters: [
                            LengthLimitingTextInputFormatter(512),
                          ],
                          style: const TextStyle(fontSize: 12),
                          onChanged: (_) => setState(() {
                            follow = false;
                          }),
                          decoration: InputDecoration(
                            isDense: true,
                            hintText: '搜索轨迹',
                            filled: true,
                            fillColor: colors.base,
                            contentPadding: const EdgeInsets.symmetric(
                              horizontal: 6,
                              vertical: 3,
                            ),
                            prefixIcon: DshGlyph(
                              LucideIcons.search,
                              size: 12,
                              color: colors.muted,
                            ),
                            prefixIconConstraints:
                                const BoxConstraints.tightFor(
                                  width: 22,
                                  height: 22,
                                ),
                            border: OutlineInputBorder(
                              borderRadius: BorderRadius.circular(4),
                              borderSide: BorderSide(color: colors.border),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
          TraceTimeline(
            spans: spans,
            selectedId: selectedId,
            range: focusRange,
            onFocus: focusRecord,
            onRange: (range) => setState(() => focusRange = range),
          ),
          if (c.window.hasBefore || c.window.hasAfter || c.loading)
            SizedBox(
              height: 30,
              child: Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  if (c.window.hasBefore)
                    DshButton(
                      height: 28,
                      fontSize: 12,
                      onPressed: c.loading ? null : () => loadOlder(display),
                      child: Text(c.loading ? '正在加载更早记录…' : '加载更早记录'),
                    ),
                  if (c.window.hasAfter)
                    DshButton(
                      height: 28,
                      fontSize: 12,
                      onPressed: c.loading
                          ? null
                          : () {
                              follow = true;
                              c.run(() => c.loadHistory());
                            },
                      child: const Text('返回最新'),
                    ),
                ],
              ),
            ),
          Expanded(
            child: LayoutBuilder(
              builder: (context, constraints) {
                final details = selected == null || !detailsOpen
                    ? null
                    : TraceDetails(
                        key: ValueKey(selected.id),
                        record: selected,
                        onClose: () => setState(() => detailsOpen = false),
                      );
                if (details == null) return ledger();
                if (constraints.maxWidth >= 760) {
                  return Row(
                    children: [
                      Expanded(child: ledger()),
                      SizedBox(width: 360, child: details),
                    ],
                  );
                }
                return Stack(
                  children: [
                    Positioned.fill(child: ledger()),
                    Positioned(
                      top: 0,
                      bottom: 0,
                      right: 0,
                      width: math.min(420, constraints.maxWidth * .92),
                      child: Material(elevation: 8, child: details),
                    ),
                  ],
                );
              },
            ),
          ),
        ],
      );
    },
  );
}

class TraceTimeline extends StatefulWidget {
  const TraceTimeline({
    super.key,
    required this.spans,
    required this.selectedId,
    required this.range,
    required this.onFocus,
    required this.onRange,
  });
  final List<TraceSpan> spans;
  final String? selectedId;
  final RangeValues? range;
  final ValueChanged<TraceRecord> onFocus;
  final ValueChanged<RangeValues?> onRange;
  @override
  State<TraceTimeline> createState() => _TraceTimelineState();
}

class _TraceTimelineState extends State<TraceTimeline> {
  double? start;
  void point(double x, double width) {
    if (widget.spans.isEmpty || width <= 0) return;
    final position = (x / width).clamp(0.0, 1.0);
    TraceSpan? best;
    var distance = double.infinity;
    for (final span in widget.spans) {
      final d = (position - (span.start + span.end) / 2).abs();
      if (d < distance) {
        distance = d;
        best = span;
      }
    }
    if (best != null) widget.onFocus(best.record);
  }

  void step(int direction) {
    if (widget.spans.isEmpty) return;
    final current = widget.spans.indexWhere(
      (s) => s.record.id == widget.selectedId,
    );
    widget.onFocus(
      widget
          .spans[(current + direction).clamp(0, widget.spans.length - 1)]
          .record,
    );
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Container(
      height: 50,
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: colors.border)),
      ),
      child: Row(
        children: [
          SizedBox(
            width: 44,
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                for (final label in ['输入', '模型', '工具'])
                  Padding(
                    padding: const EdgeInsets.only(right: 4),
                    child: SizedBox(
                      height: 14,
                      child: Text(
                        label,
                        style: TextStyle(fontSize: 10, color: colors.muted),
                      ),
                    ),
                  ),
              ],
            ),
          ),
          Expanded(
            child: LayoutBuilder(
              builder: (context, box) => Semantics(
                label: '时间线概览；横向拖动聚焦事件',
                onIncrease: () => step(1),
                onDecrease: () => step(-1),
                child: Focus(
                  onKeyEvent: (_, event) {
                    if (event is! KeyDownEvent) return KeyEventResult.ignored;
                    if (event.logicalKey == LogicalKeyboardKey.arrowRight) {
                      step(1);
                      return KeyEventResult.handled;
                    }
                    if (event.logicalKey == LogicalKeyboardKey.arrowLeft) {
                      step(-1);
                      return KeyEventResult.handled;
                    }
                    if (event.logicalKey == LogicalKeyboardKey.escape) {
                      widget.onRange(null);
                      return KeyEventResult.handled;
                    }
                    return KeyEventResult.ignored;
                  },
                  child: Builder(
                    builder: (focusContext) => Listener(
                      onPointerDown: (d) {
                        Focus.of(focusContext).requestFocus();
                        point(d.localPosition.dx, box.maxWidth);
                      },
                      child: GestureDetector(
                        key: const ValueKey('trajectory-timeline'),
                        behavior: HitTestBehavior.opaque,
                        onDoubleTap: () => widget.onRange(null),
                        onHorizontalDragStart: (d) => start =
                            (d.localPosition.dx / box.maxWidth).clamp(0, 1),
                        onHorizontalDragUpdate: (d) {
                          final end = (d.localPosition.dx / box.maxWidth).clamp(
                            0.0,
                            1.0,
                          );
                          widget.onRange(
                            RangeValues(
                              math.min(start ?? end, end),
                              math.max(start ?? end, end),
                            ),
                          );
                          point(d.localPosition.dx, box.maxWidth);
                        },
                        child: CustomPaint(
                          size: Size(box.maxWidth, 50),
                          painter: _TracePainter(
                            widget.spans,
                            widget.selectedId,
                            widget.range,
                            colors,
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _TracePainter extends CustomPainter {
  _TracePainter(this.spans, this.selected, this.range, this.colors);
  final List<TraceSpan> spans;
  final String? selected;
  final RangeValues? range;
  final DshColors colors;
  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint();
    if (range != null) {
      canvas.drawRect(
        Rect.fromLTRB(
          range!.start * size.width,
          0,
          range!.end * size.width,
          size.height,
        ),
        paint..color = colors.blue.withValues(alpha: .12),
      );
    }
    for (final span in spans) {
      final left = (span.start * size.width)
              .clamp(0.0, math.max(0.0, size.width - 2))
              .toDouble(),
          width = math
              .max(2.0, (span.end - span.start) * size.width - 2)
              .clamp(0.0, size.width - left)
              .toDouble();
      final rect = Rect.fromLTWH(left, 7 + span.record.lane * 14.0, width, 8),
          center = (span.start + span.end) / 2;
      final dim =
          range != null && (center < range!.start || center > range!.end);
      canvas.drawRRect(
        RRect.fromRectAndRadius(rect, const Radius.circular(1)),
        paint
          ..style = PaintingStyle.fill
          ..color = traceColor(
            span.record,
            colors,
          ).withValues(alpha: dim ? .20 : .85),
      );
      final firstToken = span.record.firstTokenTime,
          duration = span.record.durationMs;
      if (firstToken != null &&
          duration != null &&
          duration > 0 &&
          span.record.startTime != null) {
        final fraction = ((firstToken - span.record.startTime!) / duration)
            .clamp(0.0, 1.0);
        canvas.drawRect(
          Rect.fromLTWH(left, rect.top, width * fraction, rect.height),
          paint..color = colors.base.withValues(alpha: .40),
        );
      }
      if (span.record.id == selected) {
        canvas.drawRect(
          rect.inflate(1),
          paint
            ..style = PaintingStyle.stroke
            ..strokeWidth = 1
            ..color = colors.blue,
        );
      }
    }
  }

  @override
  bool shouldRepaint(_TracePainter old) =>
      old.spans != spans ||
      old.selected != selected ||
      old.range != range ||
      old.colors.dark != colors.dark;
}

class TraceDetails extends StatelessWidget {
  const TraceDetails({super.key, required this.record, required this.onClose});
  final TraceRecord record;
  final VoidCallback onClose;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context), row = record;
    final result = row.result,
        source = object(object(result?.data['message'])['source']),
        usage = object(result?.data['usage']);
    return Container(
      decoration: BoxDecoration(
        color: colors.base,
        border: Border(left: BorderSide(color: colors.border)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            height: 34,
            child: Row(
              children: [
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    '事件详情 · ${row.role}',
                    style: const TextStyle(fontSize: 13),
                  ),
                ),
                DshIcon(LucideIcons.x, label: '关闭事件详情', onPressed: onClose),
              ],
            ),
          ),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.all(12),
              children: [
                for (final field in <String, String>{
                  '状态':
                      const {
                        'running': '运行中',
                        'complete': '已完成',
                        'failed': '失败',
                        'interrupted': '已中断',
                      }[row.status] ??
                      '未提供',
                  '轮次／步骤': '${row.turn} / ${row.step}',
                  '开始时间': displayDate(row.startTime),
                  '结束时间': displayDate(row.endTime),
                  '总耗时': row.durationMs == null
                      ? '未提供'
                      : '${row.durationMs} ms',
                  '首 Token 耗时':
                      row.firstTokenTime == null || row.startTime == null
                      ? '未提供'
                      : '${math.max(0, row.firstTokenTime! - row.startTime!)} ms',
                  if (row.kind == 'assistant')
                    '提供方／模型':
                        '${source['provider'] ?? '未提供'} / ${source['model'] ?? '未提供'}',
                  if (usage.isNotEmpty)
                    '输出 Token': '${usage['outputTokens'] ?? '未提供'}',
                }.entries)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 10),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          field.key,
                          style: TextStyle(fontSize: 11, color: colors.muted),
                        ),
                        SelectableText(
                          field.value,
                          style: const TextStyle(fontSize: 12),
                        ),
                      ],
                    ),
                  ),
                SelectableText(
                  row.preview,
                  style: const TextStyle(fontSize: 12, height: 1.6),
                ),
                const SizedBox(height: 12),
                if (row.header != null)
                  DshButton(
                    height: 28,
                    fontSize: 12,
                    child: const Text('请求选项与系统提示词'),
                    onPressed: () => showDialog<void>(
                      context: context,
                      builder: (_) => EventDetail(event: row.header!),
                    ),
                  ),
                if (row.result != null)
                  DshButton(
                    height: 28,
                    fontSize: 12,
                    child: const Text('完整结果'),
                    onPressed: () => showDialog<void>(
                      context: context,
                      builder: (_) => EventDetail(event: row.result!),
                    ),
                  ),
                const SizedBox(height: 8),
                Text(
                  '原始事件 · ${row.events.length}',
                  style: TextStyle(fontSize: 11, color: colors.muted),
                ),
                SizedBox(
                  height: math.min(180, row.events.length * 28.0),
                  child: ListView.builder(
                    itemExtent: 28,
                    itemCount: row.events.length,
                    itemBuilder: (context, index) {
                      final event = row.events[index];
                      return DshButton(
                        height: 28,
                        fontSize: 11,
                        child: Text(
                          '#${event.seq} ${event.type}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                        onPressed: () => showDialog<void>(
                          context: context,
                          builder: (_) => EventDetail(event: event),
                        ),
                      );
                    },
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
