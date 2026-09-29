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
  Map<String, TextEditingController>? editor;
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
      disposeEditor();
      busy = false;
      load();
    }
  }

  @override
  void dispose() {
    generation++;
    widget.controller?.removeListener(connectionChanged);
    scope.cancel();
    disposeEditor();
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
      if (mounted && current && token == generation) {
        setState(() {
          detail = candidate;
          disposeEditor();
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
          notice = action == 'Create' ? '版本已保存，尚未启用' : '操作已保存';
          if (action == 'Create') disposeEditor();
          if (['Activate', 'Withdraw', 'Restore', 'Create'].contains(action)) {
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

  void disposeEditor() {
    for (final controller in editor?.values ?? <TextEditingController>[]) {
      controller.dispose();
    }
    editor = null;
  }

  void editRevision(Json? record) {
    if (busy || !current) return;
    setState(() {
      disposeEditor();
      editor = {
        for (final field in ['name', 'description', 'project', 'content'])
          field: TextEditingController(
            text:
                '${record?[field] ?? (field == 'project' ? widget.controller?.selected?.cwd : null) ?? ''}',
          ),
      };
      detail = null;
      error = null;
    });
  }

  Future<void> saveRevision() async {
    final fields = editor;
    if (fields == null) return;
    if (fields.values.any((controller) => controller.text.trim().isEmpty)) {
      setState(() => error = '请填写名称、说明、项目目录和技能正文。');
      return;
    }
    await perform('Create', {
      for (final field in fields.entries) field.key: field.value.text,
    });
  }

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
      ? '已启用'
      : '未启用';
  @override
  Widget build(BuildContext context) {
    final candidates = objects(state?['candidates']);
    final text = '${detail?['content'] ?? ''}';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            const Expanded(
              child: Text(
                '技能版本',
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
        const Text('手动选择对应项目使用的技能版本。编辑会保存新版本，启用、撤回和恢复均由你选择。'),
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
                  title: const Text('启用项目技能版本'),
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
              Align(
                alignment: Alignment.centerLeft,
                child: DshButton(
                  onPressed: busy || !current || state?['enabled'] != true
                      ? null
                      : () => editRevision(null),
                  child: const Text('创建技能版本'),
                ),
              ),
              if (state != null && candidates.isEmpty)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 14),
                  child: Text('暂无技能版本。'),
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
                              child: const Text('查看版本'),
                            ),
                            DshButton(
                              onPressed:
                                  busy ||
                                      !current ||
                                      state?['enabled'] != true ||
                                      candidate['active'] == true ||
                                      candidate['withdrawn'] == true
                                  ? null
                                  : () => perform('Activate', {
                                      'id': candidate['id'],
                                    }),
                              child: const Text('启用版本'),
                            ),
                            if (candidate['withdrawn'] == true)
                              DshButton(
                                onPressed:
                                    busy ||
                                        !current ||
                                        state?['enabled'] != true
                                    ? null
                                    : () => perform('Restore', {
                                        'id': candidate['id'],
                                      }),
                                child: const Text('恢复版本'),
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
              if (editor != null)
                Card(
                  child: Padding(
                    padding: const EdgeInsets.all(12),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        const Text(
                          '保存技能版本',
                          style: TextStyle(fontWeight: FontWeight.w600),
                        ),
                        const Text('同名技能的旧版本仍会保留，保存后需要单独启用。'),
                        for (final field in const {
                          'name': '技能名称',
                          'description': '说明',
                          'project': '项目目录',
                          'content': '技能正文',
                        }.entries)
                          Padding(
                            padding: const EdgeInsets.only(top: 10),
                            child: TextField(
                              controller: editor![field.key],
                              enabled: !busy && current,
                              decoration: InputDecoration(
                                labelText: field.value,
                              ),
                              minLines: field.key == 'content' ? 8 : 1,
                              maxLines: field.key == 'content' ? 16 : 1,
                              maxLength: {
                                'name': 80,
                                'description': 1024,
                                'project': 8192,
                                'content': 262144,
                              }[field.key],
                            ),
                          ),
                        Wrap(
                          spacing: 8,
                          children: [
                            DshButton(
                              onPressed:
                                  busy || !current || state?['enabled'] != true
                                  ? null
                                  : saveRevision,
                              child: const Text('保存新版本'),
                            ),
                            DshButton(
                              onPressed: busy
                                  ? null
                                  : () => setState(disposeEditor),
                              child: const Text('取消编辑'),
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
                        Wrap(
                          spacing: 8,
                          children: [
                            DshButton(
                              onPressed:
                                  busy || !current || state?['enabled'] != true
                                  ? null
                                  : () => editRevision(detail),
                              child: const Text('编辑为新版本'),
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
