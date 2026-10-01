import '../../design/error.dart';
import '../../l10n/zh.dart';

import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../src/controller.dart';

import 'package:dsh_desktop/design/typography.dart';

const scheduleKinds = <String, String>{
  'after': DshSettingsZh.delayedOnce,
  'at': DshSettingsZh.scheduledOnce,
  'every': DshSettingsZh.fixedInterval,
  'daily': DshSettingsZh.daily,
  'weekly': DshSettingsZh.weekly,
  'cron': 'Cron',
};

String scheduleFailure(Object failure) {
  if (failure is! DshException) return '$failure';
  final reason = failure.details['reason'] ?? failure.code;
  final message = const <String, String>{
    'schedule_conflict': DshSettingsZh.reminderConflict,
    'schedule_not_found': DshSettingsZh.reminderDeleted,
    'schedule_ended': DshSettingsZh.reminderEnded,
    'schedule_disabled': DshSettingsZh.reminderPluginDisabled,
    'session_archived': DshSettingsZh.reminderSessionArchived,
    'session_not_found': DshSettingsZh.reminderSessionDeleted,
  }[reason];
  return message == null
      ? '$failure'
      : '$message${failure.outcomeUnknown ? DshSettingsZh.outcomeUnknownSuffix : ''}';
}

String _sessionLabel(DesktopController controller, String id) =>
    controller.sessions
        .where((row) => row.id == id)
        .firstOrNull
        ?.displayTitle ??
    id;

String _ruleLabel(ScheduleRecord record) {
  final value = record.toJson();
  return switch (record.kind) {
    'after' => DshSettingsZh.afterSeconds(seconds: value['afterSeconds']),
    'at' => DshSettingsZh.onceAt,
    'every' => DshSettingsZh.everySeconds(seconds: value['everySeconds']),
    'daily' => DshSettingsZh.dailyAt(
      time: value['time'],
      timezone: value['timeZone'],
    ),
    'weekly' => DshSettingsZh.weeklyAt(
      days: (value['weekdays'] as List).join('、'),
      time: value['time'],
      timezone: value['timeZone'],
    ),
    'cron' => '${value['expression']} · ${value['timeZone']}',
    _ => record.kind,
  };
}

class SchedulePanel extends StatefulWidget {
  const SchedulePanel({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<SchedulePanel> createState() => _SchedulePanelState();
}

class _SchedulePanelState extends State<SchedulePanel> {
  late final DshClient? client = widget.controller.client;
  late int revision;
  late String? selected;
  final search = TextEditingController();
  RequestScope readScope = RequestScope(), mutationScope = RequestScope();
  List<ScheduleEntry> entries = [];
  Json? plugin;
  String filter = 'all';
  String? error, notice;
  int generation = 0;
  bool loading = true, busy = false;
  bool get stale => client == null || client != widget.controller.client;
  bool get enabled => plugin?['enabled'] == true;
  bool get disabled => stale || busy || loading;

  @override
  void initState() {
    super.initState();
    revision = widget.controller.scheduleRevision;
    selected = widget.controller.selectedId;
    widget.controller.addListener(changed);
    load();
  }

  @override
  void dispose() {
    widget.controller.removeListener(changed);
    readScope.cancel();
    mutationScope.cancel();
    search.dispose();
    super.dispose();
  }

  void changed() {
    if (!mounted) return;
    if (stale) {
      generation++;
      readScope.cancel();
      mutationScope.cancel();
      setState(() {
        loading = busy = false;
        error = DshSettingsZh.reminderConnectionChanged;
      });
    } else if (revision != widget.controller.scheduleRevision ||
        selected != widget.controller.selectedId) {
      if (selected != widget.controller.selectedId) {
        mutationScope.cancel();
        mutationScope = RequestScope();
        busy = false;
      }
      selected = widget.controller.selectedId;
      revision = widget.controller.scheduleRevision;
      load();
    }
  }

  Future<void> load() async {
    if (!mounted || stale) return;
    final version = ++generation;
    readScope.cancel();
    final scope = readScope = RequestScope();
    bool current() => mounted && !stale && version == generation;
    setState(() {
      loading = true;
      error = null;
    });
    try {
      final values = await Future.wait<Object>([
        ScheduleApi(client!).catalog(scope: scope),
        client!.rpc('pluginInventory.list', scope: scope),
      ]);
      final inventory = object(values[1]);
      if (inventory['entries'] is! List) {
        throw StateError(DshSettingsZh.pluginCatalogInvalid);
      }
      final matches = objects(inventory['entries'])
          .where(
            (entry) => [
              'dsh-schedule',
              '@deepseek-ai/dsh-schedule',
            ].contains(entry['moduleName']),
          )
          .toList();
      if (matches.length > 1) {
        throw StateError(DshSettingsZh.multipleReminderPlugins);
      }
      if (current()) {
        setState(() {
          entries = values[0] as List<ScheduleEntry>;
          plugin = matches.firstOrNull;
        });
      }
    } catch (e) {
      if (current()) setState(() => error = scheduleFailure(e));
    } finally {
      if (current()) setState(() => loading = false);
    }
  }

  Future<void> mutate(
    Future<void> Function(RequestScope) action,
    String message,
  ) async {
    if (disabled) return;
    final scope = mutationScope, originalSession = selected;
    bool current() =>
        mounted && !stale && !scope.cancelled && selected == originalSession;
    setState(() {
      busy = true;
      error = notice = null;
    });
    try {
      await action(scope);
      if (current()) {
        setState(() => notice = message);
        await load();
      }
    } catch (e) {
      if (current()) setState(() => error = scheduleFailure(e));
    } finally {
      if (current()) setState(() => busy = false);
    }
  }

  Future<void> edit([ScheduleEntry? entry]) async {
    if (disabled || !enabled) return;
    final saved = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (_) =>
          ScheduleEditor(controller: widget.controller, entry: entry),
    );
    if (saved == true && mounted && !stale) await load();
  }

  Future<void> remove(ScheduleEntry entry) async {
    if (disabled) return;
    final original = selected, scope = mutationScope;
    final confirmed = await confirmAction(
      context,
      DshSettingsZh.deleteReminderTitle,
      DshSettingsZh.deleteReminderHint(title: entry.record.title),
      action: DshSettingsZh.delete,
    );
    if (!confirmed ||
        !mounted ||
        stale ||
        scope.cancelled ||
        original != selected) {
      return;
    }
    await mutate((scope) async {
      await ScheduleApi(
        client!,
      ).delete(sessionId: entry.sessionId, id: entry.record.id, scope: scope);
    }, DshSettingsZh.reminderDeletedNotice);
  }

  @override
  Widget build(BuildContext context) {
    final query = search.text.trim().toLowerCase();
    final rows = entries
        .where(
          (entry) =>
              (filter == 'all' || entry.status == filter) &&
              '${entry.record.title} ${entry.record.prompt} ${entry.sessionId} ${_sessionLabel(widget.controller, entry.sessionId)}'
                  .toLowerCase()
                  .contains(query),
        )
        .toList();
    return ListView(
      children: [
        const Text(
          DshSettingsZh.reminders,
          style: TextStyle(
            fontSize: DshTypography.sizeSectionTitle,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 12),
        const Text(DshSettingsZh.remindersDescription),
        const SizedBox(height: 12),
        Row(
          children: [
            Expanded(
              child: Text(
                enabled
                    ? DshSettingsZh.reminderEnabled
                    : DshSettingsZh.reminderDisabledHint,
              ),
            ),
            const SizedBox(width: 12),
            DshSwitch(
              key: const ValueKey('schedule-enabled'),
              value: enabled,
              onChanged: disabled || plugin == null
                  ? null
                  : (value) => mutate(
                      (scope) async {
                        await client!.rpc(
                          'pluginInventory.setEnabled',
                          mutation: true,
                          scope: scope,
                          payload: {
                            'entryId': plugin!['entryId'],
                            'enabled': value,
                          },
                        );
                      },
                      value
                          ? DshSettingsZh.reminderEnabledNotice
                          : DshSettingsZh.reminderDisabledNotice,
                    ),
            ),
          ],
        ),
        if (plugin == null && !loading)
          const Text(DshSettingsZh.reminderPluginMissing),
        const SizedBox(height: 12),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            DshButton(
              key: const ValueKey('schedule-create'),
              primary: true,
              icon: DshIcons.plus.data,
              onPressed: disabled || !enabled ? null : () => edit(),
              child: const Text(DshSettingsZh.createReminder),
            ),
            DshButton(
              outline: true,
              onPressed: stale || busy ? null : load,
              child: const Text(DshSettingsZh.refresh),
            ),
            DshButton(
              key: const ValueKey('schedule-retention'),
              outline: true,
              onPressed: disabled || plugin == null
                  ? null
                  : () => showDialog<void>(
                      context: context,
                      barrierDismissible: false,
                      builder: (_) => ScheduleRetentionDialog(
                        controller: widget.controller,
                        entryId: plugin!['entryId'] as String,
                      ),
                    ),
              child: const Text(DshSettingsZh.receiptRetention),
            ),
            DshButton(
              key: const ValueKey('schedule-retry'),
              outline: true,
              onPressed: disabled || !enabled
                  ? null
                  : () => mutate(
                      (scope) => ScheduleApi(client!).retry(scope: scope),
                      DshSettingsZh.remindersRetryRequested,
                    ),
              child: const Text(DshSettingsZh.retryReminders),
            ),
            DshSelect<String>(
              key: const ValueKey('schedule-filter'),
              value: filter,
              options: const {
                'all': DshSettingsZh.all,
                'active': DshSettingsZh.active,
                'inactive': DshSettingsZh.ended,
              },
              onChanged: (value) => setState(() => filter = value),
            ),
          ],
        ),
        const SizedBox(height: 12),
        DshField(
          key: const ValueKey('schedule-search'),
          controller: search,
          hint: DshSettingsZh.searchReminders,
          prefix: DshIcons.search.data,
          onChanged: (_) => setState(() {}),
        ),
        if (loading)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 12),
            child: LinearProgressIndicator(),
          ),
        if (error != null) _message(error!, error: true),
        if (notice != null) _message(notice!),
        if (!loading && rows.isEmpty) const DshEmpty(DshSettingsZh.noReminders),
        for (final entry in rows)
          Container(
            key: ValueKey('schedule-entry-${entry.record.id}'),
            padding: const EdgeInsets.symmetric(vertical: 16),
            decoration: BoxDecoration(
              border: Border(
                bottom: BorderSide(color: DshColors(context).border),
              ),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '${entry.record.title} · ${entry.active ? DshSettingsZh.active : DshSettingsZh.ended}',
                  style: const TextStyle(fontWeight: FontWeight.w600),
                ),
                const SizedBox(height: 6),
                Text(
                  DshSettingsZh.sessionLabel(
                    title: _sessionLabel(widget.controller, entry.sessionId),
                  ),
                ),
                Text(_ruleLabel(entry.record)),
                Text(
                  '${entry.active ? DshSettingsZh.nextScheduledTime : DshSettingsZh.scheduledTime}：${entry.record.scheduledAt}',
                ),
                if (entry.lastDelivery != null)
                  Text(
                    DshSettingsZh.lastDelivery(
                      time: entry.lastDelivery!.deliveredAt,
                    ),
                  ),
                const SizedBox(height: 6),
                Text(
                  entry.record.prompt,
                  maxLines: 3,
                  overflow: TextOverflow.ellipsis,
                ),
                const SizedBox(height: 8),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    DshButton(
                      key: ValueKey('schedule-edit-${entry.record.id}'),
                      outline: true,
                      onPressed: disabled || !enabled || !entry.active
                          ? null
                          : () => edit(entry),
                      child: const Text(DshSettingsZh.edit),
                    ),
                    DshButton(
                      key: ValueKey('schedule-history-${entry.record.id}'),
                      outline: true,
                      onPressed: disabled
                          ? null
                          : () => showDialog<void>(
                              context: context,
                              builder: (_) => ScheduleHistoryDialog(
                                controller: widget.controller,
                                entry: entry,
                              ),
                            ),
                      child: const Text(DshSettingsZh.deliveryReceipts),
                    ),
                    DshButton(
                      key: ValueKey('schedule-delete-${entry.record.id}'),
                      destructive: true,
                      onPressed: disabled ? null : () => remove(entry),
                      child: const Text(DshSettingsZh.delete),
                    ),
                  ],
                ),
              ],
            ),
          ),
      ],
    );
  }
}

Widget _message(String text, {bool error = false}) => Padding(
  padding: const EdgeInsets.symmetric(vertical: 10),
  child: error
      ? DshErrorView(error: text)
      : SelectableText(
          text,
          style: const TextStyle(fontSize: DshTypography.sizeAuxiliary),
        ),
);

/// An editor owns its request scope and its original Host/selection identity.
class ScheduleEditor extends StatefulWidget {
  const ScheduleEditor({super.key, required this.controller, this.entry});
  final DesktopController controller;
  final ScheduleEntry? entry;
  @override
  State<ScheduleEditor> createState() => _ScheduleEditorState();
}

class _ScheduleEditorState extends State<ScheduleEditor> {
  late final DshClient? client = widget.controller.client;
  late final String? selected = widget.controller.selectedId;
  late final int selection = widget.controller.selectionRevision;
  late ScheduleRecord? expected = widget.entry?.record;
  late String? sessionId =
      widget.entry?.sessionId ??
      (sessions.containsKey(selected) ? selected : null);
  final scope = RequestScope();
  late final title = TextEditingController(text: expected?.title ?? '');
  late final prompt = TextEditingController(text: expected?.prompt ?? '');
  late final seconds = TextEditingController(
    text:
        '${expected?.toJson()['everySeconds'] ?? expected?.toJson()['afterSeconds'] ?? 600}',
  );
  late final time = TextEditingController(
    text: '${expected?.toJson()['time'] ?? '09:00'}',
  );
  late final zone = TextEditingController(
    text: '${expected?.toJson()['timeZone'] ?? 'Asia/Shanghai'}',
  );
  final date = TextEditingController();
  late final instant = TextEditingController(text: expected?.scheduledAt ?? '');
  late final expression = TextEditingController(
    text: '${expected?.toJson()['expression'] ?? '0 9 * * *'}',
  );
  late String kind = expected == null
      ? 'after'
      : expected!.kind == 'after'
      ? 'at'
      : expected!.kind;
  late String atMode = expected == null ? 'local' : 'instant';
  Set<int> weekdays = {1};
  bool busy = false,
      changeTiming = false,
      invalidated = false,
      conflict = false;
  String? error, notice;
  bool get stale =>
      invalidated ||
      client == null ||
      client != widget.controller.client ||
      selection != widget.controller.selectionRevision ||
      selected != widget.controller.selectedId;
  bool get disabled =>
      busy || stale || widget.controller.scheduleEnabled == false;
  bool get timingEditable => expected == null || changeTiming;
  Map<String, String> get sessions => {
    for (final row in widget.controller.sessions)
      if (!widget.controller.archivedSessionIds.contains(row.id))
        row.id: row.displayTitle,
  };
  @override
  void initState() {
    super.initState();
    weekdays = (expected?.toJson()['weekdays'] as List? ?? <int>[1])
        .cast<int>()
        .toSet();
    widget.controller.addListener(changed);
  }

  @override
  void dispose() {
    widget.controller.removeListener(changed);
    scope.cancel();
    for (final input in [
      title,
      prompt,
      seconds,
      time,
      zone,
      date,
      instant,
      expression,
    ]) {
      input.dispose();
    }
    super.dispose();
  }

  void changed() {
    if (!mounted) return;
    if (stale) {
      invalidated = true;
      scope.cancel();
      setState(() {
        busy = false;
        error = DshSettingsZh.reminderDraftStale;
      });
    } else {
      setState(() {});
    }
  }

  Json timing({required bool update}) {
    if (kind == 'after' || kind == 'every') {
      final value = int.tryParse(seconds.text.trim());
      final minimum = kind == 'every' ? 60 : 1;
      if (value == null || value < minimum || value > 9007199254740991) {
        throw FormatException(DshSettingsZh.secondsInvalid(minimum: minimum));
      }
      return {if (update) 'kind': kind, '${kind}_seconds': value};
    }
    if (kind == 'at' && atMode == 'instant') {
      final value = instant.text.trim();
      if (!RegExp(r'(Z|[+-]\d{2}:\d{2})$').hasMatch(value) ||
          DateTime.tryParse(value) == null) {
        throw const FormatException(DshSettingsZh.timestampOffsetRequired);
      }
      return {if (update) 'kind': kind, 'at': value};
    }
    final tz = zone.text.trim();
    if (tz != 'UTC' &&
        !RegExp(r'^[A-Za-z_+-]+(?:/[A-Za-z0-9_+.-]+)+$').hasMatch(tz)) {
      throw const FormatException(DshSettingsZh.timezoneRequired);
    }
    if (kind != 'cron' &&
        !RegExp(r'^([01]\d|2[0-3]):[0-5]\d(?::[0-5]\d(?:\.\d{1,3})?)?$')
            .hasMatch(time.text.trim())) {
      throw const FormatException(DshSettingsZh.timeFormatInvalid);
    }
    final wallTime = time.text.trim().length == 5
        ? '${time.text.trim()}:00'
        : time.text.trim();
    Object value;
    if (kind == 'at') {
      if (!RegExp(r'^\d{4}-\d{2}-\d{2}$').hasMatch(date.text.trim())) {
        throw const FormatException(DshSettingsZh.dateFormatInvalid);
      }
      value = {'date': date.text.trim(), 'time': wallTime, 'time_zone': tz};
    } else if (kind == 'cron') {
      if (expression.text.trim().split(RegExp(r'\s+')).length != 5) {
        throw const FormatException(DshSettingsZh.cronInvalid);
      }
      value = {'expression': expression.text.trim(), 'time_zone': tz};
    } else {
      if (kind == 'weekly' && weekdays.isEmpty) {
        throw const FormatException(DshSettingsZh.weekdaysRequired);
      }
      value = {
        'time': wallTime,
        'time_zone': tz,
        if (kind == 'weekly') 'weekdays': weekdays.toList()..sort(),
      };
    }
    return {if (update) 'kind': kind, kind: value};
  }

  Future<void> save() async {
    if (disabled) return;
    setState(() {
      busy = true;
      error = notice = null;
    });
    try {
      if (sessionId == null ||
          title.text.trim().isEmpty ||
          prompt.text.trim().isEmpty) {
        throw const FormatException(DshSettingsZh.reminderFieldsRequired);
      }
      final api = ScheduleApi(client!);
      if (expected == null) {
        await api.create(
          sessionId: sessionId!,
          title: title.text.trim(),
          prompt: prompt.text.trim(),
          timing: timing(update: false),
          scope: scope,
        );
      } else {
        final result = await api.update(
          sessionId: sessionId!,
          expected: expected!,
          title: title.text.trim(),
          prompt: prompt.text.trim(),
          change: changeTiming ? timing(update: true) : null,
          scope: scope,
        );
        if (result.code != null) {
          if (result.code == 'schedule_conflict' && mounted && !stale) {
            conflict = true;
          }
          throw DshException(result.code!, result.code!);
        }
      }
      if (mounted && !stale) Navigator.pop(context, true);
    } catch (e) {
      if (mounted && !stale) setState(() => error = scheduleFailure(e));
    } finally {
      if (mounted && !stale) setState(() => busy = false);
    }
  }

  Future<void> refreshExpected() async {
    if (disabled || expected == null) return;
    setState(() => busy = true);
    try {
      final entries = await ScheduleApi(client!).catalog(scope: scope);
      if (!mounted || stale) return;
      final entry = entries
          .where(
            (row) =>
                row.sessionId == sessionId && row.record.id == expected!.id,
          )
          .firstOrNull;
      if (entry == null) {
        throw DshException('schedule_not_found', DshSettingsZh.reminderMissing);
      }
      if (!entry.active) {
        throw DshException('schedule_ended', DshSettingsZh.reminderEndedState);
      }
      setState(() {
        expected = entry.record;
        conflict = false;
        error = null;
        notice = DshSettingsZh.latestReminder(
          title: entry.record.title,
          prompt: entry.record.prompt,
          scheduledAt: entry.record.scheduledAt,
        );
      });
    } catch (e) {
      if (mounted && !stale) setState(() => error = scheduleFailure(e));
    } finally {
      if (mounted && !stale) setState(() => busy = false);
    }
  }

  Widget input(
    String label,
    TextEditingController controller,
    String key, {
    String? hint,
    int lines = 1,
    bool timingField = false,
  }) => Padding(
    padding: const EdgeInsets.only(top: 12),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label),
        const SizedBox(height: 5),
        DshField(
          key: ValueKey(key),
          controller: controller,
          hint: hint,
          maxLines: lines,
          enabled: !busy && !stale && (!timingField || timingEditable),
        ),
      ],
    ),
  );

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(
      expected == null
          ? DshSettingsZh.createReminder
          : DshSettingsZh.editReminder,
    ),
    content: SizedBox(
      width: 600,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (expected == null) ...[
              const Text(DshSettingsZh.boundSession),
              const SizedBox(height: 6),
              DshSelect<String>(
                key: const ValueKey('schedule-session'),
                options: sessions,
                value: sessions.containsKey(sessionId) ? sessionId : null,
                maxWidth: 550,
                onChanged: disabled
                    ? null
                    : (value) => setState(() => sessionId = value),
              ),
              if (sessions.isEmpty)
                const Text(DshSettingsZh.reminderSessionRequired),
            ] else
              SelectableText(
                DshSettingsZh.boundSessionHint(
                  title: _sessionLabel(widget.controller, sessionId!),
                  id: sessionId,
                ),
              ),
            input(DshSettingsZh.title, title, 'schedule-title'),
            input(
              DshSettingsZh.sessionPrompt,
              prompt,
              'schedule-prompt',
              lines: 4,
            ),
            if (expected != null)
              CheckboxListTile(
                contentPadding: EdgeInsets.zero,
                key: const ValueKey('schedule-change-timing'),
                title: const Text(DshSettingsZh.editTrigger),
                value: changeTiming,
                onChanged: disabled
                    ? null
                    : (value) => setState(() => changeTiming = value == true),
              ),
            const SizedBox(height: 12),
            const Text(DshSettingsZh.trigger),
            const SizedBox(height: 6),
            DshSelect<String>(
              key: const ValueKey('schedule-kind'),
              value: kind,
              options: {
                for (final item in scheduleKinds.entries)
                  if (expected == null || item.key != 'after')
                    item.key: item.value,
              },
              onChanged: disabled || !timingEditable
                  ? null
                  : (value) => setState(() => kind = value),
            ),
            if (expected?.kind == 'after')
              const Text(DshSettingsZh.delayedRescheduleHint),
            if (kind == 'after' || kind == 'every')
              input(
                kind == 'after'
                    ? DshSettingsZh.delaySeconds
                    : DshSettingsZh.intervalSeconds,
                seconds,
                'schedule-seconds',
                timingField: true,
              ),
            if (kind == 'at') ...[
              const SizedBox(height: 10),
              DshSelect<String>(
                key: const ValueKey('schedule-at-mode'),
                value: atMode,
                options: const {
                  'local': DshSettingsZh.dateTimeZone,
                  'instant': DshSettingsZh.zonedTime,
                },
                onChanged: disabled || !timingEditable
                    ? null
                    : (value) => setState(() => atMode = value),
              ),
              if (atMode == 'instant')
                input(
                  DshSettingsZh.timestamp,
                  instant,
                  'schedule-instant',
                  hint: '2030-01-01T09:00:00+08:00',
                  timingField: true,
                )
              else
                input(
                  DshSettingsZh.date,
                  date,
                  'schedule-date',
                  hint: '2030-01-01',
                  timingField: true,
                ),
            ],
            if (['daily', 'weekly'].contains(kind) ||
                (kind == 'at' && atMode == 'local'))
              input(
                DshSettingsZh.time24,
                time,
                'schedule-time',
                hint: '09:00',
                timingField: true,
              ),
            if (kind == 'weekly')
              Wrap(
                spacing: 6,
                children: [
                  for (var day = 1; day <= 7; day++)
                    FilterChip(
                      key: ValueKey('schedule-weekday-$day'),
                      label: Text(DshSettingsZh.weekdayLabel(day: day)),
                      selected: weekdays.contains(day),
                      onSelected: disabled || !timingEditable
                          ? null
                          : (value) => setState(() {
                              value ? weekdays.add(day) : weekdays.remove(day);
                            }),
                    ),
                ],
              ),
            if (kind == 'cron') ...[
              input(
                DshSettingsZh.cron,
                expression,
                'schedule-cron',
                hint: '0 9 * * *',
                timingField: true,
              ),
              const Text(DshSettingsZh.cronHint),
            ],
            if (['daily', 'weekly', 'cron'].contains(kind) ||
                (kind == 'at' && atMode == 'local')) ...[
              input(
                DshSettingsZh.ianaTimezone,
                zone,
                'schedule-zone',
                hint: 'Asia/Shanghai',
                timingField: true,
              ),
              const Text(DshSettingsZh.timezoneHint),
            ],
            if (widget.controller.scheduleEnabled == false)
              _message(DshSettingsZh.reminderDraftDisabled),
            if (error != null) _message(error!, error: true),
            if (notice != null) _message(notice!),
            if (conflict)
              DshButton(
                key: const ValueKey('schedule-refresh-expected'),
                outline: true,
                onPressed: disabled ? null : refreshExpected,
                child: const Text(DshSettingsZh.refreshKeepDraft),
              ),
          ],
        ),
      ),
    ),
    actions: [
      DshButton(
        onPressed: () => Navigator.pop(context),
        child: const Text(DshSettingsZh.close),
      ),
      DshButton(
        key: const ValueKey('schedule-save'),
        primary: true,
        onPressed: disabled ? null : save,
        child: Text(busy ? DshZh.saving : DshSettingsZh.saveReminder),
      ),
    ],
  );
}

class ScheduleRetentionDialog extends StatefulWidget {
  const ScheduleRetentionDialog({
    super.key,
    required this.controller,
    required this.entryId,
  });
  final DesktopController controller;
  final String entryId;
  @override
  State<ScheduleRetentionDialog> createState() =>
      _ScheduleRetentionDialogState();
}

class _ScheduleRetentionDialogState extends State<ScheduleRetentionDialog> {
  late final client = widget.controller.client;
  late final selection = widget.controller.selectionRevision;
  late final selected = widget.controller.selectedId;
  final scope = RequestScope(),
      days = TextEditingController(),
      records = TextEditingController();
  Json? snapshot;
  bool busy = true, invalidated = false;
  String? error, notice;
  bool get stale =>
      invalidated ||
      client == null ||
      client != widget.controller.client ||
      selection != widget.controller.selectionRevision ||
      selected != widget.controller.selectedId;
  @override
  void initState() {
    super.initState();
    widget.controller.addListener(changed);
    load();
  }

  @override
  void dispose() {
    widget.controller.removeListener(changed);
    scope.cancel();
    days.dispose();
    records.dispose();
    super.dispose();
  }

  void changed() {
    if (stale) {
      invalidated = true;
      scope.cancel();
      if (mounted) {
        setState(() {
          busy = false;
          error = DshSettingsZh.retentionDraftStale;
        });
      }
    }
  }

  Json validate(Json value) {
    if (value['entryId'] != widget.entryId ||
        value['revision'] is! String ||
        (value['config'] != null && value['config'] is! Map)) {
      throw StateError(DshSettingsZh.retentionResponseInvalid);
    }
    return value;
  }

  Future<void> load({bool preserve = false}) async {
    if (stale) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final value = validate(
        await client!.rpc(
          'pluginInventory.getConfig',
          scope: scope,
          payload: {'entryId': widget.entryId},
        ),
      );
      if (!mounted || stale) return;
      setState(() {
        snapshot = value;
        final config = object(value['config']);
        if (!preserve) {
          days.text = '${config['deliveryHistoryDays'] ?? 30}';
          records.text = '${config['deliveryHistoryRecords'] ?? 200}';
        } else {
          notice = DshSettingsZh.latestRetention(
            days: config['deliveryHistoryDays'] ?? 30,
            records: config['deliveryHistoryRecords'] ?? 200,
          );
        }
      });
    } catch (e) {
      if (mounted && !stale) setState(() => error = scheduleFailure(e));
    } finally {
      if (mounted && !stale) setState(() => busy = false);
    }
  }

  Future<void> save() async {
    if (stale || busy || snapshot == null) return;
    setState(() {
      busy = true;
      error = notice = null;
    });
    try {
      final dayCount = int.tryParse(days.text.trim()),
          recordCount = int.tryParse(records.text.trim());
      if (dayCount == null ||
          dayCount < 1 ||
          dayCount > 3650 ||
          recordCount == null ||
          recordCount < 1 ||
          recordCount > 10000) {
        throw const FormatException(DshSettingsZh.retentionInvalid);
      }
      final value = validate(
        await client!.rpc(
          'pluginInventory.setConfig',
          mutation: true,
          scope: scope,
          payload: {
            'entryId': widget.entryId,
            'expectedRevision': snapshot!['revision'],
            'config': {
              ...object(snapshot!['config']),
              'deliveryHistoryDays': dayCount,
              'deliveryHistoryRecords': recordCount,
            },
          },
        ),
      );
      if (mounted && !stale) {
        setState(() {
          snapshot = value;
          notice = DshSettingsZh.retentionSaved;
        });
      }
    } catch (e) {
      if (mounted && !stale) {
        setState(
          () => error = DshSettingsZh.retentionSaveFailed(
            detail: scheduleFailure(e),
          ),
        );
      }
    } finally {
      if (mounted && !stale) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text(DshSettingsZh.receiptRetention),
    content: SizedBox(
      width: 500,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(DshSettingsZh.retentionHint),
            const SizedBox(height: 12),
            const Text(DshSettingsZh.retentionDays),
            DshField(
              key: const ValueKey('schedule-retention-days'),
              controller: days,
              enabled: !busy && !stale,
            ),
            const SizedBox(height: 12),
            const Text(DshSettingsZh.retentionCount),
            DshField(
              key: const ValueKey('schedule-retention-records'),
              controller: records,
              enabled: !busy && !stale,
            ),
            if (busy) const LinearProgressIndicator(),
            if (error != null) _message(error!, error: true),
            if (notice != null) _message(notice!),
          ],
        ),
      ),
    ),
    actions: [
      DshButton(
        onPressed: () => Navigator.pop(context),
        child: const Text(DshSettingsZh.close),
      ),
      DshButton(
        key: const ValueKey('schedule-retention-reload'),
        onPressed: busy || stale
            ? null
            : () => load(preserve: snapshot != null),
        child: const Text(DshSettingsZh.refreshConfiguration),
      ),
      DshButton(
        key: const ValueKey('schedule-retention-save'),
        primary: true,
        onPressed: busy || stale || snapshot == null ? null : save,
        child: const Text(DshSettingsZh.saveRetention),
      ),
    ],
  );
}

class ScheduleHistoryDialog extends StatefulWidget {
  const ScheduleHistoryDialog({
    super.key,
    required this.controller,
    required this.entry,
  });
  final DesktopController controller;
  final ScheduleEntry entry;
  @override
  State<ScheduleHistoryDialog> createState() => _ScheduleHistoryDialogState();
}

class _ScheduleHistoryDialogState extends State<ScheduleHistoryDialog> {
  late final DshClient? client = widget.controller.client;
  late final String? selected = widget.controller.selectedId;
  late final int selection = widget.controller.selectionRevision;
  late int revision;
  RequestScope scope = RequestScope();
  List<ScheduleDelivery> records = [];
  ScheduleHistoryPage? page;
  String? error;
  bool loading = false, invalidated = false;
  int generation = 0;
  bool get stale =>
      invalidated ||
      client == null ||
      client != widget.controller.client ||
      selection != widget.controller.selectionRevision ||
      selected != widget.controller.selectedId;
  @override
  void initState() {
    super.initState();
    revision = widget.controller.scheduleRevision;
    widget.controller.addListener(changed);
    load();
  }

  @override
  void dispose() {
    widget.controller.removeListener(changed);
    scope.cancel();
    super.dispose();
  }

  void changed() {
    if (stale) {
      invalidated = true;
      generation++;
      scope.cancel();
      if (mounted) {
        setState(() {
          loading = false;
          error = DshSettingsZh.receiptsConnectionChanged;
        });
      }
    } else if (revision != widget.controller.scheduleRevision) {
      revision = widget.controller.scheduleRevision;
      load();
    }
  }

  Future<void> load({bool more = false}) async {
    if (stale || (more && (loading || page?.nextBefore == null))) return;
    final version = ++generation;
    scope.cancel();
    final request = scope = RequestScope();
    final before = more ? page?.nextBefore : null;
    bool current() => mounted && !stale && version == generation;
    setState(() {
      loading = true;
      error = null;
      if (!more) {
        records = [];
        page = null;
      }
    });
    try {
      final result = await ScheduleApi(client!).history(
        sessionId: widget.entry.sessionId,
        id: widget.entry.record.id,
        before: before,
        scope: request,
      );
      if (result.code != null) throw DshException(result.code!, result.code!);
      if (current()) {
        setState(() {
          final ids = records.map((record) => record.messageId).toSet();
          records = [
            ...records,
            ...result.records.where((record) => ids.add(record.messageId)),
          ];
          page = result;
        });
      }
    } catch (e) {
      if (current()) setState(() => error = scheduleFailure(e));
    } finally {
      if (current()) setState(() => loading = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(DshSettingsZh.receiptsTitle(title: widget.entry.record.title)),
    content: SizedBox(
      width: 620,
      height: 460,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(DshSettingsZh.receiptsMeaning),
          if (page?.retentionDays != null)
            Text(
              DshSettingsZh.retentionSummary(
                days: page!.retentionDays,
                records: page!.retentionRecords,
              ),
            ),
          if (page?.earlierRecordsPruned == true)
            const Text(DshSettingsZh.receiptsPruned),
          if (page?.earlierRecordsUnavailable == true)
            const Text(DshSettingsZh.receiptsUnavailable),
          if (error != null) _message(error!, error: true),
          if (loading) const LinearProgressIndicator(),
          Expanded(
            child: ListView(
              children: [
                if (!loading && records.isEmpty && error == null)
                  const DshEmpty(DshSettingsZh.noReceipts),
                for (final record in records)
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 12),
                    child: SelectableText(
                      DshSettingsZh.deliveryDetails(
                        deliveredAt: record.deliveredAt,
                        scheduledAt: record.scheduledAt,
                        messageId: record.messageId,
                        prompt: record.prompt,
                      ),
                    ),
                  ),
                if (page?.nextBefore != null)
                  DshButton(
                    key: const ValueKey('schedule-history-more'),
                    onPressed: loading || stale ? null : () => load(more: true),
                    child: const Text(DshSettingsZh.earlierReceipts),
                  ),
              ],
            ),
          ),
        ],
      ),
    ),
    actions: [
      DshButton(
        onPressed: stale || loading ? null : () => load(),
        child: const Text(DshSettingsZh.refreshReceipts),
      ),
      DshButton(
        onPressed: () => Navigator.pop(context),
        child: const Text(DshSettingsZh.close),
      ),
    ],
  );
}
