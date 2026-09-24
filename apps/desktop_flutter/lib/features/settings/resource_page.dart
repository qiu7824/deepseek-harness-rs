import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:dsh_client/dsh_client.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

class SettingsResourcePage extends StatefulWidget {
  const SettingsResourcePage({
    super.key,
    required this.controller,
    required this.page,
    this.footer,
    this.onOpenPlugin,
  });
  final DesktopController controller;
  final String page;
  final Widget? footer;
  final ValueChanged<Json>? onOpenPlugin;
  @override
  State<SettingsResourcePage> createState() => _SettingsResourcePageState();
}

class _SettingsResourcePageState extends State<SettingsResourcePage> {
  DshClient get api => widget.controller.client!;
  final scope = RequestScope(), search = TextEditingController();
  Json data = {};
  bool loading = true, busy = false;
  String? error;
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
    try {
      Json result;
      switch (widget.page) {
        case 'archive':
          await widget.controller.refreshSessions();
          result = {'entries': widget.controller.archivedSessions};
        case 'discovery':
          result = await api.request('/__dsh-tool-discovery', scope: scope);
        default:
          result = await api.rpc(switch (widget.page) {
            'plugins' => 'pluginInventory.list',
            'presets' => 'agentPreset.list',
            'skills' => 'capabilities.list',
            _ => 'memory.list',
          }, scope: scope);
      }
      if (mounted) {
        setState(() {
          data = result;
          loading = false;
          error = null;
        });
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          error = '$e';
          loading = false;
        });
      }
    }
  }

  Future<void> action(Future<void> Function() work) async {
    setState(() => busy = true);
    try {
      await work();
      await load();
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final title = {
      'plugins': '插件',
      'presets': 'Agent 预设',
      'skills': '技能与 MCP',
      'archive': '归档会话',
      'memory': '记忆与上下文',
      'discovery': '工具发现',
    }[widget.page]!;
    final query = search.text.toLowerCase();
    var entries = objects(data['entries'] ?? data['presets'] ?? data['skills']);
    entries = entries
        .where(
          (e) =>
              '${e['name']} ${e['title']} ${e['moduleName']} ${e['description']}'
                  .toLowerCase()
                  .contains(query),
        )
        .toList();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                title,
                style: const TextStyle(
                  fontSize: 16,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
            DshIcon(
              LucideIcons.refreshCw,
              label: '刷新',
              onPressed: loading ? null : load,
            ),
            if (widget.page == 'skills')
              DshButton(
                outline: true,
                icon: LucideIcons.plus,
                onPressed: () => editSkill(),
                child: const Text('添加技能'),
              ),
          ],
        ),
        const SizedBox(height: 16),
        if (widget.page == 'archive')
          Text(
            '归档只会隐藏会话，完整记录仍会保留。你可以恢复或永久删除记录。',
            style: TextStyle(
              fontSize: 13,
              height: 1.6,
              color: DshColors(context).muted,
            ),
          )
        else
          DshField(
            controller: search,
            hint: '搜索$title',
            prefix: LucideIcons.search,
            onChanged: (_) => setState(() {}),
          ),
        const SizedBox(height: 12),
        if (error != null)
          Text(error!, style: const TextStyle(color: Colors.red, fontSize: 12)),
        if (busy) const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: loading
              ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
              : ListView.builder(
                  itemCount:
                      entries.length +
                      (widget.page == 'skills' ? 1 : 0) +
                      (widget.footer != null ? 1 : 0) +
                      (widget.page == 'discovery' ? 1 : 0),
                  itemBuilder: (context, i) {
                    if (i < entries.length) return row(entries[i]);
                    if (widget.page == 'skills' && i == entries.length) {
                      return serverList();
                    }
                    if (widget.page == 'discovery') return discovery();
                    return widget.footer ?? const SizedBox();
                  },
                ),
        ),
        if (entries.isEmpty &&
            !loading &&
            widget.footer == null &&
            widget.page != 'skills' &&
            widget.page != 'discovery')
          const Padding(
            padding: EdgeInsets.all(20),
            child: Text('暂无记录', style: TextStyle(color: Colors.grey)),
          ),
      ],
    );
  }

  Widget row(Json row) {
    if (widget.page == 'archive') return archiveRow(row);
    final title =
        '${row['name'] ?? row['title'] ?? row['moduleName'] ?? row['id'] ?? row['sessionId']}';
    final description = displayPathText(
      '${row['description'] ?? row['content'] ?? row['cwd'] ?? row['fiberPhase'] ?? row['trust'] ?? ''}',
    );
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 12),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: DshColors(context).border)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: const TextStyle(
                    fontSize: 14,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                if (description.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(top: 5),
                    child: Text(
                      description,
                      maxLines: 3,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 12,
                        height: 1.5,
                        color: DshColors(context).muted,
                      ),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          ...switch (widget.page) {
            'plugins' => [
              if (widget.onOpenPlugin != null &&
                  row['enabled'] == true &&
                  const {
                    'dsh-artifacts',
                    'dsh-context-jump',
                    'dsh-better-sidebar',
                    'dsh-sidebar-workbench-suite',
                    'dsh-voice-input',
                  }.contains(row['moduleName'] ?? row['entryId'] ?? row['id']))
                DshButton(
                  outline: true,
                  height: 28,
                  padding: const EdgeInsets.symmetric(horizontal: 9),
                  onPressed: () => widget.onOpenPlugin!(row),
                  child: Text(
                    '${row['id'] ?? row['entryId'] ?? ''}'.contains('voice')
                        ? '使用语音输入'
                        : '打开插件',
                    style: const TextStyle(fontSize: 12),
                  ),
                ),
              DshSwitch(
                value: row['enabled'] == true,
                onChanged: busy
                    ? null
                    : (v) => action(() async {
                        await api.call('pluginInventory.setEnabled', {
                          'entryId': row['entryId'],
                          'enabled': v,
                        }, true);
                        await widget.controller.loadPlugins();
                      }),
              ),
            ],
            'skills' => [
              DshSwitch(
                value: row['enabled'] == true,
                onChanged: busy
                    ? null
                    : (v) => action(() async {
                        await api.call('capabilities.skillToggle', {
                          'name': row['name'],
                          'enabled': v,
                          'expectedRevision': data['revision'],
                        }, true);
                      }),
              ),
              if (row['managed'] == true)
                DshIcon(
                  LucideIcons.pencil,
                  label: '编辑技能',
                  onPressed: () => editSkill(row),
                ),
            ],
            'presets' => [
              DshIcon(
                LucideIcons.fileText,
                label: '查看预设',
                onPressed: () => viewPreset(row),
              ),
              DshIcon(
                LucideIcons.copy,
                label: '复制预设',
                onPressed: () => copyPreset(row),
              ),
            ],
            'memory' => [
              DshSwitch(
                value: row['enabled'] == true,
                onChanged: (v) => action(() async {
                  await api.call('memory.upsert', {
                    'entry': {...row, 'enabled': v},
                    'expectedRevision': row['revision'],
                  }, true);
                }),
              ),
              DshIcon(
                LucideIcons.pencil,
                label: '编辑记忆',
                onPressed: () => editMemory(row),
              ),
              DshIcon(
                LucideIcons.trash2,
                label: '删除记忆',
                onPressed: () async {
                  if (await confirmAction(
                    context,
                    '删除记忆',
                    title,
                    action: '删除',
                  )) {
                    await action(() async {
                      await api.call('memory.remove', {
                        'id': row['id'],
                        'expectedRevision': row['revision'],
                      }, true);
                    });
                  }
                },
              ),
            ],
            _ => <Widget>[],
          },
        ],
      ),
    );
  }

  Widget archiveRow(Json row) {
    final colors = DshColors(context);
    final cwd = '${row['cwd'] ?? ''}'.replaceAll('\\', '/');
    final workspace = widget.controller.workspaces
        .where((w) => w['path'] == row['cwd'])
        .firstOrNull;
    final label =
        '${workspace?['title'] ?? cwd.split('/').where((s) => s.isNotEmpty).lastOrNull ?? '未分组'}';
    final updated = (row['updatedAt'] as num?)?.toInt() ?? 0;
    final time = updated > 0
        ? DateTime.fromMillisecondsSinceEpoch(updated)
              .toLocal()
              .toString()
              .split('.')
              .first
        : '';
    return Container(
      margin: const EdgeInsets.only(bottom: 12),
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        border: Border.all(color: colors.border),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '${row['title'] ?? '未命名会话'}',
                  style: const TextStyle(fontSize: 14, height: 22 / 14),
                ),
                const SizedBox(height: 5),
                Text(
                  '$label${time.isEmpty ? '' : '   更新于 $time'}',
                  style: TextStyle(
                    fontSize: 12,
                    height: 1.5,
                    color: colors.muted,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          DshButton(
            primary: true,
            pill: true,
            height: 32,
            fontSize: 13,
            onPressed: busy
                ? null
                : () => action(
                    () => widget.controller.archive(
                      '${row['sessionId']}',
                      restore: true,
                    ),
                  ),
            child: const Text('恢复'),
          ),
          const SizedBox(width: 8),
          DshButton(
            outline: true,
            destructive: true,
            pill: true,
            height: 32,
            fontSize: 13,
            onPressed: busy
                ? null
                : () async {
                    if (await confirmAction(
                      context,
                      '永久删除“${row['title'] ?? '未命名会话'}”？',
                      '此操作会永久删除此会话及其所有子智能体的历史记录，并移除对应列表引用；独立分支会话和工作区文件保留，无法恢复。',
                      action: '永久删除',
                    )) {
                      await action(() async {
                        await api.call('workspace.deleteArchivedSession', {
                          'sessionId': row['sessionId'],
                        }, true);
                      });
                    }
                  },
            child: const Text('删除'),
          ),
        ],
      ),
    );
  }

  Widget serverList() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      const SizedBox(height: 24),
      Row(
        children: [
          const Expanded(
            child: Text(
              'MCP 服务器',
              style: TextStyle(fontWeight: FontWeight.w600),
            ),
          ),
          DshButton(
            outline: true,
            onPressed: editServer,
            icon: LucideIcons.plus,
            child: const Text('添加服务器'),
          ),
        ],
      ),
      for (final server in objects(data['servers']))
        ListTile(
          contentPadding: EdgeInsets.zero,
          title: Text(
            '${server['name']}',
            style: const TextStyle(fontSize: 14),
          ),
          subtitle: Text(
            '${server['error'] ?? server['status'] ?? server['transport']}',
            style: const TextStyle(fontSize: 12),
          ),
          trailing: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              DshButton(
                onPressed: () => action(() async {
                  await api.call('capabilities.serverTest', {
                    'name': server['name'],
                  }, true);
                }),
                child: const Text('测试'),
              ),
              DshSwitch(
                value: server['enabled'] == true,
                onChanged: (v) => action(() async {
                  await api.call('capabilities.serverToggle', {
                    'name': server['name'],
                    'enabled': v,
                    'expectedRevision': data['revision'],
                  }, true);
                }),
              ),
            ],
          ),
        ),
    ],
  );
  Widget discovery() {
    final config = object(data['configuration']),
        runtime = object(data['runtime']);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: const Text('按需发现工具', style: TextStyle(fontSize: 14)),
          value: config['enabled'] == true,
          onChanged: (v) => action(() async {
            await api.request(
              '/__dsh-tool-discovery',
              body: {
                'configuration': {...config, 'enabled': v},
                'expectedRevision': data['revision'],
              },
              mutation: true,
            );
          }),
        ),
        if (data['restartRequired'] == true)
          const Text(
            '重启服务后生效',
            style: TextStyle(fontSize: 12, color: Colors.orange),
          ),
        const SizedBox(height: 12),
        for (final entry in runtime.entries)
          if (entry.value is num ||
              entry.value is bool ||
              entry.value is String)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Text(
                '${entry.key}：${entry.value}',
                style: const TextStyle(fontSize: 12),
              ),
            ),
      ],
    );
  }

  Future<void> editSkill([Json? skill]) async {
    var content = '';
    if (skill != null) {
      await action(() async {
        content =
            '${(await api.call('capabilities.skillRead', {'name': skill['name']}))['content']}';
      });
      if (error != null) return;
    }
    if (!mounted) return;
    final result = await showDialog<Json>(
      context: context,
      builder: (_) => TextResourceEditor(
        title: skill == null ? '添加技能' : '编辑技能',
        name: skill?['name'] as String? ?? '',
        content: content,
      ),
    );
    if (result != null) {
      await action(() async {
        await api.call('capabilities.skillSave', {
          'name': result['name'],
          'content': result['content'],
          'overwrite': skill != null,
          'expectedRevision': data['revision'],
        }, true);
      });
    }
  }

  Future<void> viewPreset(Json preset) async {
    await action(() async {
      final result = await api.call('agentPreset.read', {
        'agentPreset': preset['id'],
      });
      if (!mounted) return;
      await showDialog<void>(
        context: context,
        builder: (_) => TextResourceEditor(
          title: 'Agent 预设',
          name: '${preset['id']}',
          content: '${result['content']}',
          readOnly: true,
        ),
      );
    });
  }

  Future<void> copyPreset(Json preset) async {
    final name = await editTextDialog(context, '复制预设', '${preset['id']}-copy');
    if (name == null) return;
    await action(() async {
      await api.call('agentPreset.copy', {
        'source': preset['id'],
        'target': name,
      }, true);
    });
  }

  Future<void> editMemory(Json row) async {
    final result = await showDialog<Json>(
      context: context,
      builder: (_) => TextResourceEditor(
        title: '编辑记忆',
        name: '${row['title']}',
        content: '${row['content']}',
      ),
    );
    if (result != null) {
      await action(() async {
        await api.call('memory.upsert', {
          'entry': {
            ...row,
            'title': result['name'],
            'content': result['content'],
          },
          'expectedRevision': row['revision'],
        }, true);
      });
    }
  }

  Future<void> editServer() async {
    final result = await showDialog<Json>(
      context: context,
      builder: (_) => const McpServerDialog(),
    );
    if (result != null) {
      await action(() async {
        await api.call('capabilities.serverSave', {
          'server': result,
          'expectedRevision': data['revision'],
        }, true);
      });
    }
  }
}

class TextResourceEditor extends StatefulWidget {
  const TextResourceEditor({
    super.key,
    required this.title,
    required this.name,
    required this.content,
    this.readOnly = false,
  });
  final String title, name, content;
  final bool readOnly;
  @override
  State<TextResourceEditor> createState() => _TextResourceEditorState();
}

class _TextResourceEditorState extends State<TextResourceEditor> {
  late final name = TextEditingController(text: widget.name),
      content = TextEditingController(text: widget.content);
  @override
  void dispose() {
    name.dispose();
    content.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Dialog(
    child: SizedBox(
      width: 780,
      height: 600,
      child: Padding(
        padding: const EdgeInsets.all(22),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(widget.title, style: const TextStyle(fontSize: 17)),
            const SizedBox(height: 18),
            DshField(controller: name, hint: '名称'),
            const SizedBox(height: 12),
            Expanded(
              child: TextField(
                controller: content,
                readOnly: widget.readOnly,
                expands: true,
                maxLines: null,
                minLines: null,
                style: const TextStyle(
                  fontFamily: 'Consolas',
                  fontSize: 12,
                  height: 1.5,
                ),
                decoration: const InputDecoration(
                  border: OutlineInputBorder(),
                  hintText: '内容',
                ),
              ),
            ),
            const SizedBox(height: 16),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                DshButton(
                  onPressed: () => Navigator.pop(context),
                  child: const Text('关闭'),
                ),
                if (!widget.readOnly)
                  DshButton(
                    primary: true,
                    onPressed: () => Navigator.pop(context, {
                      'name': name.text,
                      'content': content.text,
                    }),
                    child: const Text('保存'),
                  ),
              ],
            ),
          ],
        ),
      ),
    ),
  );
}

class McpServerDialog extends StatefulWidget {
  const McpServerDialog({super.key});
  @override
  State<McpServerDialog> createState() => _McpServerDialogState();
}

class _McpServerDialogState extends State<McpServerDialog> {
  final name = TextEditingController(),
      command = TextEditingController(),
      args = TextEditingController();
  String? error;
  @override
  void dispose() {
    name.dispose();
    command.dispose();
    args.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('添加 MCP 服务器', style: TextStyle(fontSize: 17)),
    content: SizedBox(
      width: 480,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          DshField(controller: name, hint: '名称'),
          const SizedBox(height: 12),
          DshField(controller: command, hint: '可执行程序'),
          const SizedBox(height: 12),
          DshField(controller: args, hint: '参数 JSON 数组，例如 ["server.js"]'),
          if (error != null)
            Text(error!, style: const TextStyle(color: Colors.red)),
        ],
      ),
    ),
    actions: [
      DshButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      DshButton(
        primary: true,
        onPressed: () {
          try {
            final parsed = args.text.trim().isEmpty
                ? <String>[]
                : jsonDecode(args.text);
            if (name.text.isEmpty ||
                command.text.isEmpty ||
                parsed is! List ||
                parsed.any((e) => e is! String)) {
              throw const FormatException('请填写名称、程序和有效的参数数组');
            }
            Navigator.pop(context, {
              'name': name.text,
              'transport': 'stdio',
              'command': command.text,
              'args': parsed,
              'enabled': false,
            });
          } catch (e) {
            setState(() => error = '$e');
          }
        },
        child: const Text('保存'),
      ),
    ],
  );
}
