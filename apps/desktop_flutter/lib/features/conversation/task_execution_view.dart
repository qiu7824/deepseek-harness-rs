import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

const taskStateLabels = {
  'planned': '待执行',
  'running': '执行中',
  'validating': '验收中',
  'validation_failed': '验收未通过',
  'awaiting_user': '待人工验收',
  'completed': '已验收完成',
  'cancelled': '已取消',
  'blocked': '待恢复核实',
  'prepared': '已保存执行意图',
  'dispatched': '已分派',
  'effect_observed': '已观察到效果',
  'verified': '执行结果已核实',
  'committed': '已交付',
  'failed': '未通过',
  'unknown': '效果未知',
  'passed': '通过',
  'unverified': '尚未验证',
  'not_dispatched': '请求操作未启动',
};

String taskBlockerText(String message, Json task) {
  const known = {
    'Current inputs have not been validated': '当前文件与输入尚未验收。',
    'Manual acceptance required': '需要人工确认。',
    'Task was cancelled; only an explicit user continuation may resume it.':
        '任务已取消，需要明确继续此任务后才能恢复。',
    'Refreshed evidence does not match the completed input identities':
        '复核证据与原完成版本不一致，请检查文件版本后重新验收。',
    'Current goal requirements are unavailable; acceptance reuse is blocked until they can be read':
        '暂时无法读取当前目标，恢复读取后才能核对验收证据。',
    'Goal requirements changed or the linked goal is no longer current; old acceptance does not satisfy the current goal':
        '目标已变化，请核对当前要求并重新验收。',
  };
  if (known.containsKey(message)) return known[message]!;
  String checkName(String id) =>
      objects(object(task['spec'])['acceptanceChecks'])
              .where((check) => check['id'] == id)
              .firstOrNull?['description']
          as String? ??
      id;
  final acceptance = RegExp(r'^Acceptance (.+) has not passed$')
      .firstMatch(message);
  if (acceptance != null) return '验收项“${checkName(acceptance[1]!)}”尚未通过。';
  final output = RegExp(r'^Output (.+) has no verified identity$')
      .firstMatch(message);
  if (output != null) return '产物“${output[1]}”尚无已核验版本。';
  return message;
}

class TaskExecutionView extends StatefulWidget {
  const TaskExecutionView({
    super.key,
    required this.api,
    required this.session,
    this.controller,
  });
  final DshClient api;
  final String session;
  final DesktopController? controller;
  @override
  State<TaskExecutionView> createState() => _TaskExecutionViewState();
}

class _TaskExecutionViewState extends State<TaskExecutionView> {
  RequestScope scope = RequestScope();
  List<Json> tasks = [];
  Json? detail, pending;
  String selected = '';
  String? error, notice;
  bool busy = false, stopping = false;
  int generation = 0, operation = 0;
  final effectChecks = <String, String>{};
  bool get current =>
      widget.controller == null ||
      (identical(widget.controller!.client, widget.api) &&
          widget.controller!.selectedId == widget.session);
  bool get locked => busy || stopping || !current;
  Json get task => object(detail?['task']);
  bool get terminal => ['completed', 'cancelled'].contains(task['state']);

  @override
  void initState() {
    super.initState();
    widget.controller?.addListener(connectionChanged);
    refresh();
  }

  void connectionChanged() {
    if (!current && mounted) {
      generation++;
      operation++;
      scope.cancel();
      setState(() {
        busy = false;
        stopping = false;
        error = '会话或连接已变化，请重新打开任务验收。';
      });
    }
  }

  @override
  void didUpdateWidget(TaskExecutionView old) {
    super.didUpdateWidget(old);
    if (old.controller != widget.controller) {
      old.controller?.removeListener(connectionChanged);
      widget.controller?.addListener(connectionChanged);
    }
    if (old.session != widget.session || !identical(old.api, widget.api)) {
      generation++;
      operation++;
      scope.cancel();
      scope = RequestScope();
      tasks = [];
      detail = null;
      pending = null;
      selected = '';
      error = null;
      notice = null;
      busy = false;
      stopping = false;
      effectChecks.clear();
      refresh();
    }
  }

  @override
  void dispose() {
    generation++;
    operation++;
    scope.cancel();
    widget.controller?.removeListener(connectionChanged);
    super.dispose();
  }

  Future<Json> call(Json body, {bool mutation = false}) => widget.api.request(
    '/__dsh-task-execution',
    body: {'sessionId': widget.session, ...body},
    scope: scope,
    mutation: mutation,
  );
  Future<void> load([String id = '']) async {
    final token = ++generation;
    final list = await call({'action': 'list', 'summaryOnly': true});
    final rows = objects(list['tasks']);
    final choice = rows.any((row) => row['taskId'] == id)
        ? id
        : rows.firstOrNull?['taskId'] as String? ?? '';
    final value = choice.isEmpty
        ? null
        : await call({'action': 'get', 'taskId': choice});
    if (mounted && current && token == generation) {
      setState(() {
        tasks = rows;
        selected = choice;
        detail = value;
      });
    }
  }

  Future<void> refresh([String? id]) async {
    if (locked) return;
    final token = ++operation;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await load(id ?? selected);
    } catch (e) {
      if (mounted && current && token == operation) {
        setState(() => error = '$e');
      }
    } finally {
      if (mounted && token == operation) setState(() => busy = false);
    }
  }

  Future<void> perform(Json payload) async {
    if (locked) return;
    final token = ++operation;
    setState(() {
      busy = true;
      pending = payload;
      error = null;
      notice = null;
    });
    try {
      await call(payload, mutation: true);
      if (!mounted || !current || token != operation) return;
      setState(() {
        pending = null;
        notice = '操作已完成，正在核对最新记录。';
      });
      await load(payload['taskId'] as String);
      if (mounted && current && token == operation) {
        setState(() => notice = '验收记录已更新。');
      }
    } catch (e) {
      if (mounted && current && token == operation) {
        setState(() {
          if (e is DshException && e.code == 'CANCELLED') {
            pending = null;
            notice = '验收已停止。';
          } else {
            error = '$e';
          }
        });
      }
    } finally {
      if (mounted && token == operation) setState(() => busy = false);
    }
  }

  Future<void> act(String action, [Json extra = const {}]) => perform({
    'action': action,
    'taskId': task['taskId'],
    'revision': task['revision'],
    'idempotencyKey': newRequestId(),
    ...extra,
  });
  Future<void> stop() async {
    if (stopping || pending == null || !current) return;
    final token = operation, id = pending!['taskId'] as String;
    setState(() {
      stopping = true;
      error = null;
    });
    try {
      await call({
        'action': 'stop_validation',
        'taskId': id,
        'idempotencyKey': newRequestId(),
      }, mutation: true);
      if (!mounted || !current || token != operation) return;
      operation++;
      setState(() {
        pending = null;
        busy = false;
        notice = '停止请求已处理。';
      });
      await load(id);
    } catch (e) {
      if (mounted && current) setState(() => error = '$e');
    } finally {
      if (mounted && current) setState(() => stopping = false);
    }
  }

  Future<void> confirm(
    String action,
    String message, [
    Json extra = const {},
  ]) async {
    if (locked) return;
    final original = task, owner = widget.session, api = widget.api;
    final accepted = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('核对任务状态'),
        content: Text(message),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('返回'),
          ),
          TextButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text(action == 'confirm' ? '确认验收通过' : '确认'),
          ),
        ],
      ),
    );
    if (accepted != true ||
        !mounted ||
        !current ||
        locked ||
        widget.session != owner ||
        !identical(api, widget.api) ||
        task['taskId'] != original['taskId'] ||
        task['revision'] != original['revision']) {
      return;
    }
    await act(action, extra);
  }

  Widget action(String label, VoidCallback onPressed, {bool enabled = true}) =>
      DshButton(
        onPressed: locked || !enabled ? null : onPressed,
        child: Text(label),
      );
  Widget card(String title, List<Widget> children) => Card(
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(title, style: const TextStyle(fontWeight: FontWeight.w600)),
          const SizedBox(height: 10),
          ...children,
        ],
      ),
    ),
  );
  String label(dynamic value) => taskStateLabels[value] ?? '$value';
  @override
  Widget build(BuildContext context) {
    final spec = object(task['spec']),
        checks = objects(spec['acceptanceChecks']);
    final results = objects(
      object(task['acceptanceRefresh'])['results'] ?? task['acceptanceResults'],
    );
    final contentChecks = checks
        .where((check) => object(check['checker'])['path'] is String)
        .toList();
    final blockers = (detail?['blockers'] as List? ?? [])
        .map((e) => '$e')
        .toList();
    return ListView(
      padding: const EdgeInsets.all(20),
      children: [
        Wrap(
          spacing: 12,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            const Text('任务验收与恢复', style: TextStyle(fontSize: 18)),
            action('刷新', () => refresh()),
          ],
        ),
        const SizedBox(height: 12),
        const Text('验收记录与文件版本绑定。效果未知的步骤须先核实；恢复操作不会重放命令。'),
        if (error != null)
          SelectableText(error!, style: const TextStyle(color: Colors.red)),
        if (notice != null) Text(notice!),
        if (pending != null && error != null)
          action(
            '使用原操作标识核实重试',
            () => perform(Map<String, dynamic>.from(pending!)),
          ),
        if (busy || stopping) const LinearProgressIndicator(),
        if (busy &&
            [
              'validate',
              'refresh_evidence',
              'reconcile',
            ].contains(pending?['action']))
          DshButton(
            onPressed: stopping || !current ? null : stop,
            child: const Text('停止验收'),
          ),
        if (!busy && tasks.isEmpty) const Text('当前会话尚无验收契约；多步骤工作建立契约后会在这里显示。'),
        if (tasks.isNotEmpty)
          DropdownButtonFormField<String>(
            key: ValueKey(selected),
            initialValue: selected,
            isExpanded: true,
            decoration: const InputDecoration(labelText: '任务'),
            items: [
              for (final row in tasks)
                DropdownMenuItem(
                  value: row['taskId'] as String,
                  child: Text(
                    '${label(row['state'])} · ${object(row['spec'])['objective']}',
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
            ],
            onChanged: locked
                ? null
                : (id) {
                    pending = null;
                    effectChecks.clear();
                    refresh(id);
                  },
          ),
        if (task.isNotEmpty) ...[
          card('${spec['objective']}', [
            Text(
              task['state'] == 'completed' && blockers.isNotEmpty
                  ? '历史已完成 · 当前证据需复核'
                  : label(task['state']),
            ),
            Text(
              '任务 ${task['taskId']} · 版本 ${task['revision']} · 要求版本 ${task['requirementsRevision'] ?? 1}',
            ),
            for (final constraint in spec['constraints'] as List? ?? [])
              Text('• $constraint'),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                if (task['state'] == 'completed')
                  action('刷新验收证据', () => act('refresh_evidence')),
                if (!terminal) action('重新验收', () => act('validate')),
                if (!terminal)
                  action(
                    '切换到当前环境',
                    () => confirm(
                      'migrate_environment',
                      '切换后保留要求和未知效果，旧验收证据失效，必须重新验收。',
                    ),
                  ),
                if (task['state'] == 'validating')
                  action(
                    '核对版本并完成',
                    () => act('complete'),
                    enabled: blockers.isEmpty,
                  ),
                if (task['state'] == 'cancelled')
                  action('继续此任务', () => act('resume')),
                if (!terminal)
                  action(
                    '取消任务',
                    () => confirm('cancel', '取消后保留执行记录；已经发生的操作不会回滚。'),
                  ),
              ],
            ),
          ]),
          if (blockers.isNotEmpty)
            card('尚未满足的完成条件', [
              for (final blocker in blockers)
                SelectableText('• ${taskBlockerText(blocker, task)}'),
            ]),
          if (task['acceptanceRefresh'] != null)
            const Text('以下为当前复核结果；原完成记录和人工确认保持不变。'),
          for (final check in checks)
            Builder(
              builder: (context) {
                final result = results
                    .where((row) => row['checkId'] == check['id'])
                    .firstOrNull;
                final checker = object(check['checker']);
                return card('${check['description']}', [
                  Text(result == null ? '尚未验证' : label(result['status'])),
                  if (checker['path'] != null)
                    SelectableText('${checker['path']}'),
                  if (result?['coverage'] != null)
                    Text('${result!['coverage']}'),
                  if (result?['failureReason'] != null)
                    SelectableText('${result!['failureReason']}'),
                  if (result?['inputIdentity'] != null)
                    SelectableText('输入标识：${result!['inputIdentity']}'),
                  if (checker['kind'] == 'manual' &&
                      result?['status'] == 'awaiting_user' &&
                      !terminal)
                    action(
                      '核对并确认此项',
                      () => confirm(
                        'confirm',
                        '确认已检查此输入版本，且“${check['description']}”要求已满足。',
                        {
                          'checkId': check['id'],
                          'inputIdentity': result!['inputIdentity'],
                        },
                      ),
                    ),
                  if ((result?['evidenceRefs'] as List? ?? []).isNotEmpty)
                    ExpansionTile(
                      title: const Text('验收证据'),
                      children: [
                        for (final ref in result!['evidenceRefs'] as List)
                          SelectableText('$ref'),
                      ],
                    ),
                ]);
              },
            ),
          card('执行与恢复记录', [
            if (objects(task['steps']).isEmpty) const Text('尚未分派执行步骤。'),
            for (final step in objects(task['steps']))
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 10),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('${step['tool']} · ${label(step['state'])}'),
                    SelectableText('${step['executionId']}'),
                    if (step['failureReason'] != null)
                      SelectableText('${step['failureReason']}'),
                    for (final recovery in objects(
                      detail?['recovery'],
                    ).where((r) => r['stepId'] == step['id']))
                      Text('${recovery['reason']}'),
                    if ([
                          'unknown',
                          'failed',
                          'running',
                          'effect_observed',
                        ].contains(step['state']) &&
                        !terminal) ...[
                      const Text('选择能够证明该步骤效果的文件验收项；外部请求需单独核实。'),
                      DropdownButtonFormField<String>(
                        key: ValueKey(
                          '${step['id']}/${effectChecks[step['id']]}',
                        ),
                        initialValue: effectChecks[step['id']] ?? '',
                        isExpanded: true,
                        items: [
                          const DropdownMenuItem(
                            value: '',
                            child: Text('选择验收项'),
                          ),
                          for (final check in contentChecks)
                            DropdownMenuItem(
                              value: check['id'] as String,
                              child: Text(
                                '${check['description']}',
                                overflow: TextOverflow.ellipsis,
                              ),
                            ),
                        ],
                        onChanged: locked
                            ? null
                            : (id) => setState(
                                () => effectChecks[step['id'] as String] =
                                    id ?? '',
                              ),
                      ),
                      action(
                        '检查文件并确认步骤效果',
                        () => act('reconcile', {
                          'stepId': step['id'],
                          'checkId': effectChecks[step['id']],
                        }),
                        enabled: (effectChecks[step['id']] ?? '').isNotEmpty,
                      ),
                    ],
                  ],
                ),
              ),
          ]),
          if ((spec['expectedOutputs'] as List? ?? []).isNotEmpty)
            card('预期产物', [
              for (final path in spec['expectedOutputs'] as List)
                SelectableText(
                  '$path · ${object(task['outputIdentities'])[path] ?? '尚无验收版本'}',
                ),
            ]),
        ],
      ],
    );
  }
}
