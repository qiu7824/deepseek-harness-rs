import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../design/statistics_popover.dart';
import '../../l10n/statistics_zh.dart';
import '../../src/controller.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

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
      DshConversationZh.turnsAndSteps(
        turns: nonNegative(stats, 'turns'),
        steps: nonNegative(stats, 'steps'),
      ),
    );
    final times = <String>[];
    if (nonNegative(stats, 'llmMs') > 0) {
      times.add('LLM ${compactDuration(nonNegative(stats, 'llmMs'))}');
    }
    if (nonNegative(stats, 'toolMs') > 0) {
      times.add(
        DshConversationZh.toolCallTime(
          duration: compactDuration(nonNegative(stats, 'toolMs')),
        ),
      );
    }
    if (times.isNotEmpty) groups.add(times.join(' · '));
    final rates = <String>[];
    if (nonNegative(stats, 'ttftSteps') > 0) {
      rates.add(
        DshConversationZh.averageFirstTokenTime(
          duration: compactDuration(
            nonNegative(stats, 'ttftMs') / nonNegative(stats, 'ttftSteps'),
          ),
        ),
      );
    }
    if (nonNegative(stats, 'requestMs') > 0 &&
        nonNegative(stats, 'requestSamples') > 0) {
      rates.add(
        DshConversationZh.averageRequestRate(
          rate: requestRate(
            nonNegative(stats, 'requestOutputTokens') *
                1000 /
                nonNegative(stats, 'requestMs'),
          ),
        ),
      );
    } else {
      rates.add(DshConversationZh.requestRateUnavailable);
    }
    groups.add(rates.join(' · '));
  }
  if (billedInputTokens(usage) > 0 || nonNegative(usage, 'outputTokens') > 0) {
    final hit = cacheHitPercent(usage);
    groups.add(
      hit == null
          ? DshConversationZh.cacheUsageUnavailable
          : DshConversationZh.cacheHitSummary(
              hit: hit,
              coverage:
                  nonNegative(
                        object(usage['cacheStatistics']),
                        'unreportedSamples',
                      ) >
                      0
                  ? DshConversationZh.partialRequestsSuffix
                  : '',
            ),
    );
    groups.add(
      DshConversationZh.inputOutputTokens(
        input: compactTokens(billedInputTokens(usage)),
        output: compactTokens(nonNegative(usage, 'outputTokens')),
      ),
    );
  }
  return groups.join('  |  ');
}

num? _statNumber(Json values, String key) {
  final value = values[key];
  return value is num && value.isFinite && value >= 0 ? value : null;
}

String _statAmount(Json values, String key, [String suffix = '']) {
  final value = _statNumber(values, key);
  return value == null
      ? DshStatisticsZh.unavailable
      : '${oneDecimal(value)}$suffix';
}

class SessionStatsLine extends StatelessWidget {
  const SessionStatsLine({super.key, required this.controller});
  final DesktopController controller;
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([controller, controller.projectionChanges]),
    builder: (context, _) {
      final stats = object(controller.projections['sessionStats']);
      final usage = object(controller.projections['tokenUsage']);
      final scope = (controller.client, controller.selectedId);
      final requestMs = _statNumber(stats, 'requestMs');
      final requestTokens = _statNumber(stats, 'requestOutputTokens');
      final samples = _statNumber(stats, 'requestSamples');
      final ttftMs = _statNumber(stats, 'ttftMs');
      final ttftSteps = _statNumber(stats, 'ttftSteps');
      final inputParts = [
        'uncachedInputTokens',
        'cacheReadTokens',
        'cacheWriteTokens',
      ];
      final input = inputParts.every((key) => _statNumber(usage, key) != null)
          ? billedInputTokens(usage)
          : null;
      final output = _statNumber(usage, 'outputTokens');
      final total = input != null && output != null ? input + output : null;
      final hit = cacheHitPercent(usage);
      final partial =
          nonNegative(object(usage['cacheStatistics']), 'unreportedSamples') >
          0;
      final sources = objects(stats['requestSources'])
          .map((s) => '${s['provider'] ?? '—'} / ${s['model'] ?? '—'}')
          .toSet();
      String duration(String key) => _statNumber(stats, key) == null
          ? DshStatisticsZh.unavailable
          : '${oneDecimal(_statNumber(stats, key)! / 1000)}秒';
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: Wrap(
          alignment: WrapAlignment.center,
          spacing: 4,
          runSpacing: 2,
          children: [
            StatisticsPopover(
              key: const ValueKey('session-statistics-button'),
              scope: scope,
              label: DshStatisticsZh.session,
              title: DshStatisticsZh.session,
              icon: DshIcons.clock.data,
              rows: [
                (DshStatisticsZh.rounds, _statAmount(stats, 'turns')),
                (DshStatisticsZh.steps, _statAmount(stats, 'steps')),
                (DshStatisticsZh.modelTime, duration('llmMs')),
                (DshStatisticsZh.toolTime, duration('toolMs')),
                (
                  DshStatisticsZh.firstToken,
                  ttftMs != null && ttftSteps != null && ttftSteps > 0
                      ? '${oneDecimal(ttftMs / ttftSteps / 1000)}秒'
                      : DshStatisticsZh.unavailable,
                ),
                (
                  DshStatisticsZh.outputSpeed,
                  requestMs != null &&
                          requestMs > 0 &&
                          samples != null &&
                          samples > 0 &&
                          requestTokens != null
                      ? '${requestRate(requestTokens * 1000 / requestMs)} tok/s'
                      : DshStatisticsZh.unavailable,
                ),
                if (sources.isNotEmpty)
                  (DshStatisticsZh.sources, sources.join('\n')),
              ],
            ),
            StatisticsPopover(
              key: const ValueKey('session-usage-button'),
              scope: scope,
              label: total == null
                  ? DshStatisticsZh.usage
                  : '${DshStatisticsZh.usage} ${compactTokens(total)} tok',
              title: DshStatisticsZh.sessionUsage,
              icon: DshIcons.database.data,
              rows: [
                (
                  DshStatisticsZh.total,
                  total == null
                      ? DshStatisticsZh.unavailable
                      : '${oneDecimal(total)} tok',
                ),
                (
                  DshStatisticsZh.input,
                  input == null
                      ? DshStatisticsZh.unavailable
                      : '${oneDecimal(input)} tok',
                ),
                (
                  DshStatisticsZh.output,
                  _statAmount(usage, 'outputTokens', ' tok'),
                ),
                (
                  DshStatisticsZh.uncached,
                  _statAmount(usage, 'uncachedInputTokens', ' tok'),
                ),
                (
                  DshStatisticsZh.cacheRead,
                  _statAmount(usage, 'cacheReadTokens', ' tok'),
                ),
                (
                  DshStatisticsZh.cacheWrite,
                  _statAmount(usage, 'cacheWriteTokens', ' tok'),
                ),
                (
                  DshStatisticsZh.cacheHit,
                  hit == null
                      ? DshStatisticsZh.unavailable
                      : '$hit%${partial ? DshStatisticsZh.partial : ''}',
                ),
              ],
            ),
            ContextMeter(
              key: const ValueKey('session-context-button'),
              controller: controller,
            ),
          ],
        ),
      );
    },
  );
}

class ContextMeter extends StatelessWidget {
  const ContextMeter({super.key, required this.controller});
  final DesktopController controller;
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([controller, controller.projectionChanges]),
    builder: (context, _) {
      final reading = ContextOccupancy.fromJson(
        object(controller.projections['contextPressure']),
      );
      return StatisticsPopover(
        scope: (controller.client, controller.selectedId, reading != null),
        label: reading == null
            ? DshStatisticsZh.context
            : '${DshStatisticsZh.context} ${reading.percent}%',
        title: DshStatisticsZh.context,
        icon: DshIcons.brain.data,
        rows: const [],
        details: ContextSummary(controller: controller),
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
            reading == null
                ? DshConversationZh.contextUsageUnavailable
                : DshConversationZh.contextPercent(percent: reading.percent),
            style: const TextStyle(
              fontSize: DshTypography.sizeBody,
              fontWeight: FontWeight.w500,
            ),
          ),
          if (reading != null)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '${compactTokens(reading.used)} / ${compactTokens(reading.capacity)} tokens${reading.estimated ? DshConversationZh.estimatedCapacitySuffix : ''}',
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: colors.muted,
                ),
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
                        const [
                          DshConversationZh.systemPrompt,
                          DshConversationZh.toolDefinitions,
                          DshConversationZh.conversationContent,
                        ][i],
                        style: const TextStyle(
                          fontSize: DshTypography.sizeCaption,
                        ),
                      ),
                    ),
                    Text(
                      compactTokens(slices[i]),
                      style: const TextStyle(
                        fontSize: DshTypography.sizeCaption,
                      ),
                    ),
                  ],
                ),
              ),
            Text(
              DshConversationZh.contextEstimateHint,
              style: TextStyle(
                fontSize: DshTypography.sizeCaption,
                height: 1.5,
                color: colors.muted,
              ),
            ),
          ],
          if (controller.projectionWindow.oversized.isNotEmpty)
            Text(
              DshConversationZh.statusDisplayLimit,
              style: TextStyle(
                fontSize: DshTypography.sizeCaption,
                color: colors.muted,
              ),
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

  Object? error;
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
      if (mounted) setState(() => error = e);
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
        title: DshConversationZh.editTask,
        initial: '${expected[index]['content']}',
        onSave: (text) async {
          if (!mounted || c.selectedId != session) {
            throw StateError(DshConversationZh.taskSessionChanged);
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
      DshConversationZh.removeTask,
      '${expected[index]['content']}',
      action: DshConversationZh.remove,
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
        throw StateError(DshConversationZh.goalSessionChanged);
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
        !await confirmAction(
          context,
          DshConversationZh.clearGoal,
          DshConversationZh.clearGoalHint,
          action: DshConversationZh.clear,
        )) {
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
                      style: const TextStyle(
                        fontSize: DshTypography.sizeAuxiliary,
                        height: 20 / 13,
                      ),
                      decoration: const InputDecoration(
                        isDense: true,
                        contentPadding: EdgeInsets.symmetric(
                          horizontal: 8,
                          vertical: 3,
                        ),
                        border: OutlineInputBorder(),
                        hintText: DshConversationZh.goal,
                      ),
                      onSubmitted: (_) => saveGoal(),
                    ),
                  ),
                ),
                const SizedBox(width: 10),
                ValueListenableBuilder(
                  valueListenable: goalDraft,
                  builder: (_, value, _) => DshIcon(
                    DshIcons.check.data,
                    glyphSize: 14,
                    size: 28,
                    label: DshConversationZh.saveGoal,
                    onPressed: busy || value.text.trim().isEmpty
                        ? null
                        : saveGoal,
                  ),
                ),
                const SizedBox(width: 10),
                DshIcon(
                  DshIcons.close.data,
                  glyphSize: 14,
                  size: 28,
                  label: DshConversationZh.cancelGoalEdit,
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
                      'active': DshConversationZh.activeGoal,
                      'paused': DshConversationZh.pausedGoal,
                      'blocked': DshConversationZh.blockedGoal,
                    }[goal['phase']] ??
                    DshConversationZh.goal,
                style: const TextStyle(
                  fontSize: DshTypography.sizeAuxiliary,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Tooltip(
                  message: DshConversationZh.goalProgress(
                    objective: goal['objective'],
                    started: projected['roundsStarted'] ?? 0,
                    maximum: goal['maxGoalRounds'] ?? '—',
                    blockedDetail: goal['blockedReason'] is Map
                        ? '\n${object(goal['blockedReason'])['message'] ?? ''}'
                        : '',
                  ),
                  child: Text(
                    '${goal['objective']}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      fontSize: DshTypography.sizeAuxiliary,
                    ),
                  ),
                ),
              ),
              if (goal['phase'] == 'active')
                DshIcon(
                  DshIcons.pause.data,
                  label: DshConversationZh.pauseGoal,
                  glyphSize: 14,
                  size: 28,
                  onPressed: busy ? null : () => goalAction('pause', goal),
                ),
              if (['paused', 'blocked'].contains(goal['phase']))
                DshIcon(
                  DshIcons.play.data,
                  label: DshConversationZh.resumeGoal,
                  asset: 'assets/icons/goal-resume.svg',
                  glyphSize: 14,
                  size: 28,
                  onPressed: busy ? null : () => goalAction('resume', goal),
                ),
              const SizedBox(width: 10),
              DshIcon(
                DshIcons.pencil.data,
                label: DshConversationZh.editGoal,
                glyphSize: 14,
                size: 28,
                onPressed: busy ? null : () => goalAction('edit', goal),
              ),
              const SizedBox(width: 10),
              DshIcon(
                DshIcons.trash2.data,
                label: DshConversationZh.clearGoal,
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
                fontSize: DshTypography.sizeAuxiliary,
                decoration: todo['status'] == 'completed'
                    ? TextDecoration.lineThrough
                    : null,
              ),
            ),
          ),
          const SizedBox(width: 10),
          DshIcon(
            DshIcons.pencil.data,
            label: c.canEditTodos
                ? DshConversationZh.editTask
                : DshConversationZh.editTaskRequiresHostUpdate,
            size: 25,
            onPressed: busy || !c.canEditTodos
                ? null
                : () => editTodo(todos, index),
          ),
          if (todo['status'] != 'completed')
            DshIcon(
              DshIcons.trash2.data,
              label: c.canEditTodos
                  ? DshConversationZh.removeTask
                  : DshConversationZh.removeTaskRequiresHostUpdate,
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
        if (done > 0) DshConversationZh.completedCount(count: done),
        if (active > 0) DshConversationZh.activeCount(count: active),
        if (pending > 0) DshConversationZh.pendingCount(count: pending),
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
                                DshConversationZh.task,
                                style: TextStyle(
                                  fontSize: DshTypography.sizeAuxiliary,
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
                                    fontSize: DshTypography.sizeAuxiliary,
                                    color: colors.muted,
                                  ),
                                ),
                              ),
                              DshGlyph(
                                expanded
                                    ? DshIcons.chevronDown.data
                                    : DshIcons.chevronUp.data,
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
                            child: const Text(
                              DshConversationZh.stopCurrentExecution,
                            ),
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
            if (error != null) DshErrorView(error: error!),
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
  Object? error;
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
      if (mounted) setState(() => error = e);
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: AlertDialog(
      title: Text(
        widget.title,
        style: const TextStyle(fontSize: DshTypography.sizeSectionTitle),
      ),
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
                child: DshErrorView(error: error!),
              ),
          ],
        ),
      ),
      actions: [
        DshButton(
          onPressed: busy ? null : () => Navigator.pop(context),
          child: const Text(DshConversationZh.cancel),
        ),
        DshButton(
          primary: true,
          onPressed: busy ? null : save,
          child: Text(busy ? DshConversationZh.saving : DshConversationZh.save),
        ),
      ],
    ),
  );
}
