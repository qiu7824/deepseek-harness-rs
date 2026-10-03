import '../../design/error.dart';
import '../../l10n/zh.dart';
import '../../l10n/conversation_zh.dart';

import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../design/loading.dart';
import '../../design/select.dart';
import '../../design/typography.dart';
import '../../src/controller.dart';
import '../page_operation.dart';

/// Host-owned scheduled tasks over `/__dsh-schedule/*`.
class ScheduleApi {
  ScheduleApi(this.client);
  final DshClient client;
  static const _reads = {'catalog', 'history', 'wait'};

  Future<Json> call(
    String operation, [
    Json body = const {},
    RequestScope? scope,
  ]) async {
    try {
      return await client.request(
        '/__dsh-schedule/$operation',
        body: body,
        scope: scope,
        mutation: !_reads.contains(operation),
        maxBytes: 4 * 1024 * 1024,
      );
    } on DshException catch (error) {
      throw unsupportedHostPage(error, operation, DshScheduleZh.title);
    }
  }
}

const scheduleWeekdayNames = [
  DshScheduleZh.monday,
  DshScheduleZh.tuesday,
  DshScheduleZh.wednesday,
  DshScheduleZh.thursday,
  DshScheduleZh.friday,
  DshScheduleZh.saturday,
  DshScheduleZh.sunday,
];

String _two(int value) => value.toString().padLeft(2, '0');

String formatScheduleTime(String? iso) {
  final value = iso == null ? null : DateTime.tryParse(iso)?.toLocal();
  if (value == null) return '';
  final now = DateTime.now();
  final clock = '${_two(value.hour)}:${_two(value.minute)}';
  final date = value.year == now.year
      ? DshScheduleZh.monthDay(month: value.month, day: value.day)
      : DshScheduleZh.yearMonthDay(
          year: value.year,
          month: value.month,
          day: value.day,
        );
  return '$date $clock';
}

String relativeScheduleTime(String? iso, {DateTime? now}) {
  final value = iso == null ? null : DateTime.tryParse(iso);
  if (value == null) return '';
  final delta = value.difference(now ?? DateTime.now());
  if (delta <= Duration.zero) return DshScheduleZh.upcoming;
  if (delta < const Duration(hours: 1)) {
    return DshScheduleZh.minutesFromNow(count: delta.inMinutes.clamp(1, 59));
  }
  if (delta < const Duration(days: 1)) {
    return DshScheduleZh.hoursFromNow(count: (delta.inMinutes / 60).round());
  }
  return DshScheduleZh.daysFromNow(count: (delta.inHours / 24).round());
}

/// Chinese label for one stored rule, e.g. `每周一、周五 18:30`.
String scheduleRuleLabel(Json rule, {String? localZone}) {
  final zone = rule['timeZone'] as String?;
  final zoneNote = zone != null && localZone != null && zone != localZone
      ? '（$zone）'
      : '';
  switch (rule['kind']) {
    case 'at':
      return DshScheduleZh.onceAt(
        time: formatScheduleTime(rule['at'] as String?),
      );
    case 'every':
      final seconds = (rule['everySeconds'] as num?)?.toInt() ?? 0;
      if (seconds % 86400 == 0) {
        return DshScheduleZh.everyDays(count: seconds ~/ 86400);
      }
      if (seconds % 3600 == 0) {
        return DshScheduleZh.everyHours(count: seconds ~/ 3600);
      }
      return DshScheduleZh.everyMinutes(count: (seconds / 60).round());
    case 'daily':
      return DshScheduleZh.dailyAt(time: rule['time'], zone: zoneNote);
    case 'weekly':
      final days = [
        for (final day in (rule['weekdays'] as List? ?? []))
          if (day is num && day >= 1 && day <= 7)
            scheduleWeekdayNames[day.toInt() - 1],
      ];
      return days.length == 7
          ? DshScheduleZh.dailyAt(time: rule['time'], zone: zoneNote)
          : DshScheduleZh.weeklyAt(
              days: days.join('、'),
              time: rule['time'],
              zone: zoneNote,
            );
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
    if (picked != null && mounted) {
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
    if (time == null || !mounted) return;
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
              ('at', DshScheduleZh.once),
              ('every', DshScheduleZh.interval),
              ('daily', DshScheduleZh.daily),
              ('weekly', DshScheduleZh.weekly),
              ('cron', 'Cron'),
            ])
              DshButton(
                key: ValueKey('rule-kind-$kind'),
                height: 30,
                pill: true,
                active: draft.kind == kind,
                onPressed: () => change((d) => d.kind = kind),
                child: Text(
                  label,
                  style: const TextStyle(fontSize: DshTypography.sizeAuxiliary),
                ),
              ),
          ],
        ),
        const SizedBox(height: 12),
        if (draft.kind == 'at')
          _Label(
            DshScheduleZh.runAt,
            DshButton(
              key: const Key('rule-at'),
              outline: true,
              icon: DshIcons.calendarClock.data,
              onPressed: pickMoment,
              child: Text(
                '${local.year}-${_two(local.month)}-${_two(local.day)} ${_two(local.hour)}:${_two(local.minute)}',
              ),
            ),
          ),
        if (draft.kind == 'every')
          _Label(
            DshScheduleZh.duration,
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
                  options: const {
                    60: DshScheduleZh.minutes,
                    3600: DshScheduleZh.hours,
                    86400: DshScheduleZh.days,
                  },
                  value: draft.unitSeconds,
                  onChanged: (value) => change((d) => d.unitSeconds = value),
                ),
              ],
            ),
          ),
        if (draft.kind == 'daily' || draft.kind == 'weekly')
          _Label(
            DshScheduleZh.time,
            DshButton(
              key: const Key('rule-time'),
              outline: true,
              icon: DshIcons.clock.data,
              onPressed: pickTime,
              child: Text(draft.time),
            ),
          ),
        if (draft.kind == 'weekly')
          _Label(
            DshScheduleZh.weekday,
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
                        fontSize: DshTypography.sizeAuxiliary,
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
            DshScheduleZh.cron,
            DshField(
              controller: expression,
              onChanged: (text) => change((d) => d.expression = text),
            ),
            hint: DshScheduleZh.cronHint,
          ),
        if (['daily', 'weekly', 'cron'].contains(draft.kind))
          _Label(
            DshScheduleZh.timezone,
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
  ScheduleApi? _api, _injected;
  DshClient? _client;
  RequestScope _scope = RequestScope();
  int _generation = 0, _fetchRevision = 0;
  DialogRoute<Json>? _createRoute;
  Json? catalog;
  String? error;
  String filter = 'all', query = '';
  String? selected;

  ScheduleApi? get api => _api;
  List<Json> get tasks => objects(catalog?['tasks']);
  String get hostZone => (catalog?['hostTimeZone'] as String?) ?? 'UTC';

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_hostChanged);
    _bind(force: true, notify: false);
  }

  @override
  void didUpdateWidget(SchedulePage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_hostChanged);
      widget.controller.addListener(_hostChanged);
    }
    _bind(
      force:
          oldWidget.controller != widget.controller ||
          oldWidget.api != widget.api,
      notify: false,
    );
    if (oldWidget.initialTaskId != widget.initialTaskId) {
      selected = widget.initialTaskId;
    }
  }

  void _hostChanged() => _bind();

  void _closeCreate() {
    final route = _createRoute;
    _createRoute = null;
    if (route == null) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final navigator = route.navigator;
      if (navigator != null && navigator.mounted && route.isActive) {
        navigator.removeRoute(route);
      }
    });
  }

  void _bind({bool force = false, bool notify = true}) {
    final client = widget.api?.client ?? widget.controller.client;
    if (!force &&
        identical(client, _client) &&
        identical(widget.api, _injected)) {
      return;
    }
    watcher?.dispose();
    watcher = null;
    _scope.cancel();
    _scope = RequestScope();
    _generation++;
    _closeCreate();
    _client = client;
    _injected = widget.api;
    _api = widget.api ?? (client == null ? null : ScheduleApi(client));
    catalog = null;
    selected = widget.initialTaskId;
    error = client == null ? DshScheduleZh.connectFirst : null;
    if (notify && mounted) setState(() {});
    final owner = _api, generation = _generation;
    if (owner != null) {
      watcher = ScheduleWatch(owner, (scope) => fetch(owner, generation, scope))
        ..start();
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_hostChanged);
    _generation++;
    watcher?.dispose();
    _scope.cancel();
    _closeCreate();
    super.dispose();
  }

  Future<int?> fetch(
    ScheduleApi owner,
    int generation,
    RequestScope scope,
  ) async {
    final request = ++_fetchRevision;
    bool current() =>
        mounted &&
        generation == _generation &&
        identical(owner, _api) &&
        !scope.cancelled;
    try {
      final value = await owner.call('catalog', const {}, scope);
      if (!current()) return null;
      final revision = (value['revision'] as num?)?.toInt();
      if (request != _fetchRevision) {
        return (catalog?['revision'] as num?)?.toInt() ?? revision;
      }
      setState(() {
        catalog = value;
        error = revision == null
            ? DshScheduleZh.unsupportedHost
            : value['error'] != null
            ? DshScheduleZh.storageUnavailable(detail: value['error'])
            : value['deliveryError'] != null
            ? DshScheduleZh.deliveryPersistenceFailed(
                detail: value['deliveryError'],
              )
            : null;
      });
      return revision;
    } catch (e) {
      if (!current()) return null;
      if (request == _fetchRevision) {
        setState(
          () => error = e is DshException && e.code == 'http-404'
              ? DshScheduleZh.unsupportedHost
              : '$e',
        );
      }
      if (e is DshException && e.code == 'http-404') return null;
      rethrow;
    }
  }

  Future<void> load() async {
    final owner = _api;
    if (owner == null) return;
    try {
      await fetch(owner, _generation, _scope);
    } catch (_) {}
  }

  String sessionTitle(String id) {
    for (final session in widget.controller.sessions) {
      if (session.id == id) return session.displayTitle;
    }
    return DshScheduleZh.sessionLabel(
      id: id.length > 12 ? id.substring(id.length - 8) : id,
    );
  }

  Future<void> create() async {
    final owner = _api, generation = _generation;
    if (owner == null) return;
    bool current() =>
        mounted &&
        generation == _generation &&
        identical(owner, _api) &&
        !_scope.cancelled;
    final route = DialogRoute<Json>(
      context: context,
      builder: (_) => ScheduleCreateDialog(
        controller: widget.controller,
        api: owner,
        hostTimeZone: hostZone,
        ownerScope: _scope,
        isCurrent: current,
      ),
    );
    _createRoute = route;
    try {
      final task = await Navigator.of(context).push(route);
      if (task != null && current()) {
        setState(() => selected = task['id'] as String?);
        await load();
      }
    } finally {
      if (identical(_createRoute, route)) _createRoute = null;
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
            loading: catalog == null && error == null,
            selected: selected,
            hostZone: hostZone,
            sessionTitle: sessionTitle,
            onSelect: (id) => setState(() => selected = id),
          );
          final detail = current == null
              ? null
              : ScheduleTaskDetail(
                  key: ValueKey((api, current['id'])),
                  ownerScope: _scope,
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
                      DshIcons.arrowLeft.data,
                      label: DshScheduleZh.backToSession,
                      onPressed: widget.onClose,
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          const Text(
                            DshScheduleZh.title,
                            style: TextStyle(
                              fontSize: DshTypography.sizeTitle,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                          const SizedBox(height: 4),
                          Text(
                            DshScheduleZh.description,
                            style: TextStyle(
                              fontSize: DshTypography.sizeAuxiliary,
                              color: colors.muted,
                            ),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 12),
                    DshButton(
                      key: const Key('schedule-create'),
                      primary: true,
                      icon: DshIcons.plus.data,
                      onPressed: api == null || catalog?['error'] != null
                          ? null
                          : create,
                      child: const Text(DshScheduleZh.newTask),
                    ),
                  ],
                ),
                const SizedBox(height: 14),
                if (error != null)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 10),
                    child: DshErrorView(error: error!),
                  ),
                Wrap(
                  spacing: 6,
                  runSpacing: 8,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    for (final (id, label) in [
                      ('all', DshScheduleZh.all),
                      ('active', DshScheduleZh.enabled),
                      ('inactive', DshScheduleZh.disabled),
                    ])
                      DshButton(
                        key: ValueKey('schedule-filter-$id'),
                        height: 30,
                        pill: true,
                        active: filter == id,
                        onPressed: () => setState(() => filter = id),
                        child: Text(
                          '$label ${counts[id]}',
                          style: const TextStyle(
                            fontSize: DshTypography.sizeAuxiliary,
                          ),
                        ),
                      ),
                    SizedBox(
                      width: 220,
                      child: DshField(
                        hint: DshScheduleZh.search,
                        prefix: DshIcons.search.data,
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
    if (loading && tasks.isEmpty) {
      return DshListSkeleton(
        label: DshConversationZh.loadingList(name: DshScheduleZh.title),
      );
    }
    if (tasks.isEmpty) {
      return DshEmpty(
        total == 0 ? DshScheduleZh.empty : DshScheduleZh.noMatches,
        icon: DshIcons.alarmClock.data,
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
                              ? DshTokens.of(context).error.foreground
                              : active
                              ? DshTokens.of(context).success.foreground
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
                            fontSize: DshTypography.sizeBody,
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
                          style: TextStyle(
                            fontSize: DshTypography.sizeCaption,
                            color: colors.muted,
                          ),
                        ),
                      ),
                      Text(
                        active
                            ? (task['nextRunAt'] == null
                                  ? DshScheduleZh.awaitingDelivery
                                  : relativeScheduleTime(
                                      task['nextRunAt'] as String?,
                                    ))
                            : DshScheduleZh.disabled,
                        style: TextStyle(
                          fontSize: DshTypography.sizeCaption,
                          color: colors.muted,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    sessionTitle('${task['sessionId']}'),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: DshTypography.sizeCaption,
                      color: colors.muted,
                    ),
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
    this.ownerScope,
  });
  final RequestScope? ownerScope;
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
  RequestScope _scope = RequestScope();
  void Function()? _detach;
  int _generation = 0, _historyRevision = 0;

  @override
  void initState() {
    super.initState();
    _detach = widget.ownerScope?.register(_scope.cancel);
  }

  PageOperation operation() {
    final generation = _generation,
        api = widget.api,
        id = widget.task['id'],
        session = widget.task['sessionId'];
    return PageOperation(
      _scope,
      () =>
          mounted &&
          generation == _generation &&
          identical(api, widget.api) &&
          id == widget.task['id'] &&
          session == widget.task['sessionId'],
    );
  }

  String _signature() =>
      '${title.text}\u0000${prompt.text}\u0000${draft.signature}';
  bool get dirty => _signature() != baseline;

  @override
  void didUpdateWidget(covariant ScheduleTaskDetail oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.api != widget.api ||
        oldWidget.task['id'] != widget.task['id'] ||
        oldWidget.ownerScope != widget.ownerScope) {
      _detach?.call();
      _scope.cancel();
      _scope = RequestScope();
      _detach = widget.ownerScope?.register(_scope.cancel);
      _generation++;
      busy = null;
      error = null;
      notice = null;
      history = null;
      title.text = '${widget.task['title']}';
      prompt.text = '${widget.task['prompt']}';
      draft = RuleDraft.fromRule(object(widget.task['rule']), widget.hostZone);
      baseline = _signature();
    }
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
    _generation++;
    _detach?.call();
    _scope.cancel();
    title.dispose();
    prompt.dispose();
    super.dispose();
  }

  Json get binding => {
    'id': widget.task['id'],
    'sessionId': widget.task['sessionId'],
  };

  Future<void> run(
    String name,
    Future<void> Function(PageOperation op) action,
  ) async {
    if (busy != null) return;
    final op = operation();
    if (!op.valid) return;
    setState(() {
      busy = name;
      error = null;
      notice = null;
    });
    try {
      await action(op);
    } on DshException catch (e) {
      if (!op.valid) return;
      if (e.code == 'http-409') {
        notice = DshScheduleZh.conflict;
        baseline = '';
        try {
          await op.request(widget.onChanged);
        } catch (failure) {
          if (op.valid) error = '$failure';
        }
      } else {
        error = '$e';
      }
    } catch (e) {
      if (op.valid) error = '$e';
    } finally {
      if (op.valid) setState(() => busy = null);
    }
  }

  Future<void> save() => run('save', (op) async {
    final original = RuleDraft.fromRule(
      object(widget.task['rule']),
      widget.hostZone,
    );
    final savedSignature = _signature();
    final body = {
      ...binding,
      'expectedUpdatedAt': widget.task['updatedAt'],
      'title': title.text,
      'prompt': prompt.text,
      if (draft.signature != original.signature) 'rule': draft.toRule(),
    };
    await op.request(() => widget.api.call('update', body, op.scope));
    baseline = savedSignature;
    await op.request(widget.onChanged);
  });

  Future<void> loadHistory() async {
    final request = ++_historyRevision, op = operation();
    if (!op.valid) return;
    try {
      final value = await op.request(
        () => widget.api.call('history', {...binding, 'limit': 50}, op.scope),
      );
      if (op.valid && request == _historyRevision) {
        setState(() => history = value);
      }
    } catch (e) {
      if (op.valid && request == _historyRevision) setState(() => error = '$e');
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
              DshIcon(
                DshIcons.close.data,
                label: DshScheduleZh.closeDetails,
                onPressed: widget.onClose,
              ),
              const SizedBox(width: 4),
              Expanded(
                child: Text(
                  '${task['title']}',
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: DshTypography.sizeSectionTitle,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              const Text(
                DshScheduleZh.enable,
                style: TextStyle(fontSize: DshTypography.sizeAuxiliary),
              ),
              const SizedBox(width: 6),
              DshSwitch(
                key: const Key('schedule-active'),
                value: active,
                onChanged: busy != null
                    ? null
                    : (value) => run('toggle', (op) async {
                        await op.request(
                          () => widget.api.call('setActive', {
                            ...binding,
                            'active': value,
                          }, op.scope),
                        );
                        await op.request(widget.onChanged);
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
                DshScheduleZh.nextRun(
                  value: active && task['nextRunAt'] != null
                      ? '${formatScheduleTime(task['nextRunAt'] as String?)}（${relativeScheduleTime(task['nextRunAt'] as String?)}）'
                      : DshScheduleZh.none,
                ),
                DshScheduleZh.recentRun(
                  value: last.isEmpty
                      ? DshScheduleZh.neverRun
                      : '${formatScheduleTime(last['deliveredAt'] as String?)} · ${last['outcome'] == 'delivered' ? DshScheduleZh.delivered : DshScheduleZh.deliveryFailed}',
                ),
                task['origin'] == 'agent'
                    ? DshScheduleZh.agentCreated
                    : DshScheduleZh.userCreated,
              ])
                Text(
                  text,
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
                ),
            ],
          ),
          const SizedBox(height: 6),
          Row(
            children: [
              Text(
                DshScheduleZh.targetSessionLabel,
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: colors.muted,
                ),
              ),
              Flexible(
                child: Text(
                  widget.sessionTitle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: DshTypography.sizeCaption),
                ),
              ),
              if (widget.sessionKnown)
                TextButton(
                  onPressed: widget.onOpenSession,
                  child: const Text(DshScheduleZh.openSession),
                ),
            ],
          ),
          const SizedBox(height: 4),
          Row(
            children: [
              for (final (id, label) in [
                ('rules', DshScheduleZh.rule),
                (
                  'records',
                  DshScheduleZh.historyTab(count: task['historyCount'] ?? 0),
                ),
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
                    child: Text(
                      label,
                      style: const TextStyle(
                        fontSize: DshTypography.sizeAuxiliary,
                      ),
                    ),
                  ),
                ),
            ],
          ),
          const SizedBox(height: 12),
          if (notice != null)
            Text(
              notice!,
              style: TextStyle(
                fontSize: DshTypography.sizeCaption,
                color: colors.muted,
              ),
            ),
          if (error != null) DshErrorView(error: error!),
          Expanded(
            child: tab == 'rules'
                ? ListView(
                    children: [
                      _Label(
                        DshScheduleZh.name,
                        DshField(
                          controller: title,
                          onChanged: (_) => setState(() {}),
                        ),
                      ),
                      _Label(
                        DshScheduleZh.prompt,
                        DshField(
                          key: const Key('schedule-prompt'),
                          controller: prompt,
                          maxLines: 5,
                          onChanged: (_) => setState(() {}),
                        ),
                      ),
                      _Label(
                        DshScheduleZh.frequency,
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
                        final owner = operation();
                        final confirmed = await confirmAction(
                          context,
                          DshScheduleZh.deleteTask,
                          DshScheduleZh.deleteHint,
                          action: DshScheduleZh.delete,
                        );
                        if (!confirmed || !owner.valid) return;
                        await run('delete', (op) async {
                          await op.request(
                            () => widget.api.call('delete', binding, op.scope),
                          );
                          widget.onDeleted();
                        });
                      },
                child: const Text(DshScheduleZh.delete),
              ),
              DshButton(
                key: const Key('schedule-run-now'),
                onPressed: busy != null
                    ? null
                    : () => run('run', (op) async {
                        await op.request(
                          () => widget.api.call('runNow', binding, op.scope),
                        );
                        setState(() => tab = 'records');
                        await op.request(widget.onChanged);
                        if (op.valid) await loadHistory();
                      }),
                child: Text(
                  busy == 'run' ? DshScheduleZh.running : DshScheduleZh.runNow,
                ),
              ),
              if (dirty)
                Text(
                  DshScheduleZh.unsaved,
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
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
                child: const Text(DshZh.cancel),
              ),
              DshButton(
                key: const Key('schedule-save'),
                primary: true,
                onPressed: !dirty || busy != null || prompt.text.trim().isEmpty
                    ? null
                    : save,
                child: Text(busy == 'save' ? DshScheduleZh.saving : DshZh.save),
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
      return DshEmpty(DshScheduleZh.noHistory, icon: DshIcons.history.data);
    }
    return ListView(
      children: [
        Text(
          DshScheduleZh.deliveryMeaning,
          style: TextStyle(
            fontSize: DshTypography.sizeCaption,
            color: colors.muted,
          ),
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
                      record['outcome'] == 'delivered'
                          ? DshScheduleZh.delivered
                          : DshScheduleZh.deliveryFailed,
                      style: TextStyle(
                        fontWeight: FontWeight.w600,
                        color: record['outcome'] == 'delivered'
                            ? null
                            : DshTokens.of(context).error.foreground,
                      ),
                    ),
                    if (record['manual'] == true) ...[
                      const SizedBox(width: 6),
                      Text(
                        DshScheduleZh.manual,
                        style: TextStyle(
                          fontSize: DshTypography.sizeCaption,
                          color: colors.muted,
                        ),
                      ),
                    ],
                    const Spacer(),
                    Text(
                      formatScheduleTime(record['deliveredAt'] as String?),
                      style: TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        color: colors.muted,
                      ),
                    ),
                  ],
                ),
                if (record['error'] != null)
                  Text(
                    '${record['error']}',
                    style: TextStyle(
                      fontSize: DshTypography.sizeCaption,
                      color: DshTokens.of(context).error.foreground,
                    ),
                  ),
                const SizedBox(height: 4),
                SelectableText(
                  '${record['prompt']}',
                  maxLines: 3,
                  style: TextStyle(
                    fontSize: DshTypography.sizeAuxiliary,
                    color: colors.muted,
                  ),
                ),
              ],
            ),
          ),
        if (history!['earlierRecordsPruned'] == true &&
            history!['hasMore'] != true)
          Text(
            DshScheduleZh.retentionNotice(
              days: history!['retentionDays'],
              records: history!['retentionRecords'],
            ),
            style: TextStyle(
              fontSize: DshTypography.sizeCaption,
              color: colors.muted,
            ),
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
    this.ownerScope,
    this.isCurrent,
  });
  final RequestScope? ownerScope;
  final bool Function()? isCurrent;
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
  final _scope = RequestScope();
  void Function()? _detach;
  @override
  void initState() {
    super.initState();
    _detach = widget.ownerScope?.register(_scope.cancel);
  }

  PageOperation operation() =>
      PageOperation(_scope, () => mounted && widget.isCurrent?.call() != false);

  DesktopController get c => widget.controller;

  Map<String, String> get targets => {
    for (final session in c.sessions)
      if (!session.blank || session.id == c.selectedId)
        session.id: session.displayTitle,
    for (final workspace in c.workspaces)
      'new:${displayPathText('${workspace['path']}')}':
          DshScheduleZh.newWorkspaceSession(
            title: workspace['title'] ?? workspace['path'],
          ),
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
    _detach?.call();
    _scope.cancel();
    prompt.dispose();
    title.dispose();
    super.dispose();
  }

  Future<void> submit() async {
    final op = operation();
    if (!op.valid || busy) return;
    final owner = widget.api;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      var sessionId = target;
      if (target.startsWith('new:')) {
        final client = owner.client;
        final created = await op.request(
          () => client.rpc(
            'session.create',
            payload: {'cwd': target.substring(4), 'agentPreset': c.preset},
            mutation: true,
            scope: op.scope,
          ),
        );
        sessionId = created['sessionId'] as String;
        final name = (title.text.trim().isEmpty
            ? prompt.text.trim().split('\n').first
            : title.text.trim());
        try {
          await op.request(
            () => client.rpc(
              'session.rename',
              payload: {
                'sessionId': sessionId,
                'title': DshScheduleZh.sessionTitle(
                  title: name.length > 40 ? name.substring(0, 40) : name,
                ),
              },
              mutation: true,
              scope: op.scope,
            ),
          );
        } catch (_) {}
        if (!op.valid) return;
        if (identical(c.client, client)) unawaited(c.refreshSessions());
      }
      final value = await op.request(
        () => owner.call('create', {
          'sessionId': sessionId,
          'title': title.text,
          'prompt': prompt.text,
          'rule': draft.toRule(),
        }, op.scope),
      );
      if (mounted && op.valid) {
        Navigator.of(context).pop(object(value['task']));
      }
    } on DshException catch (e) {
      if (op.valid) setState(() => error = '$e');
    } catch (e) {
      if (op.valid) setState(() => error = '$e');
    } finally {
      if (op.valid) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final options = targets;
    return AlertDialog(
      title: const Text(
        DshScheduleZh.createTitle,
        style: TextStyle(fontSize: DshTypography.sizeSectionTitle),
      ),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _Label(
                DshScheduleZh.prompt,
                DshField(
                  key: const Key('schedule-create-prompt'),
                  controller: prompt,
                  maxLines: 4,
                  autofocus: true,
                  hint: DshScheduleZh.promptHint,
                  onChanged: (_) => setState(() {}),
                ),
              ),
              _Label(
                DshScheduleZh.name,
                DshField(controller: title, hint: DshScheduleZh.nameHint),
              ),
              _Label(
                DshScheduleZh.frequency,
                RuleEditor(
                  draft: draft,
                  onChanged: (next) => setState(() => draft = next),
                ),
              ),
              _Label(
                DshScheduleZh.targetSession,
                options.isEmpty
                    ? const Text(DshScheduleZh.sessionRequired)
                    : DshSelect<String>(
                        key: const Key('schedule-create-target'),
                        options: options,
                        value: options.containsKey(target) ? target : null,
                        maxWidth: 480,
                        onChanged: (value) => setState(() => target = value),
                      ),
                hint: DshScheduleZh.targetSessionHint,
              ),
              if (error != null) DshErrorView(error: error!),
            ],
          ),
        ),
      ),
      actions: [
        DshButton(
          onPressed: busy ? null : () => Navigator.pop(context),
          child: const Text(DshZh.cancel),
        ),
        DshButton(
          key: const Key('schedule-create-submit'),
          primary: true,
          onPressed:
              busy || prompt.text.trim().isEmpty || !options.containsKey(target)
              ? null
              : submit,
          child: Text(busy ? DshScheduleZh.creating : DshScheduleZh.create),
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
  ScheduleApi? _injected;
  DshClient? _client;
  String? _session;
  int _generation = 0;
  List<Json> active = [];

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_hostChanged);
    _bind(force: true, notify: false);
  }

  @override
  void didUpdateWidget(ScheduleSessionBadge oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_hostChanged);
      widget.controller.addListener(_hostChanged);
    }
    _bind(force: oldWidget.controller != widget.controller, notify: false);
  }

  void _hostChanged() => _bind();

  void _bind({bool force = false, bool notify = true}) {
    final client = widget.api?.client ?? widget.controller.client;
    final session = widget.sessionId;
    if (!force &&
        identical(client, _client) &&
        identical(widget.api, _injected) &&
        session == _session) {
      return;
    }
    watcher?.dispose();
    watcher = null;
    _client = client;
    _injected = widget.api;
    _session = session;
    final generation = ++_generation;
    active = [];
    if (notify && mounted) setState(() {});
    final owner = widget.api ?? (client == null ? null : ScheduleApi(client));
    if (owner == null) return;
    watcher = ScheduleWatch(owner, (scope) async {
      final value = await owner.call('catalog', {'sessionId': session}, scope);
      if (!mounted || generation != _generation || scope.cancelled) return null;
      final revision = (value['revision'] as num?)?.toInt();
      final next = revision == null
          ? <Json>[]
          : objects(value['tasks'])
                .where((task) => task['status'] == 'active')
                .toList();
      String signature(List<Json> tasks) => tasks
          .map((task) => '${task['id']}|${task['title']}|${task['nextRunAt']}')
          .join(',');
      if (signature(next) != signature(active)) setState(() => active = next);
      return revision;
    })..start();
  }

  @override
  void dispose() {
    _generation++;
    widget.controller.removeListener(_hostChanged);
    watcher?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (active.isEmpty) return const SizedBox.shrink();
    final colors = DshColors(context);
    final label = DshScheduleZh.activeCount(count: active.length);
    final badge = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        DshGlyph(DshIcons.alarmClock.data, size: 15, color: colors.muted),
        const SizedBox(width: 4),
        Text(
          '${active.length}',
          style: TextStyle(
            fontSize: DshTypography.sizeCaption,
            color: colors.muted,
          ),
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
                                fontSize: DshTypography.sizeCaption,
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
  bool _closed = false, _started = false;

  void start() {
    if (_closed || _started) return;
    _started = true;
    unawaited(_run());
  }

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
