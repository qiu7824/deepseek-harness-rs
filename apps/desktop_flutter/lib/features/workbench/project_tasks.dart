import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';

const taskStates = {
  'todo': '待办',
  'in-progress': '进行中',
  'done': '已完成',
  'update': '需更新',
  'feedback': '反馈',
};

class ProjectTasks extends StatefulWidget {
  const ProjectTasks({super.key, required this.api, required this.session});
  final DshClient api;
  final String session;
  @override
  State<ProjectTasks> createState() => _ProjectTasksState();
}

class _ProjectTasksState extends State<ProjectTasks> {
  final scope = RequestScope(), search = TextEditingController();
  Json? board;
  String filter = 'all';
  String? error;
  bool busy = true;

  @override
  void initState() {
    super.initState();
    load();
  }

  @override
  void dispose() {
    scope.cancel();
    search.dispose();
    super.dispose();
  }

  Future<void> load() async {
    setState(() => busy = true);
    try {
      final value = await widget.api.request(
        '/__dsh-productivity/tasks/list',
        body: {'sessionId': widget.session},
        scope: scope,
        maxBytes: 2 * 1024 * 1024,
      );
      if (mounted) {
        setState(() {
          board = value;
          error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> edit([Json? task]) async {
    if (board == null) return;
    final revision = board!['revision'];
    final original = objects(board!['tasks']);
    final changed = await showDialog<Json>(
      context: context,
      builder: (_) => TaskEditor(
        initial: task,
        onSave: (value) async {
          final tasks = [...original];
          final index = tasks.indexWhere((row) => row['id'] == value['id']);
          if (index < 0) {
            tasks.add(value);
          } else {
            tasks[index] = value;
          }
          return widget.api.request(
            '/__dsh-productivity/tasks/save',
            body: {
              'sessionId': widget.session,
              'revision': revision,
              'tasks': tasks,
            },
            mutation: true,
            scope: scope,
            maxBytes: 2 * 1024 * 1024,
          );
        },
      ),
    );
    if (changed != null && mounted) {
      setState(() {
        board = changed;
        error = null;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final rows =
        objects(board?['tasks'])
            .where(
              (row) =>
                  (filter == 'all' || row['status'] == filter) &&
                  '${row['title']} ${row['detail']}'.toLowerCase().contains(
                    search.text.toLowerCase(),
                  ),
            )
            .toList()
          ..sort(
            (a, b) => ((a['priority'] as num?) ?? 3).compareTo(
              (b['priority'] as num?) ?? 3,
            ),
          );
    return Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  '项目任务',
                  style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
                ),
              ),
              DshIcon(
                LucideIcons.refreshCw,
                label: '刷新项目任务',
                onPressed: busy ? null : load,
              ),
              DshButton(
                outline: true,
                height: 32,
                fontSize: 13,
                onPressed: busy || board == null ? null : edit,
                child: const Text('新增任务'),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Text(
            '工作区共用一份 Markdown 清单，其他对话和 AI 的修改会在刷新后显示。',
            style: TextStyle(fontSize: 12, height: 1.6, color: colors.muted),
          ),
          const SizedBox(height: 12),
          Row(
            children: [
              Expanded(
                child: DshField(
                  controller: search,
                  prefix: LucideIcons.search,
                  hint: '搜索任务',
                  onChanged: (_) => setState(() {}),
                ),
              ),
              const SizedBox(width: 8),
              DshSelect<String>(
                options: const {'all': '全部', ...taskStates},
                value: filter,
                onChanged: (v) => setState(() => filter = v),
              ),
            ],
          ),
          if (error != null)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 12),
              child: Text(
                error!,
                style: const TextStyle(color: Colors.red, fontSize: 12),
              ),
            ),
          if (busy) const LinearProgressIndicator(minHeight: 2),
          const SizedBox(height: 12),
          Expanded(
            child: rows.isEmpty
                ? const DshEmpty('还没有项目任务')
                : ListView.builder(
                    itemCount: rows.length,
                    itemBuilder: (context, index) {
                      final row = rows[index];
                      return Container(
                        margin: const EdgeInsets.only(bottom: 12),
                        padding: const EdgeInsets.all(14),
                        decoration: BoxDecoration(
                          border: Border.all(color: colors.border),
                          borderRadius: BorderRadius.circular(12),
                        ),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Row(
                              children: [
                                Expanded(
                                  child: Text(
                                    '${row['title']}',
                                    style: const TextStyle(
                                      fontSize: 14,
                                      fontWeight: FontWeight.w500,
                                    ),
                                  ),
                                ),
                                DshButton(
                                  height: 28,
                                  fontSize: 12,
                                  onPressed: busy ? null : () => edit(row),
                                  child: const Text('编辑'),
                                ),
                              ],
                            ),
                            Wrap(
                              spacing: 8,
                              children: [
                                Text(
                                  'P${row['priority']}',
                                  style: TextStyle(
                                    fontSize: 12,
                                    color: colors.blue,
                                  ),
                                ),
                                Text(
                                  taskStates[row['status']] ??
                                      '${row['status']}',
                                  style: TextStyle(
                                    fontSize: 12,
                                    color: colors.muted,
                                  ),
                                ),
                              ],
                            ),
                            if ('${row['detail'] ?? ''}'.isNotEmpty)
                              Padding(
                                padding: const EdgeInsets.only(top: 8),
                                child: Text(
                                  '${row['detail']}',
                                  style: const TextStyle(
                                    fontSize: 13,
                                    height: 1.6,
                                  ),
                                ),
                              ),
                          ],
                        ),
                      );
                    },
                  ),
          ),
        ],
      ),
    );
  }
}

class TaskEditor extends StatefulWidget {
  const TaskEditor({super.key, this.initial, required this.onSave});
  final Json? initial;
  final Future<Json> Function(Json) onSave;
  @override
  State<TaskEditor> createState() => _TaskEditorState();
}

class _TaskEditorState extends State<TaskEditor> {
  late final title = TextEditingController(
    text: widget.initial?['title'] as String? ?? '',
  );
  late final detail = TextEditingController(
    text: widget.initial?['detail'] as String? ?? '',
  );
  late String status = widget.initial?['status'] as String? ?? 'todo';
  late int priority = (widget.initial?['priority'] as num?)?.toInt() ?? 2;
  late final String id = widget.initial?['id'] as String? ?? newRequestId();
  bool busy = false;
  String? error;
  @override
  void dispose() {
    title.dispose();
    detail.dispose();
    super.dispose();
  }

  Future<void> save() async {
    if (title.text.trim().isEmpty ||
        title.text.runes.length > 300 ||
        detail.text.runes.length > 20000) {
      setState(() => error = '请填写任务标题（最多 300 字）；说明最多 20000 字。');
      return;
    }
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final result = await widget.onSave({
        'id': id,
        'title': title.text.trim(),
        'detail': detail.text,
        'status': status,
        'priority': priority,
      });
      if (mounted) Navigator.pop(context, result);
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
      title: Text(
        widget.initial == null ? '新增任务' : '编辑任务',
        style: const TextStyle(fontSize: 17),
      ),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('任务标题', style: TextStyle(fontSize: 13)),
              const SizedBox(height: 6),
              DshField(controller: title, autofocus: true),
              const SizedBox(height: 16),
              Wrap(
                spacing: 16,
                runSpacing: 10,
                children: [
                  DshSelect<String>(
                    options: taskStates,
                    value: status,
                    onChanged: busy ? null : (v) => setState(() => status = v),
                  ),
                  DshSelect<int>(
                    options: const {
                      0: 'P0 · 紧急',
                      1: 'P1 · 高',
                      2: 'P2 · 中',
                      3: 'P3 · 低',
                    },
                    value: priority,
                    onChanged: busy
                        ? null
                        : (v) => setState(() => priority = v),
                  ),
                ],
              ),
              const SizedBox(height: 16),
              const Text('说明与反馈', style: TextStyle(fontSize: 13)),
              const SizedBox(height: 6),
              DshField(controller: detail, maxLines: 6),
              if (error != null)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: SelectableText(
                    error!,
                    style: const TextStyle(color: Colors.red, fontSize: 12),
                  ),
                ),
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
          primary: true,
          pill: true,
          onPressed: busy ? null : save,
          child: Text(busy ? '保存中…' : '保存'),
        ),
      ],
    ),
  );
}
