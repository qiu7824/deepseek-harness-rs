import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../src/controller.dart';

const scheduleKinds = <String, String>{
  'after': '延迟一次',
  'at': '指定时间',
  'every': '固定间隔',
  'daily': '每天',
  'weekly': '每周',
  'cron': 'Cron',
};

String scheduleFailure(Object failure) {
  if (failure is! DshException) return '$failure';
  final reason = failure.details['reason'] ?? failure.code;
  final message = const <String, String>{
    'schedule_conflict': '提醒已被其他操作更新；草稿已保留，请读取最新版本后再保存。',
    'schedule_not_found': '提醒已被删除，请刷新目录。',
    'schedule_ended': '此提醒已结束，不能修改；可新建提醒。',
    'schedule_disabled': '提醒插件已停用；请在目录中手动启用后再保存。',
    'session_archived': '绑定会话已归档，不能接收提醒。',
    'session_not_found': '绑定会话已删除，不能接收提醒。',
  }[reason];
  return message == null
      ? '$failure'
      : '$message${failure.outcomeUnknown ? ' 操作结果尚未确认，请刷新核对。' : ''}';
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
    'after' => '延迟 ${value['afterSeconds']} 秒，一次',
    'at' => '指定时间，一次',
    'every' => '每 ${value['everySeconds']} 秒',
    'daily' => '每天 ${value['time']} · ${value['timeZone']}',
    'weekly' =>
      '周 ${(value['weekdays'] as List).join('、')} ${value['time']} · ${value['timeZone']}',
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
        error = '连接已变化，请重新打开全局提醒。';
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
      if (inventory['entries'] is! List) throw StateError('插件目录返回的数据不完整。');
      final matches = objects(inventory['entries'])
          .where(
            (entry) => [
              'dsh-schedule',
              '@deepseek-ai/dsh-schedule',
            ].contains(entry['moduleName']),
          )
          .toList();
      if (matches.length > 1) throw StateError('存在多个提醒插件，请先在插件设置中核对。');
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
      '删除提醒？',
      '删除“${entry.record.title}”及其发送回执，后续不再发送。会话中的消息保留。',
      action: '删除',
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
    }, '提醒已删除。');
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
          '全局提醒',
          style: TextStyle(fontSize: 18, fontWeight: FontWeight.w600),
        ),
        const SizedBox(height: 12),
        const Text(
          '提醒绑定会话，由 Host 到时发送；客户端可关闭，Host 服务需要保持运行。发送回执表示消息已送入会话，不代表模型已执行成功。',
        ),
        const SizedBox(height: 12),
        Row(
          children: [
            Expanded(
              child: Text(
                enabled
                    ? '提醒已启用'
                    : '提醒默认关闭；启用后才会发送或允许新建、修改。停用期间仍可查看目录、回执和删除提醒。',
              ),
            ),
            const SizedBox(width: 12),
            DshSwitch(
              key: const ValueKey('schedule-enabled'),
              value: enabled,
              onChanged: disabled || plugin == null
                  ? null
                  : (value) => mutate((scope) async {
                      await client!.rpc(
                        'pluginInventory.setEnabled',
                        mutation: true,
                        scope: scope,
                        payload: {
                          'entryId': plugin!['entryId'],
                          'enabled': value,
                        },
                      );
                    }, value ? '提醒已启用。' : '提醒已停用。'),
            ),
          ],
        ),
        if (plugin == null && !loading) const Text('未找到提醒插件；请检查 Host 插件库存。'),
        const SizedBox(height: 12),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            DshButton(
              key: const ValueKey('schedule-create'),
              primary: true,
              icon: LucideIcons.plus,
              onPressed: disabled || !enabled ? null : () => edit(),
              child: const Text('新建提醒'),
            ),
            DshButton(
              outline: true,
              onPressed: stale || busy ? null : load,
              child: const Text('刷新'),
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
              child: const Text('回执保留设置'),
            ),
            DshButton(
              key: const ValueKey('schedule-retry'),
              outline: true,
              onPressed: disabled || !enabled
                  ? null
                  : () => mutate(
                      (scope) => ScheduleApi(client!).retry(scope: scope),
                      '已请求重新检查待发送提醒，请查看发送回执。',
                    ),
              child: const Text('重试待发送提醒'),
            ),
            DshSelect<String>(
              key: const ValueKey('schedule-filter'),
              value: filter,
              options: const {'all': '全部', 'active': '有效', 'inactive': '已结束'},
              onChanged: (value) => setState(() => filter = value),
            ),
          ],
        ),
        const SizedBox(height: 12),
        DshField(
          key: const ValueKey('schedule-search'),
          controller: search,
          hint: '搜索标题、提示或会话',
          prefix: LucideIcons.search,
          onChanged: (_) => setState(() {}),
        ),
        if (loading)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 12),
            child: LinearProgressIndicator(),
          ),
        if (error != null) _message(error!, error: true),
        if (notice != null) _message(notice!),
        if (!loading && rows.isEmpty) const DshEmpty('没有匹配的提醒'),
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
                  '${entry.record.title} · ${entry.active ? '有效' : '已结束'}',
                  style: const TextStyle(fontWeight: FontWeight.w600),
                ),
                const SizedBox(height: 6),
                Text('会话：${_sessionLabel(widget.controller, entry.sessionId)}'),
                Text(_ruleLabel(entry.record)),
                Text(
                  '${entry.active ? '下次计划时间' : '计划时间'}：${entry.record.scheduledAt}',
                ),
                if (entry.lastDelivery != null)
                  Text('最近发送到会话：${entry.lastDelivery!.deliveredAt}'),
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
                      child: const Text('编辑'),
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
                      child: const Text('发送回执'),
                    ),
                    DshButton(
                      key: ValueKey('schedule-delete-${entry.record.id}'),
                      destructive: true,
                      onPressed: disabled ? null : () => remove(entry),
                      child: const Text('删除'),
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
  child: SelectableText(
    text,
    style: TextStyle(fontSize: 13, color: error ? Colors.red : null),
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
        error = '会话或连接已变化，此草稿已停止提交；可复制内容后重新打开。';
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
        throw FormatException('秒数必须为 $minimum 到 9007199254740991 的整数。');
      }
      return {if (update) 'kind': kind, '${kind}_seconds': value};
    }
    if (kind == 'at' && atMode == 'instant') {
      final value = instant.text.trim();
      if (!RegExp(r'(Z|[+-]\d{2}:\d{2})$').hasMatch(value) ||
          DateTime.tryParse(value) == null) {
        throw const FormatException('指定时间须含时区偏移，例如 2030-01-01T09:00:00+08:00。');
      }
      return {if (update) 'kind': kind, 'at': value};
    }
    final tz = zone.text.trim();
    if (tz != 'UTC' &&
        !RegExp(r'^[A-Za-z_+-]+(?:/[A-Za-z0-9_+.-]+)+$').hasMatch(tz)) {
      throw const FormatException(
        '请输入明确的 IANA 时区，例如 Asia/Shanghai、America/New_York 或 UTC；不使用 CST 等缩写。',
      );
    }
    if (kind != 'cron' &&
        !RegExp(r'^([01]\d|2[0-3]):[0-5]\d(?::[0-5]\d(?:\.\d{1,3})?)?$')
            .hasMatch(time.text.trim())) {
      throw const FormatException('时间使用 24 小时制 HH:mm 或 HH:mm:ss，可带最多三位毫秒。');
    }
    final wallTime = time.text.trim().length == 5
        ? '${time.text.trim()}:00'
        : time.text.trim();
    Object value;
    if (kind == 'at') {
      if (!RegExp(r'^\d{4}-\d{2}-\d{2}$').hasMatch(date.text.trim())) {
        throw const FormatException('日期使用 YYYY-MM-DD。');
      }
      value = {'date': date.text.trim(), 'time': wallTime, 'time_zone': tz};
    } else if (kind == 'cron') {
      if (expression.text.trim().split(RegExp(r'\s+')).length != 5) {
        throw const FormatException('Cron 需要五段：分 时 日 月 周。');
      }
      value = {'expression': expression.text.trim(), 'time_zone': tz};
    } else {
      if (kind == 'weekly' && weekdays.isEmpty) {
        throw const FormatException('至少选择一个星期。');
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
        throw const FormatException('请选择绑定会话，并填写标题和发送给会话的提示。');
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
      if (entry == null) throw DshException('schedule_not_found', '提醒不存在');
      if (!entry.active) throw DshException('schedule_ended', '提醒已结束');
      setState(() {
        expected = entry.record;
        conflict = false;
        error = null;
        notice =
            '已读取最新版本，草稿保留，请比较后保存。\n最新标题：${entry.record.title}\n最新提示：${entry.record.prompt}\n最新计划：${entry.record.scheduledAt}';
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
    title: Text(expected == null ? '新建提醒' : '编辑提醒'),
    content: SizedBox(
      width: 600,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (expected == null) ...[
              const Text('绑定会话'),
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
              if (sessions.isEmpty) const Text('请先创建会话，然后再设置提醒。'),
            ] else
              SelectableText(
                '绑定会话：${_sessionLabel(widget.controller, sessionId!)}\n$sessionId\n提醒始终绑定此会话。',
              ),
            input('标题', title, 'schedule-title'),
            input('发送给会话的提示', prompt, 'schedule-prompt', lines: 4),
            if (expected != null)
              CheckboxListTile(
                contentPadding: EdgeInsets.zero,
                key: const ValueKey('schedule-change-timing'),
                title: const Text('修改触发规则'),
                value: changeTiming,
                onChanged: disabled
                    ? null
                    : (value) => setState(() => changeTiming = value == true),
              ),
            const SizedBox(height: 12),
            const Text('触发规则'),
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
            if (expected?.kind == 'after') const Text('延迟提醒若需改期，请指定新的发送时间。'),
            if (kind == 'after' || kind == 'every')
              input(
                kind == 'after' ? '延迟秒数（至少 1 秒）' : '间隔秒数（至少 60 秒）',
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
                  'local': '日期、时间和 IANA 时区',
                  'instant': '含时区偏移的时间',
                },
                onChanged: disabled || !timingEditable
                    ? null
                    : (value) => setState(() => atMode = value),
              ),
              if (atMode == 'instant')
                input(
                  '发送时间（RFC 3339）',
                  instant,
                  'schedule-instant',
                  hint: '2030-01-01T09:00:00+08:00',
                  timingField: true,
                )
              else
                input(
                  '日期',
                  date,
                  'schedule-date',
                  hint: '2030-01-01',
                  timingField: true,
                ),
            ],
            if (['daily', 'weekly'].contains(kind) ||
                (kind == 'at' && atMode == 'local'))
              input(
                '时间（24 小时制）',
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
                      label: Text(
                        '周${const ['一', '二', '三', '四', '五', '六', '日'][day - 1]}',
                      ),
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
                'Cron 表达式（分 时 日 月 周）',
                expression,
                'schedule-cron',
                hint: '0 9 * * *',
                timingField: true,
              ),
              const Text('例如 0 9 * * 1-5 表示工作日 09:00；最小间隔为 1 分钟。'),
            ],
            if (['daily', 'weekly', 'cron'].contains(kind) ||
                (kind == 'at' && atMode == 'local')) ...[
              input(
                'IANA 时区',
                zone,
                'schedule-zone',
                hint: 'Asia/Shanghai',
                timingField: true,
              ),
              const Text(
                '明确使用 Asia/Shanghai、America/New_York 或 UTC；不使用 CST 等歧义缩写。',
              ),
            ],
            if (widget.controller.scheduleEnabled == false)
              _message('提醒已停用，草稿保留；请在目录中手动启用后再保存。'),
            if (error != null) _message(error!, error: true),
            if (notice != null) _message(notice!),
            if (conflict)
              DshButton(
                key: const ValueKey('schedule-refresh-expected'),
                outline: true,
                onPressed: disabled ? null : refreshExpected,
                child: const Text('读取最新版本，保留草稿'),
              ),
          ],
        ),
      ),
    ),
    actions: [
      DshButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('关闭'),
      ),
      DshButton(
        key: const ValueKey('schedule-save'),
        primary: true,
        onPressed: disabled ? null : save,
        child: Text(busy ? '保存中…' : '保存提醒'),
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
          error = '会话或连接已变化，草稿停止提交；请重新打开设置。';
        });
      }
    }
  }

  Json validate(Json value) {
    if (value['entryId'] != widget.entryId ||
        value['revision'] is! String ||
        (value['config'] != null && value['config'] is! Map)) {
      throw StateError('回执配置返回的数据不完整。');
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
          notice =
              '已读取最新配置，草稿保留。最新值：${config['deliveryHistoryDays'] ?? 30} 天 / ${config['deliveryHistoryRecords'] ?? 200} 条；请比较后保存。';
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
        throw const FormatException('保留天数须为 1–3650，条数须为 1–10000，均为整数。');
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
          notice = '保留设置已保存；新投递时应用，不会立即清理已有回执。';
        });
      }
    } catch (e) {
      if (mounted && !stale) {
        setState(() => error = '${scheduleFailure(e)}\n草稿已保留，可读取最新配置后再保存。');
      }
    } finally {
      if (mounted && !stale) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('回执保留设置'),
    content: SizedBox(
      width: 500,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              '默认保留 30 天、最多 200 条。新投递时应用保留策略；查看记录和保存设置不会立即清理回执，也不会启用提醒。',
            ),
            const SizedBox(height: 12),
            const Text('保留天数（1–3650）'),
            DshField(
              key: const ValueKey('schedule-retention-days'),
              controller: days,
              enabled: !busy && !stale,
            ),
            const SizedBox(height: 12),
            const Text('最多条数（1–10000）'),
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
        child: const Text('关闭'),
      ),
      DshButton(
        key: const ValueKey('schedule-retention-reload'),
        onPressed: busy || stale
            ? null
            : () => load(preserve: snapshot != null),
        child: const Text('读取最新配置'),
      ),
      DshButton(
        key: const ValueKey('schedule-retention-save'),
        primary: true,
        onPressed: busy || stale || snapshot == null ? null : save,
        child: const Text('保存保留设置'),
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
          error = '会话或连接已变化，请重新打开回执。';
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
    title: Text('发送回执 · ${widget.entry.record.title}'),
    content: SizedBox(
      width: 620,
      height: 460,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('按发送顺序从新到旧排列；仅表示消息已发送到绑定会话，不代表模型执行成功。'),
          if (page?.retentionDays != null)
            Text(
              '保留范围：${page!.retentionDays} 天，最多 ${page!.retentionRecords} 条。',
            ),
          if (page?.earlierRecordsPruned == true) const Text('更早回执已按保留策略清理。'),
          if (page?.earlierRecordsUnavailable == true) const Text('部分更早回执不可用。'),
          if (error != null) _message(error!, error: true),
          if (loading) const LinearProgressIndicator(),
          Expanded(
            child: ListView(
              children: [
                if (!loading && records.isEmpty && error == null)
                  const DshEmpty('暂无发送回执'),
                for (final record in records)
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 12),
                    child: SelectableText(
                      '已发送到会话：${record.deliveredAt}\n计划时间：${record.scheduledAt}\n消息：${record.messageId}${record.prompt == null ? '' : '\n${record.prompt}'}',
                    ),
                  ),
                if (page?.nextBefore != null)
                  DshButton(
                    key: const ValueKey('schedule-history-more'),
                    onPressed: loading || stale ? null : () => load(more: true),
                    child: const Text('加载更早回执'),
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
        child: const Text('刷新回执'),
      ),
      DshButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('关闭'),
      ),
    ],
  );
}
