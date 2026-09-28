import 'dart:convert';
import 'dart:typed_data';

import 'package:dsh_client/dsh_client.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/text_document.dart';
import '../../src/controller.dart';

class SkillRevisionsPage extends StatefulWidget {
  const SkillRevisionsPage({
    super.key,
    required this.api,
    this.controller,
    this.onClose,
  });
  final DshClient api;
  final DesktopController? controller;
  final VoidCallback? onClose;
  @override
  State<SkillRevisionsPage> createState() => _SkillRevisionsPageState();
}

class _SkillRevisionsPageState extends State<SkillRevisionsPage> {
  RequestScope scope = RequestScope();
  Json? state, detail;
  List<Json> samples = [];
  final selected = <String>{};
  String? error, notice;
  bool busy = false;
  int generation = 0;
  bool get current =>
      widget.controller == null ||
      identical(widget.controller!.client, widget.api);
  @override
  void initState() {
    super.initState();
    widget.controller?.addListener(connectionChanged);
    load();
  }

  void connectionChanged() {
    if (!current && mounted) {
      generation++;
      scope.cancel();
      setState(() {
        busy = false;
        error = '连接已变化，请关闭后重新打开技能版本。';
      });
    }
  }

  @override
  void didUpdateWidget(SkillRevisionsPage old) {
    super.didUpdateWidget(old);
    if (old.controller != widget.controller) {
      old.controller?.removeListener(connectionChanged);
      widget.controller?.addListener(connectionChanged);
    }
    if (!identical(old.api, widget.api)) {
      generation++;
      scope.cancel();
      scope = RequestScope();
      state = null;
      detail = null;
      samples = [];
      selected.clear();
      busy = false;
      load();
    }
  }

  @override
  void dispose() {
    generation++;
    widget.controller?.removeListener(connectionChanged);
    scope.cancel();
    super.dispose();
  }

  Future<Json> call(
    String action, [
    Json payload = const {},
    bool mutation = false,
  ]) async => object(
    await widget.api.callValue(
      'capabilities.skillRevision$action',
      payload: payload,
      mutation: mutation,
      scope: scope,
    ),
  );
  Future<void> load() async {
    if (busy || !current) return;
    final token = ++generation;
    setState(() => busy = true);
    try {
      final value = await call('List');
      if (mounted && current && token == generation) {
        setState(() {
          state = value;
          error = null;
        });
      }
    } catch (e) {
      if (mounted && current && token == generation) {
        setState(() => error = '$e');
      }
    } finally {
      if (mounted && token == generation) setState(() => busy = false);
    }
  }

  Future<void> inspect(String id) async {
    if (busy || !current) return;
    final token = ++generation;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final candidate = await call('Read', {'id': id});
      final value = await widget.api.request(
        '/__dsh-task-execution',
        body: {
          'action': 'list',
          'sessionId': candidate['ownerSessionId'],
          'summaryOnly': true,
        },
        scope: scope,
      );
      final tasks = objects(value['tasks']).where((task) {
        final subject = object(object(task['spec'])['validationSubject']);
        return task['state'] == 'completed' &&
            subject['kind'] == 'skill' &&
            subject['identity'] == candidate['contentHash'];
      }).toList();
      if (mounted && current && token == generation) {
        setState(() {
          detail = candidate;
          samples = tasks;
          selected
            ..clear()
            ..addAll(
              objects(candidate['samples']).map((s) => '${s['taskId']}'),
            );
        });
      }
    } catch (e) {
      if (mounted && current && token == generation) {
        setState(() => error = '$e');
      }
    } finally {
      if (mounted && token == generation) setState(() => busy = false);
    }
  }

  Future<void> perform(String action, Json fields) async {
    if (busy || !current || state == null) return;
    final token = ++generation;
    setState(() {
      busy = true;
      error = null;
      notice = null;
    });
    try {
      final value = await call(action, {
        'expectedRevision': state!['revision'],
        ...fields,
      }, true);
      if (mounted && current && token == generation) {
        setState(() {
          state = value['state'] is Map ? object(value['state']) : value;
          notice = '操作已保存';
          if (action == 'Validate' && detail != null) {
            detail = {...detail!, 'validation': value['evidence']};
          }
          if (['Activate', 'Withdraw', 'Restore'].contains(action)) {
            detail = null;
          }
        });
      }
    } catch (e) {
      if (mounted && current && token == generation) {
        setState(() => error = '$e');
        try {
          final value = await call('List');
          if (mounted && current && token == generation) {
            setState(() => state = value);
          }
        } catch (_) {}
      }
    } finally {
      if (mounted && token == generation) setState(() => busy = false);
    }
  }

  List<Json> get references => samples
      .where((task) => selected.contains(task['taskId']))
      .map(
        (task) => {
          'taskId': task['taskId'],
          'revision': task['revision'],
          'expectedSuccess':
              object(
                object(task['spec'])['validationSubject'],
              )['expectedOutcome'] ==
              'success',
        },
      )
      .toList();
  Future<void> saveText() async {
    final record = detail;
    if (record == null) return;
    final target = await getSaveLocation(suggestedName: '${record['name']}.md');
    if (target == null) return;
    try {
      await XFile.fromData(
        Uint8List.fromList(utf8.encode('${record['content'] ?? ''}')),
        mimeType: 'text/markdown',
      ).saveTo(target.path);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    }
  }

  String label(Json candidate) => candidate['withdrawn'] == true
      ? '已撤回'
      : candidate['active'] == true
      ? '已启用，使用前复核适用性'
      : candidate['validation'] != null
      ? '有验证记录，尚未启用'
      : '待验证';
  @override
  Widget build(BuildContext context) {
    final candidates = objects(state?['candidates']);
    final refs = references,
        canValidate =
            refs.any((s) => s['expectedSuccess'] == true) &&
            refs.any((s) => s['expectedSuccess'] == false);
    final text = '${detail?['content'] ?? ''}';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            const Expanded(
              child: Text(
                '技能版本与验证',
                style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
              ),
            ),
            if (widget.onClose != null)
              DshIcon(
                LucideIcons.x,
                label: '关闭技能版本',
                onPressed: widget.onClose,
              ),
          ],
        ),
        const SizedBox(height: 10),
        const Text('候选版本通过正向与反向样本验证后启用，限定对应项目和运行环境；撤回与恢复保留历史。'),
        if (busy) const LinearProgressIndicator(minHeight: 2),
        if (error != null)
          Text(error!, style: const TextStyle(color: Colors.red)),
        if (notice != null)
          Text(notice!, style: const TextStyle(color: Colors.green)),
        Expanded(
          child: ListView(
            children: [
              if (state != null)
                SwitchListTile(
                  contentPadding: EdgeInsets.zero,
                  title: const Text('启用经过验证的技能版本'),
                  value: state!['enabled'] == true,
                  onChanged: busy || !current
                      ? null
                      : (enabled) => perform('Toggle', {'enabled': enabled}),
                ),
              Align(
                alignment: Alignment.centerLeft,
                child: DshButton(
                  onPressed: busy || !current ? null : load,
                  child: const Text('刷新版本'),
                ),
              ),
              if (state != null && candidates.isEmpty)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 14),
                  child: Text('暂无候选版本。可在任务会话中创建技能候选，再建立该版本的正向、反向验收任务。'),
                ),
              for (final candidate in candidates)
                Card(
                  child: Padding(
                    padding: const EdgeInsets.all(12),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '${candidate['name']}',
                          style: const TextStyle(fontWeight: FontWeight.w600),
                        ),
                        Text('${candidate['description'] ?? ''}'),
                        Text(
                          '${candidate['project'] ?? ''}',
                          style: TextStyle(
                            fontSize: 12,
                            color: DshColors(context).muted,
                          ),
                        ),
                        Text(label(candidate)),
                        const SizedBox(height: 8),
                        Wrap(
                          spacing: 8,
                          runSpacing: 8,
                          children: [
                            DshButton(
                              onPressed: busy || !current
                                  ? null
                                  : () => inspect('${candidate['id']}'),
                              child: const Text('查看与验证'),
                            ),
                            DshButton(
                              onPressed:
                                  busy ||
                                      !current ||
                                      state?['enabled'] != true ||
                                      candidate['validation'] == null ||
                                      candidate['active'] == true ||
                                      candidate['withdrawn'] == true
                                  ? null
                                  : () => perform('Activate', {
                                      'id': candidate['id'],
                                    }),
                              child: const Text('验证并启用'),
                            ),
                            if (candidate['withdrawn'] == true)
                              DshButton(
                                onPressed:
                                    busy ||
                                        !current ||
                                        state?['enabled'] != true ||
                                        candidate['validation'] == null
                                    ? null
                                    : () => perform('Restore', {
                                        'id': candidate['id'],
                                      }),
                                child: const Text('复核并恢复此版本'),
                              )
                            else
                              DshButton(
                                onPressed: busy || !current
                                    ? null
                                    : () => perform('Withdraw', {
                                        'id': candidate['id'],
                                      }),
                                child: const Text('撤回版本'),
                              ),
                          ],
                        ),
                      ],
                    ),
                  ),
                ),
              if (detail != null)
                Card(
                  child: Padding(
                    padding: const EdgeInsets.all(12),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '${detail!['name']}',
                          style: const TextStyle(fontWeight: FontWeight.w600),
                        ),
                        const Text('至少选择一个正向和一个反向的已验收任务。'),
                        ExpansionTile(
                          tilePadding: EdgeInsets.zero,
                          title: const Text('技能正文'),
                          children: [
                            SizedBox(
                              height: 240,
                              child: SingleChildScrollView(
                                child: SelectableText(
                                  text.length > 64000
                                      ? text.substring(
                                          0,
                                          TextDocument.boundary(text, 64000),
                                        )
                                      : text,
                                ),
                              ),
                            ),
                            if (text.length > 64000)
                              const Text('正文较长，显示前 64000 个字符。'),
                            DshButton(
                              onPressed: saveText,
                              child: const Text('保存完整正文'),
                            ),
                          ],
                        ),
                        if (samples.isEmpty) const Text('尚无符合条件的验收任务。'),
                        for (final task in samples)
                          CheckboxListTile(
                            contentPadding: EdgeInsets.zero,
                            controlAffinity: ListTileControlAffinity.leading,
                            title: Text(
                              '${object(object(task['spec'])['validationSubject'])['expectedOutcome'] == 'success' ? '正向' : '反向'}：${object(task['spec'])['objective']}',
                            ),
                            value: selected.contains(task['taskId']),
                            onChanged: busy || !current
                                ? null
                                : (checked) => setState(() {
                                    if (checked == true) {
                                      selected.add('${task['taskId']}');
                                    } else {
                                      selected.remove(task['taskId']);
                                    }
                                  }),
                          ),
                        Wrap(
                          spacing: 8,
                          children: [
                            DshButton(
                              onPressed:
                                  busy ||
                                      !current ||
                                      detail!['withdrawn'] == true ||
                                      !canValidate
                                  ? null
                                  : () => perform('Validate', {
                                      'id': detail!['id'],
                                      'samples': refs,
                                    }),
                              child: const Text('核验所选样本'),
                            ),
                            DshButton(
                              onPressed: busy
                                  ? null
                                  : () => setState(() => detail = null),
                              child: const Text('关闭详情'),
                            ),
                          ],
                        ),
                      ],
                    ),
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }
}
