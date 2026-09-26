import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../design/typography.dart';
import '../../src/controller.dart';

/// Host-owned scheduled tasks over `/__dsh-schedule/*`.
class ScheduleApi {
  ScheduleApi(this.client);
  final DshClient client;
  static const _reads = {'catalog', 'history', 'wait'};

  Future<Json> call(
    String operation, [
    Json body = const {},
    RequestScope? scope,
  ]) => client.request(
    '/__dsh-schedule/$operation',
    body: body,
    scope: scope,
    mutation: !_reads.contains(operation),
    maxBytes: 4 * 1024 * 1024,
  );
}

const scheduleWeekdayNames = ['周一', '周二', '周三', '周四', '周五', '周六', '周日'];

String _two(int value) => value.toString().padLeft(2, '0');

String formatScheduleTime(String? iso) {
  final value = iso == null ? null : DateTime.tryParse(iso)?.toLocal();
  if (value == null) return '';
  final now = DateTime.now();
  final clock = '${_two(value.hour)}:${_two(value.minute)}';
  final date = value.year == now.year
      ? '${value.month}月${value.day}日'
      : '${value.year}年${value.month}月${value.day}日';
  return '$date $clock';
}

String relativeScheduleTime(String? iso, {DateTime? now}) {
  final value = iso == null ? null : DateTime.tryParse(iso);
  if (value == null) return '';
  final delta = value.difference(now ?? DateTime.now());
  if (delta <= Duration.zero) return '即将运行';
  if (delta < const Duration(hours: 1)) {
    return '${delta.inMinutes.clamp(1, 59)} 分钟后';
  }
  if (delta < const Duration(days: 1)) {
    return '${(delta.inMinutes / 60).round()} 小时后';
  }
  return '${(delta.inHours / 24).round()} 天后';
}

/// Chinese label for one stored rule, e.g. `每周一、周五 18:30`.
String scheduleRuleLabel(Json rule, {String? localZone}) {
  final zone = rule['timeZone'] as String?;
  final zoneNote = zone != null && localZone != null && zone != localZone
      ? '（$zone）'
      : '';
  switch (rule['kind']) {
    case 'at':
      return '单次 · ${formatScheduleTime(rule['at'] as String?)}';
    case 'every':
      final seconds = (rule['everySeconds'] as num?)?.toInt() ?? 0;
      if (seconds % 86400 == 0) return '每 ${seconds ~/ 86400} 天';
      if (seconds % 3600 == 0) return '每 ${seconds ~/ 3600} 小时';
      return '每 ${(seconds / 60).round()} 分钟';
    case 'daily':
      return '每天 ${rule['time']}$zoneNote';
    case 'weekly':
      final days = [
        for (final day in (rule['weekdays'] as List? ?? []))
          if (day is num && day >= 1 && day <= 7)
            scheduleWeekdayNames[day.toInt() - 1],
      ];
      return days.length == 7
          ? '每天 ${rule['time']}$zoneNote'
          : '每${days.join('、')} ${rule['time']}$zoneNote';
    case 'cron':
      return 'Cron · ${rule['expression']}$zoneNote';
    default:
      return '${rule['kind']}';
  }
}

/// Editable form of one rule.
class RuleDraft {
  RuleDraft({
    this.kind = 'daily',
    DateTime? at,
    this.interval = 1,
    this.unitSeconds = 3600,
    this.time = '09:00',
    Set<int>? weekdays,
    this.expression = '0 9 * * 1-5',
    this.timeZone = 'UTC',
  }) : at = at ?? DateTime.now().add(const Duration(hours: 1)),
       weekdays = weekdays ?? {1, 2, 3, 4, 5};

  factory RuleDraft.fromRule(Json? rule, String zone) {
    final draft = RuleDraft(timeZone: (rule?['timeZone'] as String?) ?? zone);
    switch (rule?['kind']) {
      case 'at':
        draft.kind = 'at';
        draft.at = DateTime.tryParse('${rule!['at']}')?.toLocal() ?? draft.at;
      case 'every':
        final seconds = (rule!['everySeconds'] as num?)?.toInt() ?? 3600;
        draft.kind = 'every';
        draft.unitSeconds = seconds % 86400 == 0
            ? 86400
            : seconds % 3600 == 0
            ? 3600
            : 60;
        draft.interval = seconds ~/ draft.unitSeconds;
      case 'daily':
        draft.kind = 'daily';
        draft.time = '${rule!['time']}'.substring(0, 5);
      case 'weekly':
        draft.kind = 'weekly';
        draft.time = '${rule!['time']}'.substring(0, 5);
        draft.weekdays = {
          for (final day in (rule['weekdays'] as List? ?? []))
            if (day is num) day.toInt(),
        };
      case 'cron':
        draft.kind = 'cron';
        draft.expression = '${rule!['expression']}';
    }
    return draft;
  }

  String kind;
  DateTime at;
  int interval, unitSeconds;
  String time, expression, timeZone;
  Set<int> weekdays;

  Json toRule() => switch (kind) {
    'at' => {'kind': 'at', 'at': at.toUtc().toIso8601String()},
    'every' => {'kind': 'every', 'everySeconds': interval * unitSeconds},
    'daily' => {'kind': 'daily', 'time': time, 'timeZone': timeZone.trim()},
    'weekly' => {
      'kind': 'weekly',
      'time': time,
      'weekdays': (weekdays.toList()..sort()),
      'timeZone': timeZone.trim(),
    },
    _ => {
      'kind': 'cron',
      'expression': expression.trim(),
      'timeZone': timeZone.trim(),
    },
  };

  RuleDraft copy() => RuleDraft(
    kind: kind,
    at: at,
    interval: interval,
    unitSeconds: unitSeconds,
    time: time,
    weekdays: {...weekdays},
    expression: expression,
    timeZone: timeZone,
  );

  String get signature => toRule().toString();
}

class _Label extends StatelessWidget {
  const _Label(this.text, this.child, {this.hint});
  final String text;
  final String? hint;
  final Widget child;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            text,
            style: DshTypography.caption.copyWith(color: colors.muted),
          ),
          const SizedBox(height: 6),
          child,
          if (hint != null) ...[
            const SizedBox(height: 4),
            Text(
              hint!,
              style: DshTypography.caption.copyWith(color: colors.muted),
            ),
          ],
        ],
      ),
    );
  }
}

/// Frequency editor shared by creation and the detail page.
class RuleEditor extends StatefulWidget {
  const RuleEditor({super.key, required this.draft, required this.onChanged});
  final RuleDraft draft;
  final ValueChanged<RuleDraft> onChanged;
  @override
  State<RuleEditor> createState() => _RuleEditorState();
}

class _RuleEditorState extends State<RuleEditor> {
  late final interval = TextEditingController(text: '${widget.draft.interval}');
  late final expression = TextEditingController(text: widget.draft.expression);
  late final zone = TextEditingController(text: widget.draft.timeZone);

  @override
  void didUpdateWidget(covariant RuleEditor oldWidget) {
    super.didUpdateWidget(oldWidget);
    void sync(TextEditingController controller, String value) {
      if (controller.text != value) controller.text = value;
    }

    sync(interval, '${widget.draft.interval}');
    sync(expression, widget.draft.expression);
    sync(zone, widget.draft.timeZone);
  }

  @override
  void dispose() {
    interval.dispose();
    expression.dispose();
    zone.dispose();
    super.dispose();
  }

  void change(void Function(RuleDraft draft) edit) {
    final next = widget.draft.copy();
    edit(next);
    widget.onChanged(next);
  }

  Future<void> pickTime() async {
    final parts = widget.draft.time.split(':');
    final picked = await showTimePicker(
      context: context,
      initialTime: TimeOfDay(
        hour: int.tryParse(parts.first) ?? 9,
        minute: int.tryParse(parts.length > 1 ? parts[1] : '0') ?? 0,
      ),
      builder: (context, child) => MediaQuery(
        data: MediaQuery.of(context).copyWith(alwaysUse24HourFormat: true),
        child: child!,
      ),
    );
    if (picked != null) {
      change((d) => d.time = '${_two(picked.hour)}:${_two(picked.minute)}');
    }
  }

  Future<void> pickMoment() async {
    final current = widget.draft.at;
    final date = await showDatePicker(
      context: context,
      initialDate: current.isBefore(DateTime.now()) ? DateTime.now() : current,
      firstDate: DateTime.now().subtract(const Duration(days: 1)),
      lastDate: DateTime.now().add(const Duration(days: 3650)),
    );
    if (date == null || !mounted) return;
    final time = await showTimePicker(
      context: context,
      initialTime: TimeOfDay.fromDateTime(current),
      builder: (context, child) => MediaQuery(
        data: MediaQuery.of(context).copyWith(alwaysUse24HourFormat: true),
        child: child!,
      ),
    );
    if (time == null) return;
    change(
      (d) => d.at = DateTime(
        date.year,
        date.month,
        date.day,
        time.hour,
        time.minute,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final draft = widget.draft;
    final colors = DshColors(context);
    final local = draft.at;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final (kind, label) in [
              ('at', '单次'),
              ('every', '每隔'),
              ('daily', '每天'),
              ('weekly', '每周'),
              ('cron', 'Cron'),
            ])
              DshButton(
                key: ValueKey('rule-kind-$kind'),
                height: 30,
                pill: true,
                active: draft.kind == kind,
                onPressed: () => change((d) => d.kind = kind),
                child: Text(label, style: const TextStyle(fontSize: 13)),
              ),
          ],
        ),
        const SizedBox(height: 12),
        if (draft.kind == 'at')
          _Label(
            '执行时间',
            DshButton(
              key: const Key('rule-at'),
              outline: true,
              icon: LucideIcons.calendarClock,
              onPressed: pickMoment,
              child: Text(
                '${local.year}-${_two(local.month)}-${_two(local.day)} ${_two(local.hour)}:${_two(local.minute)}',
              ),
            ),
          ),
        if (draft.kind == 'every')
          _Label(
            '间隔',
            Row(
              children: [
                SizedBox(
                  width: 96,
                  child: TextField(
                    key: const Key('rule-interval'),
                    controller: interval,
                    keyboardType: TextInputType.number,
                    inputFormatters: [FilteringTextInputFormatter.digitsOnly],
                    decoration: const InputDecoration(
                      isDense: true,
                      border: OutlineInputBorder(),
                    ),
                    onChanged: (text) =>
                        change((d) => d.interval = int.tryParse(text) ?? 0),
                  ),
                ),
                const SizedBox(width: 8),
                DshSelect<int>(
                  options: const {60: '分钟', 3600: '小时', 86400: '天'},
                  value: draft.unitSeconds,
                  onChanged: (value) => change((d) => d.unitSeconds = value),
                ),
              ],
            ),
          ),
        if (draft.kind == 'daily' || draft.kind == 'weekly')
          _Label(
            '时间',
            DshButton(
              key: const Key('rule-time'),
              outline: true,
              icon: LucideIcons.clock,
              onPressed: pickTime,
              child: Text(draft.time),
            ),
          ),
        if (draft.kind == 'weekly')
          _Label(
            '星期',
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (var day = 1; day <= 7; day++)
                  DshButton(
                    key: ValueKey('rule-weekday-$day'),
                    height: 30,
                    pill: true,
                    active: draft.weekdays.contains(day),
                    activeBackgroundColor: colors.text,
                    onPressed: () => change((d) {
                      if (!d.weekdays.remove(day)) d.weekdays.add(day);
                    }),
                    child: Text(
                      scheduleWeekdayNames[day - 1],
                      style: TextStyle(
                        fontSize: 13,
                        color: draft.weekdays.contains(day)
                            ? colors.base
                            : null,
                      ),
                    ),
                  ),
              ],
            ),
          ),
        if (draft.kind == 'cron')
          _Label(
            'Cron 表达式',
            DshField(
              controller: expression,
              onChanged: (text) => change((d) => d.expression = text),
            ),
            hint: '5 个字段：分 时 日 月 周，例如 0 9 * * 1-5 表示工作日 9:00',
          ),
        if (['daily', 'weekly', 'cron'].contains(draft.kind))
          _Label(
            '时区',
            DshField(
              controller: zone,
              onChanged: (text) => change((d) => d.timeZone = text),
            ),
          ),
      ],
    );
  }
}

/// The scheduled task manager shown in the main area.
class SchedulePage extends StatefulWidget {
  const SchedulePage({
    super.key,
    required this.controller,
    required this.onClose,
    required this.onOpenSession,
    this.initialTaskId,
    this.api,
  });
  final DesktopController controller;
  final VoidCallback onClose;
  final ValueChanged<String> onOpenSession;
  final String? initialTaskId;
  final ScheduleApi? api;
  @override
  State<SchedulePage> createState() => _SchedulePageState();
}

class _SchedulePageState extends State<SchedulePage> {
  ScheduleWatch? watcher;
  Json? catalog;
  String? error;
  String filter = 'all', query = '';
  String? selected;

  ScheduleApi? get api =>
      widget.api ??
      (widget.controller.client == null
          ? null
          : ScheduleApi(widget.controller.client!));
  List<Json> get tasks => objects(catalog?['tasks']);
  String get hostZone => (catalog?['hostTimeZone'] as String?) ?? 'UTC';

  @override
  void initState() {
    super.initState();
    selected = widget.initialTaskId;
    final api = this.api;
    if (api != null) {
      watcher = ScheduleWatch(api, fetch)..start();
    }
  }

  @override
  void dispose() {
    watcher?.dispose();
    super.dispose();
  }

  /// Read the catalog; returns its revision, or null for a Host without
  /// scheduled tasks.
  Future<int?> fetch(RequestScope scope) async {
    final api = this.api;
    if (api == null) return null;
    try {
      final value = await api.call('catalog', const {}, scope);
      if (!mounted) return null;
      final revision = (value['revision'] as num?)?.toInt();
      setState(() {
        catalog = value;
        error = revision == null
            ? '本机服务不支持定时任务，请更新到最新版本'
            : value['error'] == null
            ? null
            : '定时任务存储不可用：${value['error']}';
      });
      return revision;
    } catch (e) {
      if (mounted) setState(() => error = '$e');
      rethrow;
    }
  }

  Future<void> load() async {
    final watcher = this.watcher;
    if (watcher == null) return;
    try {
      await fetch(watcher.scope);
    } catch (_) {}
  }

  String sessionTitle(String id) {
    for (final session in widget.controller.sessions) {
      if (session.id == id) return session.displayTitle;
    }
    return '会话 ${id.length > 12 ? id.substring(id.length - 8) : id}';
  }

  Future<void> create() async {
    final api = this.api;
    if (api == null) return;
    final task = await showDialog<Json>(
      context: context,
      builder: (_) => ScheduleCreateDialog(
        controller: widget.controller,
        api: api,
        hostTimeZone: hostZone,
      ),
    );
    if (task != null && mounted) {
      setState(() => selected = task['id'] as String?);
      await load();
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final all = tasks;
    final visible =
        all.where((task) {
          final active = task['status'] == 'active';
          final matches = filter == 'all' || (filter == 'active') == active;
          final text = '${task['title']} ${task['prompt']}'.toLowerCase();
          return matches &&
              (query.isEmpty || text.contains(query.toLowerCase()));
        }).toList()..sort((a, b) {
          final status =
              (a['status'] == 'active' ? 0 : 1) -
              (b['status'] == 'active' ? 0 : 1);
          if (status != 0) return status;
          return '${a['nextRunAt'] ?? '~'}'.compareTo(
            '${b['nextRunAt'] ?? '~'}',
          );
        });
    final current = all.where((task) => task['id'] == selected).firstOrNull;
    final counts = {
      'all': all.length,
      'active': all.where((t) => t['status'] == 'active').length,
      'inactive': all.where((t) => t['status'] != 'active').length,
    };
    return Material(
      color: colors.base,
      child: LayoutBuilder(
        builder: (context, box) {
          final wide = box.maxWidth >= 880;
          final list = _TaskList(
            tasks: visible,
            total: all.length,
            loading: catalog == null,
            selected: selected,
            hostZone: hostZone,
            sessionTitle: sessionTitle,
            onSelect: (id) => setState(() => selected = id),
          );
          final detail = current == null
              ? null
              : ScheduleTaskDetail(
                  key: ValueKey(current['id']),
                  task: current,
                  api: api!,
                  hostZone: hostZone,
                  sessionTitle: sessionTitle(current['sessionId'] as String),
                  sessionKnown: widget.controller.sessions.any(
                    (s) => s.id == current['sessionId'],
                  ),
                  onChanged: load,
                  onDeleted: () {
                    setState(() => selected = null);
                    unawaited(load());
                  },
                  onOpenSession: () =>
                      widget.onOpenSession(current['sessionId'] as String),
                  onClose: () => setState(() => selected = null),
                );
          return Padding(
            padding: EdgeInsets.fromLTRB(
              wide ? 28 : 16,
              20,
              wide ? 28 : 16,
              16,
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    DshIcon(
                      LucideIcons.arrowLeft,
                      label: '返回对话',
                      onPressed: widget.onClose,
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          const Text(
                            '定时任务',
                            style: TextStyle(
                              fontSize: 22,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                          const SizedBox(height: 4),
                          Text(
                            '到点后把任务作为新消息发送到原会话执行；关闭会话或重启应用后仍会按时运行。也可以直接在对话中让智能体设置。',
                            style: TextStyle(fontSize: 13, color: colors.muted),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 12),
                    DshButton(
                      key: const Key('schedule-create'),
                      primary: true,
                      icon: LucideIcons.plus,
                      onPressed: api == null || catalog?['error'] != null
                          ? null
                          : create,
                      child: const Text('新建任务'),
                    ),
                  ],
                ),
                const SizedBox(height: 14),
                if (error != null)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 10),
                    child: Text(
                      error!,
                      style: const TextStyle(color: Colors.red),
                    ),
                  ),
                Wrap(
                  spacing: 6,
                  runSpacing: 8,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    for (final (id, label) in [
                      ('all', '全部'),
                      ('active', '已启用'),
                      ('inactive', '已停用'),
                    ])
                      DshButton(
                        key: ValueKey('schedule-filter-$id'),
                        height: 30,
                        pill: true,
                        active: filter == id,
                        onPressed: () => setState(() => filter = id),
                        child: Text(
                          '$label ${counts[id]}',
                          style: const TextStyle(fontSize: 13),
                        ),
                      ),
                    SizedBox(
                      width: 220,
                      child: DshField(
                        hint: '搜索任务',
                        prefix: LucideIcons.search,
                        onChanged: (text) => setState(() => query = text),
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 14),
                Expanded(
                  child: !wide
                      ? detail ?? list
                      : detail == null
                      ? list
                      : Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            SizedBox(width: 360, child: list),
                            const SizedBox(width: 18),
                            Expanded(child: detail),
                          ],
                        ),
                ),
              ],
            ),
          );
        },
      ),
    );
  }
}

class _TaskList extends StatelessWidget {
  const _TaskList({
    required this.tasks,
    required this.total,
    required this.loading,
    required this.selected,
    required this.hostZone,
    required this.sessionTitle,
    required this.onSelect,
  });
  final List<Json> tasks;
  final int total;
  final bool loading;
  final String? selected, hostZone;
  final String Function(String) sessionTitle;
  final ValueChanged<String> onSelect;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    if (loading) {
      return const Center(child: CircularProgressIndicator(strokeWidth: 2));
    }
    if (tasks.isEmpty) {
      return DshEmpty(
        total == 0
            ? '还没有定时任务\n可以在这里新建，也可以在对话中说“每个工作日 9 点汇总昨天的提交”'
            : '没有符合条件的任务',
        icon: LucideIcons.alarmClock,
      );
    }
    return ListView.separated(
      itemCount: tasks.length,
      separatorBuilder: (_, _) => const SizedBox(height: 8),
      itemBuilder: (context, index) {
        final task = tasks[index];
        final active = task['status'] == 'active';
        final failed = object(task['lastDelivery'])['outcome'] == 'failed';
        return Material(
          key: ValueKey('schedule-task-${task['id']}'),
          color: colors.layer,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
            side: BorderSide(
              color: task['id'] == selected ? colors.blue : colors.border,
            ),
          ),
          child: InkWell(
            borderRadius: BorderRadius.circular(12),
            onTap: () => onSelect(task['id'] as String),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Container(
                        width: 8,
                        height: 8,
                        decoration: BoxDecoration(
                          color: failed
                              ? Colors.red
                              : active
                              ? const Color(0xff1e9e5a)
                              : colors.muted,
                          shape: BoxShape.circle,
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          '${task['title']}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 4),
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          scheduleRuleLabel(
                            object(task['rule']),
                            localZone: hostZone,
                          ),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 12, color: colors.muted),
                        ),
                      ),
                      Text(
                        active
                            ? (task['nextRunAt'] == null
                                  ? '等待投递'
                                  : relativeScheduleTime(
                                      task['nextRunAt'] as String?,
                                    ))
                            : '已停用',
                        style: TextStyle(fontSize: 12, color: colors.muted),
                      ),
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    sessionTitle('${task['sessionId']}'),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12, color: colors.muted),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

/// One task's rule editor and delivery records.
class ScheduleTaskDetail extends StatefulWidget {
  const ScheduleTaskDetail({
    super.key,
    required this.task,
    required this.api,
    required this.hostZone,
    required this.sessionTitle,
    required this.sessionKnown,
    required this.onChanged,
    required this.onDeleted,
    required this.onOpenSession,
    required this.onClose,
  });
  final Json task;
  final ScheduleApi api;
  final String hostZone, sessionTitle;
  final bool sessionKnown;
  final Future<void> Function() onChanged;
  final VoidCallback onDeleted, onOpenSession, onClose;
  @override
  State<ScheduleTaskDetail> createState() => _ScheduleTaskDetailState();
}

class _ScheduleTaskDetailState extends State<ScheduleTaskDetail> {
  late final title = TextEditingController(text: '${widget.task['title']}');
  late final prompt = TextEditingController(text: '${widget.task['prompt']}');
  late RuleDraft draft = RuleDraft.fromRule(
    object(widget.task['rule']),
    widget.hostZone,
  );
  late String baseline = _signature();
  String tab = 'rules';
  String? busy, error, notice;
  Json? history;

  String _signature() =>
      '${title.text}\u0000${prompt.text}\u0000${draft.signature}';
  bool get dirty => _signature() != baseline;

  @override
  void didUpdateWidget(covariant ScheduleTaskDetail oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.task['updatedAt'] != widget.task['updatedAt'] && !dirty) {
      title.text = '${widget.task['title']}';
      prompt.text = '${widget.task['prompt']}';
      draft = RuleDraft.fromRule(object(widget.task['rule']), widget.hostZone);
      baseline = _signature();
    }
    if (oldWidget.task['historyCount'] != widget.task['historyCount'] &&
        tab == 'records') {
      unawaited(loadHistory());
    }
  }

  @override
  void dispose() {
    title.dispose();
    prompt.dispose();
    super.dispose();
  }

  Json get binding => {
    'id': widget.task['id'],
    'sessionId': widget.task['sessionId'],
  };

  Future<void> run(String name, Future<void> Function() action) async {
    setState(() {
      busy = name;
      error = null;
      notice = null;
    });
    try {
      await action();
    } on DshException catch (e) {
      if (!mounted) return;
      if (e.code == 'http-409') {
        notice = '任务已被其他操作修改，已刷新为最新内容';
        baseline = '';
        await widget.onChanged();
      } else {
        error = e.message;
      }
    } catch (e) {
      if (mounted) error = '$e';
    } finally {
      if (mounted) setState(() => busy = null);
    }
  }

  Future<void> save() => run('save', () async {
    final original = RuleDraft.fromRule(
      object(widget.task['rule']),
      widget.hostZone,
    );
    await widget.api.call('update', {
      ...binding,
      'expectedUpdatedAt': widget.task['updatedAt'],
      'title': title.text,
      'prompt': prompt.text,
      if (draft.signature != original.signature) 'rule': draft.toRule(),
    });
    baseline = _signature();
    await widget.onChanged();
  });

  Future<void> loadHistory() async {
    try {
      final value = await widget.api.call('history', {...binding, 'limit': 50});
      if (mounted) setState(() => history = value);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final task = widget.task;
    final active = task['status'] == 'active';
    final last = object(task['lastDelivery']);
    return Container(
      key: const Key('schedule-detail'),
      decoration: BoxDecoration(
        color: colors.layer,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: colors.border),
      ),
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              DshIcon(LucideIcons.x, label: '关闭详情', onPressed: widget.onClose),
              const SizedBox(width: 4),
              Expanded(
                child: Text(
                  '${task['title']}',
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: 17,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              const Text('启用', style: TextStyle(fontSize: 13)),
              const SizedBox(width: 6),
              DshSwitch(
                key: const Key('schedule-active'),
                value: active,
                onChanged: busy != null
                    ? null
                    : (value) => run('toggle', () async {
                        await widget.api.call('setActive', {
                          ...binding,
                          'active': value,
                        });
                        await widget.onChanged();
                      }),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 16,
            runSpacing: 4,
            children: [
              for (final text in [
                scheduleRuleLabel(
                  object(task['rule']),
                  localZone: widget.hostZone,
                ),
                '下次运行：${active && task['nextRunAt'] != null ? '${formatScheduleTime(task['nextRunAt'] as String?)}（${relativeScheduleTime(task['nextRunAt'] as String?)}）' : '无'}',
                '最近运行：${last.isEmpty ? '尚未运行' : '${formatScheduleTime(last['deliveredAt'] as String?)} · ${last['outcome'] == 'delivered' ? '已发送' : '发送失败'}'}',
                task['origin'] == 'agent' ? '由智能体创建' : '由你创建',
              ])
                Text(text, style: TextStyle(fontSize: 12, color: colors.muted)),
            ],
          ),
          const SizedBox(height: 6),
          Row(
            children: [
              Text(
                '目标会话：',
                style: TextStyle(fontSize: 12, color: colors.muted),
              ),
              Flexible(
                child: Text(
                  widget.sessionTitle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: 12),
                ),
              ),
              if (widget.sessionKnown)
                TextButton(
                  onPressed: widget.onOpenSession,
                  child: const Text('打开会话'),
                ),
            ],
          ),
          const SizedBox(height: 4),
          Row(
            children: [
              for (final (id, label) in [
                ('rules', '规则'),
                ('records', '运行记录 ${task['historyCount'] ?? 0}'),
              ])
                Padding(
                  padding: const EdgeInsets.only(right: 6),
                  child: DshButton(
                    key: ValueKey('schedule-tab-$id'),
                    height: 30,
                    pill: true,
                    active: tab == id,
                    onPressed: () {
                      setState(() => tab = id);
                      if (id == 'records') unawaited(loadHistory());
                    },
                    child: Text(label, style: const TextStyle(fontSize: 13)),
                  ),
                ),
            ],
          ),
          const SizedBox(height: 12),
          if (notice != null)
            Text(notice!, style: TextStyle(fontSize: 12, color: colors.muted)),
          if (error != null)
            Text(
              error!,
              style: const TextStyle(fontSize: 12, color: Colors.red),
            ),
          Expanded(
            child: tab == 'rules'
                ? ListView(
                    children: [
                      _Label(
                        '名称',
                        DshField(
                          controller: title,
                          onChanged: (_) => setState(() {}),
                        ),
                      ),
                      _Label(
                        '任务内容',
                        DshField(
                          key: const Key('schedule-prompt'),
                          controller: prompt,
                          maxLines: 5,
                          onChanged: (_) => setState(() {}),
                        ),
                      ),
                      _Label(
                        '频率',
                        RuleEditor(
                          draft: draft,
                          onChanged: (next) => setState(() => draft = next),
                        ),
                      ),
                    ],
                  )
                : _Records(history: history),
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              DshButton(
                key: const Key('schedule-delete'),
                destructive: true,
                onPressed: busy != null
                    ? null
                    : () async {
                        final confirmed = await confirmAction(
                          context,
                          '删除定时任务',
                          '删除后不再运行，已发送的消息保留在会话中。',
                          action: '删除',
                        );
                        if (!confirmed) return;
                        await run('delete', () async {
                          await widget.api.call('delete', binding);
                          widget.onDeleted();
                        });
                      },
                child: const Text('删除'),
              ),
              DshButton(
                key: const Key('schedule-run-now'),
                onPressed: busy != null
                    ? null
                    : () => run('run', () async {
                        await widget.api.call('runNow', binding);
                        setState(() => tab = 'records');
                        await widget.onChanged();
                        await loadHistory();
                      }),
                child: Text(busy == 'run' ? '运行中…' : '立即运行'),
              ),
              if (dirty)
                Text(
                  '有未保存的修改',
                  style: TextStyle(fontSize: 12, color: colors.muted),
                ),
              DshButton(
                onPressed: !dirty || busy != null
                    ? null
                    : () => setState(() {
                        title.text = '${task['title']}';
                        prompt.text = '${task['prompt']}';
                        draft = RuleDraft.fromRule(
                          object(task['rule']),
                          widget.hostZone,
                        );
                        baseline = _signature();
                      }),
                child: const Text('取消'),
              ),
              DshButton(
                key: const Key('schedule-save'),
                primary: true,
                onPressed: !dirty || busy != null || prompt.text.trim().isEmpty
                    ? null
                    : save,
                child: Text(busy == 'save' ? '正在保存…' : '保存'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _Records extends StatelessWidget {
  const _Records({required this.history});
  final Json? history;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    if (history == null) {
      return const Center(child: CircularProgressIndicator(strokeWidth: 2));
    }
    final records = objects(history!['records']);
    if (records.isEmpty) {
      return const DshEmpty('还没有运行记录', icon: LucideIcons.history);
    }
    return ListView(
      children: [
        Text(
          '“已发送”表示消息已进入会话，不代表任务已完成。',
          style: TextStyle(fontSize: 12, color: colors.muted),
        ),
        const SizedBox(height: 8),
        for (final record in records)
          Container(
            margin: const EdgeInsets.only(bottom: 8),
            padding: const EdgeInsets.all(10),
            decoration: BoxDecoration(
              color: colors.base,
              borderRadius: BorderRadius.circular(10),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Text(
                      record['outcome'] == 'delivered' ? '已发送' : '发送失败',
                      style: TextStyle(
                        fontWeight: FontWeight.w600,
                        color: record['outcome'] == 'delivered'
                            ? null
                            : Colors.red,
                      ),
                    ),
                    if (record['manual'] == true) ...[
                      const SizedBox(width: 6),
                      Text(
                        '手动',
                        style: TextStyle(fontSize: 12, color: colors.muted),
                      ),
                    ],
                    const Spacer(),
                    Text(
                      formatScheduleTime(record['deliveredAt'] as String?),
                      style: TextStyle(fontSize: 12, color: colors.muted),
                    ),
                  ],
                ),
                if (record['error'] != null)
                  Text(
                    '${record['error']}',
                    style: const TextStyle(fontSize: 12, color: Colors.red),
                  ),
                const SizedBox(height: 4),
                SelectableText(
                  '${record['prompt']}',
                  maxLines: 3,
                  style: TextStyle(fontSize: 13, color: colors.muted),
                ),
              ],
            ),
          ),
        if (history!['earlierRecordsPruned'] == true &&
            history!['hasMore'] != true)
          Text(
            '更早的记录已按保留策略清理（${history!['retentionDays']} 天 / ${history!['retentionRecords']} 条）',
            style: TextStyle(fontSize: 12, color: colors.muted),
          ),
      ],
    );
  }
}

/// Creates a task in an existing conversation or a new one of a workspace.
class ScheduleCreateDialog extends StatefulWidget {
  const ScheduleCreateDialog({
    super.key,
    required this.controller,
    required this.api,
    required this.hostTimeZone,
  });
  final DesktopController controller;
  final ScheduleApi api;
  final String hostTimeZone;
  @override
  State<ScheduleCreateDialog> createState() => _ScheduleCreateDialogState();
}

class _ScheduleCreateDialogState extends State<ScheduleCreateDialog> {
  final prompt = TextEditingController(), title = TextEditingController();
  late RuleDraft draft = RuleDraft(timeZone: widget.hostTimeZone);
  late String target = _defaultTarget();
  bool busy = false;
  String? error;

  DesktopController get c => widget.controller;

  Map<String, String> get targets => {
    for (final session in c.sessions)
      if (!session.blank || session.id == c.selectedId)
        session.id: session.displayTitle,
    for (final workspace in c.workspaces)
      'new:${displayPathText('${workspace['path']}')}':
          '＋ 新会话 · ${workspace['title'] ?? workspace['path']}',
  };

  String _defaultTarget() {
    final options = targets;
    if (c.selectedId != null && options.containsKey(c.selectedId)) {
      return c.selectedId!;
    }
    return options.keys.firstOrNull ?? '';
  }

  @override
  void dispose() {
    prompt.dispose();
    title.dispose();
    super.dispose();
  }

  Future<void> submit() async {
    setState(() {
      busy = true;
      error = null;
    });
    try {
      var sessionId = target;
      if (target.startsWith('new:')) {
        final client = c.client;
        if (client == null) throw StateError('请先连接本机服务');
        final created = await client.call('session.create', {
          'cwd': target.substring(4),
          'agentPreset': c.preset,
        }, true);
        sessionId = created['sessionId'] as String;
        final name = (title.text.trim().isEmpty
            ? prompt.text.trim().split('\n').first
            : title.text.trim());
        try {
          await client.call('session.rename', {
            'sessionId': sessionId,
            'title':
                '定时任务 · ${name.length > 40 ? name.substring(0, 40) : name}',
          }, true);
        } catch (_) {}
        unawaited(c.refreshSessions());
      }
      final value = await widget.api.call('create', {
        'sessionId': sessionId,
        'title': title.text,
        'prompt': prompt.text,
        'rule': draft.toRule(),
      });
      if (mounted) Navigator.of(context).pop(object(value['task']));
    } on DshException catch (e) {
      if (mounted) setState(() => error = e.message);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final options = targets;
    return AlertDialog(
      title: const Text('新建定时任务', style: TextStyle(fontSize: 17)),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _Label(
                '任务内容',
                DshField(
                  key: const Key('schedule-create-prompt'),
                  controller: prompt,
                  maxLines: 4,
                  autofocus: true,
                  hint: '到点后要执行的指令，例如：汇总今天的新闻并列出三条要点',
                  onChanged: (_) => setState(() {}),
                ),
              ),
              _Label('名称', DshField(controller: title, hint: '留空则使用任务内容的开头')),
              _Label(
                '频率',
                RuleEditor(
                  draft: draft,
                  onChanged: (next) => setState(() => draft = next),
                ),
              ),
              _Label(
                '目标会话',
                options.isEmpty
                    ? const Text('请先添加工作区或新建一个会话')
                    : DshSelect<String>(
                        key: const Key('schedule-create-target'),
                        options: options,
                        value: options.containsKey(target) ? target : null,
                        maxWidth: 480,
                        onChanged: (value) => setState(() => target = value),
                      ),
                hint: '任务会发送到这个会话，由该会话的智能体执行',
              ),
              if (error != null)
                Text(error!, style: const TextStyle(color: Colors.red)),
            ],
          ),
        ),
      ),
      actions: [
        DshButton(
          onPressed: busy ? null : () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        DshButton(
          key: const Key('schedule-create-submit'),
          primary: true,
          onPressed:
              busy || prompt.text.trim().isEmpty || !options.containsKey(target)
              ? null
              : submit,
          child: Text(busy ? '正在创建…' : '创建'),
        ),
      ],
    );
  }
}

/// Clock in the conversation header while the conversation has active
/// scheduled tasks; opens the schedule page on the chosen task.
class ScheduleSessionBadge extends StatefulWidget {
  const ScheduleSessionBadge({
    super.key,
    required this.controller,
    required this.sessionId,
    required this.onOpen,
    this.api,
  });
  final DesktopController controller;
  final String sessionId;
  final ValueChanged<String?> onOpen;
  final ScheduleApi? api;
  @override
  State<ScheduleSessionBadge> createState() => _ScheduleSessionBadgeState();
}

class _ScheduleSessionBadgeState extends State<ScheduleSessionBadge> {
  ScheduleWatch? watcher;
  List<Json> active = [];

  ScheduleApi? get api =>
      widget.api ??
      (widget.controller.client == null
          ? null
          : ScheduleApi(widget.controller.client!));

  @override
  void initState() {
    super.initState();
    final api = this.api;
    if (api != null) {
      watcher = ScheduleWatch(api, (scope) async {
        final value = await api.call('catalog', {
          'sessionId': widget.sessionId,
        }, scope);
        final revision = (value['revision'] as num?)?.toInt();
        if (!mounted || revision == null) return null;
        final next = objects(value['tasks'])
            .where((task) => task['status'] == 'active')
            .toList();
        String signature(List<Json> tasks) => tasks
            .map(
              (task) => '${task['id']}|${task['title']}|${task['nextRunAt']}',
            )
            .join(',');
        if (signature(next) != signature(active)) {
          setState(() => active = next);
        }
        return revision;
      })..start();
    }
  }

  @override
  void dispose() {
    watcher?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (active.isEmpty) return const SizedBox.shrink();
    final colors = DshColors(context);
    final label = '定时任务 ${active.length}';
    final badge = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        DshGlyph(LucideIcons.alarmClock, size: 15, color: colors.muted),
        const SizedBox(width: 4),
        Text(
          '${active.length}',
          style: TextStyle(fontSize: 12, color: colors.muted),
        ),
      ],
    );
    return Padding(
      padding: const EdgeInsets.only(right: 8),
      child: Tooltip(
        message: active
            .map(
              (task) =>
                  '${task['title']} · ${relativeScheduleTime(task['nextRunAt'] as String?)}',
            )
            .join('\n'),
        child: Semantics(
          label: label,
          button: true,
          child: active.length == 1
              ? InkWell(
                  key: const Key('schedule-badge'),
                  borderRadius: BorderRadius.circular(8),
                  onTap: () => widget.onOpen(active.first['id'] as String?),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 6,
                      vertical: 6,
                    ),
                    child: badge,
                  ),
                )
              : PopupMenuButton<String>(
                  key: const Key('schedule-badge'),
                  tooltip: '',
                  onSelected: widget.onOpen,
                  itemBuilder: (_) => [
                    for (final task in active)
                      PopupMenuItem(
                        value: task['id'] as String,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text('${task['title']}'),
                            Text(
                              '${scheduleRuleLabel(object(task['rule']))} · ${relativeScheduleTime(task['nextRunAt'] as String?)}',
                              style: TextStyle(
                                fontSize: 12,
                                color: colors.muted,
                              ),
                            ),
                          ],
                        ),
                      ),
                  ],
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 6,
                      vertical: 6,
                    ),
                    child: badge,
                  ),
                ),
        ),
      ),
    );
  }
}

/// Long-polls the Host's scheduled-task revision and refetches after each
/// change. [fetch] returns the fetched revision, or null for a Host that does
/// not provide scheduled tasks, which ends the watch. Errors back off with a
/// cancellable pause, so a failing or unsupported Host never spins.
class ScheduleWatch {
  ScheduleWatch(this.api, this.fetch);
  final ScheduleApi api;
  final Future<int?> Function(RequestScope scope) fetch;
  final scope = RequestScope();
  Timer? _timer;
  Completer<void>? _pause;
  bool _closed = false;

  void start() => unawaited(_run());

  Future<void> _run() async {
    int? revision;
    while (!_closed) {
      try {
        if (revision == null) {
          revision = await fetch(scope);
          if (revision == null) return;
          continue;
        }
        final next = (await api.call('wait', {
          'revision': revision,
        }, scope))['revision'];
        if (_closed || next is! num) return;
        if (next.toInt() != revision) revision = null;
      } catch (_) {
        if (_closed) return;
        await _sleep(const Duration(seconds: 5));
      }
    }
  }

  Future<void> _sleep(Duration duration) {
    final pause = _pause = Completer<void>();
    _timer = Timer(duration, () {
      if (!pause.isCompleted) pause.complete();
    });
    return pause.future;
  }

  void dispose() {
    _closed = true;
    scope.cancel();
    _timer?.cancel();
    final pause = _pause;
    if (pause != null && !pause.isCompleted) pause.complete();
  }
}
