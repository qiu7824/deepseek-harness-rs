import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

String oneDecimal(num n) =>
    n.toStringAsFixed(1).replaceFirst(RegExp(r'\.0$'), '');
String compactTokens(num n) => n < 1000
    ? '${n.round()}'
    : n < 1000000
    ? '${n >= 100000 ? (n / 1000).round() : oneDecimal(n / 1000)}K'
    : '${n >= 100000000 ? (n / 1000000).round() : oneDecimal(n / 1000000)}M';
String compactDuration(num ms) => ms < 60000
    ? '${oneDecimal(ms / 1000)}s'
    : '${(ms / 60000).floor()}m${((ms / 1000).round() % 60)}s';
String requestRate(num rate) =>
    rate >= 10 ? '${rate.round()}' : oneDecimal(rate);

String sessionStatsText(Json projections) {
  final stats = object(projections['sessionStats']),
      usage = object(projections['tokenUsage']);
  final groups = <String>[];
  if (nonNegative(stats, 'steps') > 0) {
    groups.add(
      '${nonNegative(stats, 'turns')} 轮 · ${nonNegative(stats, 'steps')} 步',
    );
    final times = <String>[];
    if (nonNegative(stats, 'llmMs') > 0) {
      times.add('LLM ${compactDuration(nonNegative(stats, 'llmMs'))}');
    }
    if (nonNegative(stats, 'toolMs') > 0) {
      times.add('工具调用 ${compactDuration(nonNegative(stats, 'toolMs'))}');
    }
    if (times.isNotEmpty) groups.add(times.join(' · '));
    final rates = <String>[];
    if (nonNegative(stats, 'ttftSteps') > 0) {
      rates.add(
        '首 token 平均 ${compactDuration(nonNegative(stats, 'ttftMs') / nonNegative(stats, 'ttftSteps'))}',
      );
    }
    if (nonNegative(stats, 'requestMs') > 0 &&
        nonNegative(stats, 'requestSamples') > 0) {
      rates.add(
        '请求平均 ${requestRate(nonNegative(stats, 'requestOutputTokens') * 1000 / nonNegative(stats, 'requestMs'))} tok/s',
      );
    } else {
      rates.add('请求速率未提供');
    }
    groups.add(rates.join(' · '));
  }
  if (billedInputTokens(usage) > 0 || nonNegative(usage, 'outputTokens') > 0) {
    final hit = cacheHitPercent(usage);
    groups.add(
      hit == null
          ? '缓存统计未提供'
          : '缓存命中 $hit%${nonNegative(object(usage['cacheStatistics']), 'unreportedSamples') > 0 ? '（部分请求）' : ''}',
    );
    groups.add(
      '输入 ${compactTokens(billedInputTokens(usage))} tok · 输出 ${compactTokens(nonNegative(usage, 'outputTokens'))} tok',
    );
  }
  return groups.join('  |  ');
}

class SessionStatsLine extends StatelessWidget {
  const SessionStatsLine({super.key, required this.controller});
  final DesktopController controller;
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller.projectionChanges,
    builder: (context, _) {
      final line = sessionStatsText(controller.projections);
      if (line.isEmpty) return const SizedBox(height: 18);
      final sources = objects(
        object(controller.projections['sessionStats'])['requestSources'],
      );
      return Tooltip(
        message: [
          line,
          ...sources.map(
            (s) =>
                '${s['provider']} / ${s['model']} · ${s['executionInstanceId'] ?? ''}',
          ),
        ].join('\n'),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(48, 28, 48, 8),
          child: SingleChildScrollView(
            key: const ValueKey('session-stats-scroll'),
            scrollDirection: Axis.horizontal,
            child: Text(
              line,
              maxLines: 1,
              softWrap: false,
              style: TextStyle(
                fontSize: 12,
                height: 20 / 12,
                color: DshColors(context).muted,
              ),
            ),
          ),
        ),
      );
    },
  );
}

class ContextMeter extends StatefulWidget {
  const ContextMeter({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<ContextMeter> createState() => _ContextMeterState();
}

class _ContextMeterState extends State<ContextMeter> {
  final popover = ShadPopoverController();
  @override
  void dispose() {
    popover.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.controller.projectionChanges,
    builder: (context, _) {
      final reading = ContextOccupancy.fromJson(
        object(widget.controller.projections['contextPressure']),
      );
      if (reading == null) {
        if (popover.isOpen) {
          WidgetsBinding.instance.addPostFrameCallback((_) {
            if (mounted) popover.hide();
          });
        }
        return const SizedBox();
      }
      return CallbackShortcuts(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.escape): popover.hide,
        },
        child: ShadPopover(
          controller: popover,
          padding: const EdgeInsets.all(12),
          anchor: const ShadAnchorAuto(
            targetAnchor: Alignment.topRight,
            followerAnchor: Alignment.bottomRight,
            offset: Offset(0, -8),
          ),
          popover: (_) => SizedBox(
            width: 240,
            child: ContextSummary(controller: widget.controller),
          ),
          child: Tooltip(
            message: '上下文已用 ${reading.percent}%',
            child: Semantics(
              label: '上下文已用 ${reading.percent}%',
              button: true,
              child: ShadButton.ghost(
                width: 28,
                height: 28,
                padding: EdgeInsets.zero,
                onPressed: popover.toggle,
                child: SizedBox(
                  width: 14,
                  height: 14,
                  child: CircularProgressIndicator(
                    value: reading.ratio,
                    strokeWidth: 2,
                    backgroundColor: DshColors(context).border,
                    color: DshColors(context).muted,
                  ),
                ),
              ),
            ),
          ),
        ),
      );
    },
  );
}

class ContextSummary extends StatelessWidget {
  const ContextSummary({super.key, required this.controller});
  final DesktopController controller;
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller.projectionChanges,
    builder: (context, _) {
      final reading = ContextOccupancy.fromJson(
        object(controller.projections['contextPressure']),
      );
      final breakdown = object(controller.projections['contextBreakdown']),
          colors = DshColors(context);
      final slices = [
        nonNegative(breakdown, 'systemTokens'),
        nonNegative(breakdown, 'toolsTokens'),
        nonNegative(breakdown, 'messageTokens'),
      ];
      final sum = slices.fold<num>(0, (a, b) => a + b);
      final hues = [
        const Color(0xffadb2b8),
        const Color(0xffa78bfa),
        const Color(0xff4d93f8),
      ];
      return Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            reading == null ? '上下文占用尚未提供' : '上下文已用 ${reading.percent}%',
            style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w500),
          ),
          if (reading != null)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '${compactTokens(reading.used)} / ${compactTokens(reading.capacity)} tokens${reading.estimated ? ' · 容量估算' : ''}',
                style: TextStyle(fontSize: 12, color: colors.muted),
              ),
            ),
          if (sum > 0) ...[
            const SizedBox(height: 10),
            ClipRRect(
              borderRadius: BorderRadius.circular(4),
              child: SizedBox(
                height: 4,
                child: Row(
                  children: [
                    for (var i = 0; i < 3; i++)
                      if (slices[i] > 0)
                        Expanded(
                          flex: (slices[i] / sum * 10000).round().clamp(
                            1,
                            10000,
                          ),
                          child: ColoredBox(
                            color: hues[i],
                            child: const SizedBox.expand(),
                          ),
                        ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 10),
            for (var i = 0; i < 3; i++)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 3),
                child: Row(
                  children: [
                    Container(width: 8, height: 8, color: hues[i]),
                    const SizedBox(width: 6),
                    Expanded(
                      child: Text(
                        const ['系统提示词', '工具定义', '对话内容'][i],
                        style: const TextStyle(fontSize: 12),
                      ),
                    ),
                    Text(
                      compactTokens(slices[i]),
                      style: const TextStyle(fontSize: 12),
                    ),
                  ],
                ),
              ),
            Text(
              '组成按启发式估算，与供应商计费用量可能不同。',
              style: TextStyle(fontSize: 11, height: 1.5, color: colors.muted),
            ),
          ],
          if (controller.projectionWindow.oversized.isNotEmpty)
            Text(
              '部分状态数据超过显示预算，未载入。',
              style: TextStyle(fontSize: 12, color: colors.muted),
            ),
        ],
      );
    },
  );
}

class ProgressDock extends StatefulWidget {
  const ProgressDock({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<ProgressDock> createState() => _ProgressDockState();
}

class _ProgressDockState extends State<ProgressDock> {
  bool expanded = false, busy = false;
  final goalDraft = TextEditingController();
  Json? editingGoal;
  String? editingSession;
  DshClient? editingClient;
  @override
  void dispose() {
    goalDraft.dispose();
    super.dispose();
  }

  Future<void> saveGoal() async {
    final goal = editingGoal, text = goalDraft.text.trim();
    if (goal == null || text.isEmpty || busy) return;
    if (c.selectedId != editingSession || c.client != editingClient) {
      setState(() => editingGoal = null);
      return;
    }
    await action(() async {
      await c.changeGoal('edit', goal, objective: text);
      if (mounted) setState(() => editingGoal = null);
    });
  }

  String? error;
  Object? todoSource;
  List<Json> retainedTodos = [];
  DesktopController get c => widget.controller;
  Future<void> action(Future<void> Function() work) async {
    if (busy) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await work();
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> editTodo(List<Json> expected, int index) async {
    final session = c.selectedId;
    await showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (_) => ProjectionTextEditor(
        title: '编辑任务',
        initial: '${expected[index]['content']}',
        onSave: (text) async {
          if (!mounted || c.selectedId != session) {
            throw StateError('会话已切换，请保留草稿后重新打开任务。');
          }
          await c.updateTodos(expected, {
            'kind': 'edit',
            'index': index,
            'content': text,
          });
        },
      ),
    );
  }

  Future<void> removeTodo(List<Json> expected, int index) async {
    final session = c.selectedId;
    if (!await confirmAction(
      context,
      '移除任务',
      '${expected[index]['content']}',
      action: '移除',
    )) {
      return;
    }
    if (!mounted || c.selectedId != session) return;
    await action(
      () => c.updateTodos(expected, {'kind': 'remove', 'index': index}),
    );
  }

  Future<void> goalAction(String operation, Json expected) async {
    final session = c.selectedId;
    Future<void> commit({String? objective}) async {
      if (!mounted || session == null || c.selectedId != session) {
        throw StateError('会话已切换，请重新打开目标。');
      }
      await c.changeGoal(operation, expected, objective: objective);
    }

    if (operation == 'edit') {
      setState(() {
        editingGoal = expected;
        editingSession = session;
        editingClient = c.client;
        goalDraft.text = '${expected['objective']}';
      });
      return;
    }
    if (operation == 'clear' &&
        !await confirmAction(context, '清除目标', '清除当前目标，保留会话历史。', action: '清除')) {
      return;
    }
    if (mounted) await action(commit);
  }

  Widget goalRow(Json projected, Json goal, DshColors colors) => Container(
    key: const ValueKey('goal-status-bar'),
    height: 36,
    padding: const EdgeInsets.only(left: 12, right: 5),
    decoration: BoxDecoration(
      color: colors.layer,
      border: Border.all(color: colors.border),
      borderRadius: BorderRadius.circular(12),
    ),
    child:
        editingGoal != null &&
            editingSession == c.selectedId &&
            editingClient == c.client
        ? CallbackShortcuts(
            bindings: {
              const SingleActivator(LogicalKeyboardKey.escape): () {
                if (!busy) setState(() => editingGoal = null);
              },
            },
            child: Row(
              children: [
                Expanded(
                  child: SizedBox(
                    height: 26,
                    child: TextField(
                      key: const ValueKey('goal-objective-input'),
                      controller: goalDraft,
                      autofocus: true,
                      enabled: !busy,
                      style: const TextStyle(fontSize: 13, height: 20 / 13),
                      decoration: const InputDecoration(
                        isDense: true,
                        contentPadding: EdgeInsets.symmetric(
                          horizontal: 8,
                          vertical: 3,
                        ),
                        border: OutlineInputBorder(),
                        hintText: '目标',
                      ),
                      onSubmitted: (_) => saveGoal(),
                    ),
                  ),
                ),
                const SizedBox(width: 10),
                ValueListenableBuilder(
                  valueListenable: goalDraft,
                  builder: (_, value, _) => DshIcon(
                    LucideIcons.check,
                    glyphSize: 14,
                    size: 28,
                    label: '保存目标',
                    onPressed: busy || value.text.trim().isEmpty
                        ? null
                        : saveGoal,
                  ),
                ),
                const SizedBox(width: 10),
                DshIcon(
                  LucideIcons.x,
                  glyphSize: 14,
                  size: 28,
                  label: '取消编辑目标',
                  onPressed: busy
                      ? null
                      : () => setState(() => editingGoal = null),
                ),
              ],
            ),
          )
        : Row(
            children: [
              DshGlyph(
                null,
                asset: 'assets/icons/goal.svg',
                size: 14,
                color: colors.muted,
              ),
              const SizedBox(width: 10),
              Text(
                const {
                      'active': '进行中的目标',
                      'paused': '已暂停的目标',
                      'blocked': '受阻的目标',
                    }[goal['phase']] ??
                    '目标',
                style: const TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Tooltip(
                  message:
                      '${goal['objective']}\n${projected['roundsStarted'] ?? 0} / ${goal['maxGoalRounds'] ?? '—'} 轮${goal['blockedReason'] is Map ? '\n${object(goal['blockedReason'])['message'] ?? ''}' : ''}',
                  child: Text(
                    '${goal['objective']}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 13),
                  ),
                ),
              ),
              if (goal['phase'] == 'active')
                DshIcon(
                  LucideIcons.pause,
                  label: '暂停目标',
                  glyphSize: 14,
                  size: 28,
                  onPressed: busy ? null : () => goalAction('pause', goal),
                ),
              if (['paused', 'blocked'].contains(goal['phase']))
                DshIcon(
                  LucideIcons.play,
                  label: '恢复目标',
                  asset: 'assets/icons/goal-resume.svg',
                  glyphSize: 14,
                  size: 28,
                  onPressed: busy ? null : () => goalAction('resume', goal),
                ),
              const SizedBox(width: 10),
              DshIcon(
                LucideIcons.pencil,
                label: '编辑目标',
                glyphSize: 14,
                size: 28,
                onPressed: busy ? null : () => goalAction('edit', goal),
              ),
              const SizedBox(width: 10),
              DshIcon(
                LucideIcons.trash2,
                label: '清除目标',
                glyphSize: 14,
                size: 28,
                onPressed: busy ? null : () => goalAction('clear', goal),
              ),
            ],
          ),
  );
  Widget todoRow(List<Json> todos, int index, DshColors colors) {
    final todo = todos[index];
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Row(
        children: [
          DshGlyph(
            null,
            asset: 'assets/icons/todo-${todo['status']}.svg',
            size: 14,
            color: todo['status'] == 'completed'
                ? const Color(0xff22c55e)
                : todo['status'] == 'in_progress'
                ? colors.blue
                : colors.muted,
          ),
          const SizedBox(width: 10),
          Flexible(
            fit: FlexFit.loose,
            child: Text(
              '${todo['content']}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 13,
                decoration: todo['status'] == 'completed'
                    ? TextDecoration.lineThrough
                    : null,
              ),
            ),
          ),
          const SizedBox(width: 10),
          DshIcon(
            LucideIcons.pencil,
            label: c.canEditTodos ? '编辑任务' : '编辑任务需要更新 Host',
            size: 25,
            onPressed: busy || !c.canEditTodos
                ? null
                : () => editTodo(todos, index),
          ),
          if (todo['status'] != 'completed')
            DshIcon(
              LucideIcons.trash2,
              label: c.canEditTodos ? '移除任务' : '移除任务需要更新 Host',
              size: 25,
              onPressed: busy || !c.canEditTodos
                  ? null
                  : () => removeTodo(todos, index),
            ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: c.projectionChanges,
    builder: (context, _) {
      if (!identical(todoSource, c.projections['todos'])) {
        todoSource = c.projections['todos'];
        retainedTodos = objects(todoSource);
      }
      final todos = retainedTodos,
          projected = object(c.projections['goal']),
          goal = object(object(c.projections['goal'])['goal']);
      final visibleGoal = goal.isNotEmpty && goal['phase'] != 'complete';
      if (todos.isEmpty && !visibleGoal) return const SizedBox();
      final done = todos.where((t) => t['status'] == 'completed').length,
          active = todos.where((t) => t['status'] == 'in_progress').length,
          colors = DshColors(context);
      final pending = todos.length - done - active;
      final summary = [
        if (done > 0) '$done 已完成',
        if (active > 0) '$active 进行中',
        if (pending > 0) '$pending 待处理',
      ].join(' · ');
      return Padding(
        padding: const EdgeInsets.only(bottom: 6),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (todos.isNotEmpty)
              Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 12,
                  vertical: 6,
                ),
                decoration: BoxDecoration(
                  color: colors.layer,
                  border: Border.all(color: colors.border),
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    LayoutBuilder(
                      builder: (context, constraints) => DshButton(
                        key: const ValueKey('todo-disclosure'),
                        height: 24,
                        padding: EdgeInsets.zero,
                        onPressed: () => setState(() => expanded = !expanded),
                        child: SizedBox(
                          width: constraints.maxWidth,
                          child: Row(
                            children: [
                              DshGlyph(
                                null,
                                asset: 'assets/icons/task-list.svg',
                                size: 14,
                                color: colors.muted,
                              ),
                              const SizedBox(width: 10),
                              const Text(
                                '任务',
                                style: TextStyle(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w500,
                                ),
                              ),
                              const SizedBox(width: 10),
                              Expanded(
                                child: Text(
                                  summary,
                                  textAlign: TextAlign.start,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                    fontSize: 13,
                                    color: colors.muted,
                                  ),
                                ),
                              ),
                              DshGlyph(
                                expanded
                                    ? LucideIcons.chevronDown
                                    : LucideIcons.chevronUp,
                                size: 14,
                                color: colors.muted,
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                    if (expanded) ...[
                      const SizedBox(height: 8),
                      SizedBox(
                        height: (todos.length * 33 - 8)
                            .clamp(25, 180)
                            .toDouble(),
                        child: ListView.builder(
                          itemExtent: 33,
                          itemCount: todos.length,
                          itemBuilder: (context, index) =>
                              todoRow(todos, index, colors),
                        ),
                      ),
                      if (c.running)
                        Align(
                          alignment: Alignment.centerRight,
                          child: DshButton(
                            height: 28,
                            destructive: true,
                            onPressed: busy ? null : () => action(c.stop),
                            child: const Text('停止当前任务'),
                          ),
                        ),
                    ],
                  ],
                ),
              ),
            if (visibleGoal) ...[
              if (todos.isNotEmpty) const SizedBox(height: 6),
              goalRow(projected, goal, colors),
            ],
            if (error != null)
              Text(
                error!,
                style: const TextStyle(fontSize: 12, color: Colors.red),
              ),
          ],
        ),
      );
    },
  );
}

class ProjectionTextEditor extends StatefulWidget {
  const ProjectionTextEditor({
    super.key,
    required this.title,
    required this.initial,
    required this.onSave,
    this.maxLines = 6,
    this.width = 520,
  });
  final String title, initial;
  final int maxLines;
  final double width;
  final Future<void> Function(String) onSave;
  @override
  State<ProjectionTextEditor> createState() => _ProjectionTextEditorState();
}

class _ProjectionTextEditorState extends State<ProjectionTextEditor> {
  late final input = TextEditingController(text: widget.initial);
  bool busy = false;
  String? error;
  @override
  void dispose() {
    input.dispose();
    super.dispose();
  }

  Future<void> save() async {
    if (busy || input.text.trim().isEmpty) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await widget.onSave(input.text.trim());
      if (mounted) Navigator.pop(context);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: AlertDialog(
      title: Text(widget.title, style: const TextStyle(fontSize: 17)),
      content: SizedBox(
        width: widget.width,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            DshField(
              controller: input,
              maxLines: widget.maxLines,
              autofocus: true,
            ),
            if (error != null)
              Padding(
                padding: const EdgeInsets.only(top: 12),
                child: Text(
                  error!,
                  style: const TextStyle(fontSize: 12, color: Colors.red),
                ),
              ),
          ],
        ),
      ),
      actions: [
        DshButton(
          onPressed: busy ? null : () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        DshButton(
          primary: true,
          onPressed: busy ? null : save,
          child: Text(busy ? '保存中…' : '保存'),
        ),
      ],
    ),
  );
}
