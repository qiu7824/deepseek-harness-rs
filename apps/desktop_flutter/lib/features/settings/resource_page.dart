import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:dsh_client/dsh_client.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../src/controller.dart';
import 'learning_panel.dart';

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
  DshClient? boundApi;
  bool get staleConnection =>
      boundApi == null || widget.controller.client != boundApi;
  DshClient get api {
    if (staleConnection) throw StateError('连接已变化，请关闭后重新打开设置。');
    return boundApi!;
  }

  final scope = RequestScope(), search = TextEditingController();
  Json data = {};
  bool loading = true, busy = false;
  String? error, notice;
  int loadGeneration = 0;
  @override
  void initState() {
    super.initState();
    boundApi = widget.controller.client;
    widget.controller.addListener(connectionChanged);
    load();
  }

  void connectionChanged() {
    if (staleConnection && mounted) {
      scope.cancel();
      loadGeneration++;
      setState(() {
        loading = false;
        error = '连接已变化，请关闭后重新打开设置。';
      });
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(connectionChanged);
    scope.cancel();
    search.dispose();
    super.dispose();
  }

  Future<void> load() async {
    if (staleConnection) {
      connectionChanged();
      return;
    }
    final generation = ++loadGeneration;
    if (mounted) setState(() => loading = true);
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
      if (mounted && generation == loadGeneration) {
        setState(() {
          data = result;
          loading = false;
          error = null;
        });
      }
    } catch (e) {
      if (mounted && generation == loadGeneration) {
        setState(() {
          error = '$e';
          loading = false;
        });
      }
    }
  }

  Future<void> action(Future<void> Function() work) async {
    if (busy || !mounted || staleConnection) return;
    setState(() {
      busy = true;
      error = null;
      notice = null;
    });
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
              onPressed: loading || busy ? null : load,
            ),
            if (widget.page == 'skills')
              DshButton(
                outline: true,
                icon: LucideIcons.plus,
                onPressed: loading || busy ? null : () => editSkill(),
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
        if (notice != null)
          Text(
            notice!,
            style: TextStyle(color: DshColors(context).blue, fontSize: 12),
          ),
        if (busy) const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: loading
              ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
              : ListView.builder(
                  itemCount:
                      entries.length +
                      (widget.page == 'memory' ? 1 : 0) +
                      (widget.page == 'skills' ? 1 : 0) +
                      (widget.footer != null ? 1 : 0) +
                      (widget.page == 'discovery' ? 1 : 0),
                  itemBuilder: (context, i) {
                    if (i < entries.length) return row(entries[i]);
                    if (widget.page == 'memory' && i == entries.length) {
                      return LearningPanel(controller: widget.controller);
                    }
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
            widget.page != 'memory' &&
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
                  onPressed: busy ? null : () => editSkill(row),
                ),
              if (row['managed'] == true)
                DshIcon(
                  LucideIcons.trash2,
                  label: '移除技能',
                  onPressed: busy
                      ? null
                      : () => removeCapability(row, skill: true),
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
                onChanged: busy
                    ? null
                    : (v) => action(() async {
                        await api.call('memory.upsert', {
                          'entry': {...row, 'enabled': v},
                          'expectedRevision': row['revision'],
                        }, true);
                      }),
              ),
              DshIcon(
                LucideIcons.pencil,
                label: '编辑记忆',
                onPressed: busy ? null : () => editMemory(row),
              ),
              DshIcon(
                LucideIcons.trash2,
                label: '删除记忆',
                onPressed: busy
                    ? null
                    : () async {
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
            onPressed: loading || busy ? null : () => editServer(),
            icon: LucideIcons.plus,
            child: const Text('添加服务器'),
          ),
        ],
      ),
      for (final server in objects(data['servers']).where(
        (server) =>
            '${server['name']} ${server['command']} ${server['endpoint']}'
                .toLowerCase()
                .contains(search.text.toLowerCase()),
      ))
        ListTile(
          contentPadding: EdgeInsets.zero,
          title: Text(
            '${server['name']}',
            style: const TextStyle(fontSize: 14),
          ),
          subtitle: Text(
            '${server['error'] ?? const {'connected': '已连接', 'disabled': '已停用', 'error': '连接失败', 'pending': '等待连接'}[server['status']] ?? server['transport']} · ${server['toolCount'] ?? 0} 个工具${server['hasSecrets'] == true ? ' · 已配置凭证' : ''}',
            style: const TextStyle(fontSize: 12),
          ),
          trailing: Wrap(
            children: [
              DshButton(
                onPressed: busy
                    ? null
                    : () => action(() async {
                        final result = await api.call(
                          'capabilities.serverTest',
                          {'name': server['name']},
                          true,
                        );
                        if (result['status'] == 'error' ||
                            result['error'] != null) {
                          throw DshException(
                            'mcp-connection',
                            '${result['error'] ?? '连接失败'}',
                          );
                        }
                        if (mounted) {
                          setState(
                            () => notice =
                                '${server['name']} 连接成功，可用工具 ${result['toolCount'] ?? 0} 个',
                          );
                        }
                      }),
                child: const Text('测试'),
              ),
              DshIcon(
                LucideIcons.pencil,
                label: '编辑 MCP 服务器',
                onPressed: busy ? null : () => editServer(server),
              ),
              DshIcon(
                LucideIcons.trash2,
                label: '移除 MCP 服务器',
                onPressed: busy ? null : () => removeCapability(server),
              ),
              DshSwitch(
                value: server['enabled'] == true,
                onChanged: busy
                    ? null
                    : (v) => action(() async {
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
          onChanged: busy
              ? null
              : (v) => action(() async {
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
    if (busy || loading) return;
    final revision = data['revision'];
    var content = '';
    if (skill != null) {
      await action(() async {
        content =
            '${(await api.call('capabilities.skillRead', {'name': skill['name']}))['content']}';
      });
      if (error != null) return;
    }
    if (!mounted) return;
    final saved = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (_) => TextResourceEditor(
        title: skill == null ? '添加技能' : '编辑技能',
        name: skill?['name'] as String? ?? '',
        content: content,
        nameReadOnly: skill != null,
        onSave: (value) async {
          await api.call('capabilities.skillSave', {
            'name': value['name'],
            'content': value['content'],
            'overwrite': skill != null,
            'expectedRevision': revision,
          }, true);
        },
      ),
    );
    if (saved == true && mounted) await load();
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
    if (busy || loading) return;
    final saved = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (_) => TextResourceEditor(
        title: '编辑记忆',
        name: '${row['title']}',
        content: '${row['content']}',
        onSave: (value) async {
          await api.call('memory.upsert', {
            'entry': {
              ...row,
              'title': value['name'],
              'content': value['content'],
            },
            'expectedRevision': row['revision'],
          }, true);
        },
      ),
    );
    if (saved == true && mounted) await load();
  }

  Future<void> editServer([Json? server]) async {
    if (busy || loading) return;
    final revision = data['revision'];
    final result = await showDialog<Json>(
      context: context,
      barrierDismissible: false,
      builder: (_) => McpServerDialog(
        initial: server,
        onSave: (value) {
          if (server == null &&
              objects(data['servers'])
                  .any((entry) => entry['name'] == value['name'])) {
            throw DshException('duplicate', '同名 MCP 服务器已存在，请使用编辑。');
          }
          return api.call('capabilities.serverSave', {
            'server': value,
            'expectedRevision': revision,
          }, true);
        },
      ),
    );
    if (result != null && mounted) {
      await load();
      if (mounted && (result['status'] == 'error' || result['error'] != null)) {
        setState(() => error = '配置已保存，连接失败：${result['error'] ?? '连接失败'}');
      }
    }
  }

  Future<void> removeCapability(Json entry, {bool skill = false}) async {
    if (busy || loading) return;
    final revision = data['revision'];
    if (!await confirmAction(
          context,
          '移除${skill ? '技能' : 'MCP 服务器'}“${entry['name']}”？',
          skill ? '技能文件将移入本机回收目录。' : '移除服务器配置并断开连接，其工具将不再可用。',
          action: '移除',
        ) ||
        !mounted) {
      return;
    }
    await action(() async {
      await api.call(
        skill ? 'capabilities.skillRemove' : 'capabilities.serverRemove',
        {'name': entry['name'], 'expectedRevision': revision},
        true,
      );
    });
  }
}

class TextResourceEditor extends StatefulWidget {
  const TextResourceEditor({
    super.key,
    required this.title,
    required this.name,
    required this.content,
    this.readOnly = false,
    this.nameReadOnly = false,
    this.onSave,
  });
  final String title, name, content;
  final bool readOnly, nameReadOnly;
  final Future<void> Function(Json)? onSave;
  @override
  State<TextResourceEditor> createState() => _TextResourceEditorState();
}

class _TextResourceEditorState extends State<TextResourceEditor> {
  late final name = TextEditingController(text: widget.name),
      content = TextEditingController(text: widget.content);
  bool busy = false;
  String? error;
  @override
  void dispose() {
    name.dispose();
    content.dispose();
    super.dispose();
  }

  Future<void> save() async {
    if (busy || widget.readOnly) return;
    final value = {'name': name.text.trim(), 'content': content.text};
    if (widget.onSave == null) {
      Navigator.pop(context, value);
      return;
    }
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await widget.onSave!(value);
      if (mounted) Navigator.pop(context, true);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: Dialog(
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
              DshField(
                controller: name,
                hint: '名称',
                enabled: !busy && !widget.readOnly && !widget.nameReadOnly,
              ),
              const SizedBox(height: 12),
              Expanded(
                child: TextField(
                  controller: content,
                  readOnly: busy || widget.readOnly,
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
              if (error != null)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: Text(
                    error!,
                    style: const TextStyle(color: Colors.red, fontSize: 12),
                  ),
                ),
              const SizedBox(height: 16),
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  DshButton(
                    onPressed: busy ? null : () => Navigator.pop(context),
                    child: const Text('关闭'),
                  ),
                  if (!widget.readOnly)
                    DshButton(
                      primary: true,
                      onPressed: busy ? null : save,
                      child: Text(busy ? '保存中…' : '保存'),
                    ),
                ],
              ),
            ],
          ),
        ),
      ),
    ),
  );
}

class McpServerDialog extends StatefulWidget {
  const McpServerDialog({super.key, this.initial, this.onSave});
  final Json? initial;
  final Future<Json> Function(Json)? onSave;
  @override
  State<McpServerDialog> createState() => _McpServerDialogState();
}

class _McpServerDialogState extends State<McpServerDialog> {
  late final name = TextEditingController(
        text: widget.initial?['name'] as String? ?? '',
      ),
      command = TextEditingController(
        text: widget.initial?['command'] as String? ?? '',
      ),
      args = TextEditingController(
        text: jsonEncode(widget.initial?['args'] ?? []),
      ),
      cwd = TextEditingController(
        text: widget.initial?['cwd'] as String? ?? '',
      ),
      endpoint = TextEditingController(
        text: widget.initial?['endpoint'] as String? ?? '',
      ),
      env = TextEditingController(),
      headers = TextEditingController();
  late String transport = widget.initial?['transport'] as String? ?? 'stdio';
  late bool enabled = widget.initial?['enabled'] == true;
  bool busy = false;
  String? error;
  @override
  void dispose() {
    name.dispose();
    command.dispose();
    args.dispose();
    cwd.dispose();
    endpoint.dispose();
    env.dispose();
    headers.dispose();
    super.dispose();
  }

  Map<String, String>? parseSecrets(TextEditingController field, String label) {
    if (field.text.trim().isEmpty) return null;
    final value = jsonDecode(field.text);
    if (value is! Map || value.values.any((entry) => entry is! String)) {
      throw FormatException('$label必须为字符串键值组成的 JSON 对象');
    }
    return Map<String, String>.from(value);
  }

  Future<void> save() async {
    if (busy) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      if (!RegExp(r'^[a-zA-Z0-9_-]{1,32}$').hasMatch(name.text.trim())) {
        throw const FormatException('名称须为 1–32 个字母、数字、下划线或连字符');
      }
      final parsedArgs = args.text.trim().isEmpty
          ? <String>[]
          : jsonDecode(args.text);
      if (parsedArgs is! List || parsedArgs.any((entry) => entry is! String)) {
        throw const FormatException('参数必须为字符串组成的 JSON 数组');
      }
      if (transport == 'stdio' && command.text.trim().isEmpty) {
        throw const FormatException('请填写可执行程序');
      }
      if (transport == 'http') {
        final uri = Uri.tryParse(endpoint.text.trim());
        if (uri == null ||
            !['http', 'https'].contains(uri.scheme) ||
            uri.host.isEmpty) {
          throw const FormatException('请填写有效的 HTTP 或 HTTPS 服务器地址');
        }
      }
      final envValue = parseSecrets(env, '环境变量'),
          headerValue = parseSecrets(headers, '请求头');
      final value = <String, dynamic>{
        'name': name.text.trim(),
        'transport': transport,
        'command': command.text.trim(),
        'args': parsedArgs,
        'cwd': cwd.text.trim(),
        'endpoint': endpoint.text.trim(),
        'enabled': enabled,
        'env': ?envValue,
        'headers': ?headerValue,
      };
      final result = widget.onSave == null
          ? value
          : await widget.onSave!(value);
      if (mounted) Navigator.pop(context, result);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Widget field(
    TextEditingController controller,
    String label, {
    String? hint,
    int lines = 1,
    bool secret = false,
    bool locked = false,
  }) => Padding(
    padding: const EdgeInsets.only(bottom: 12),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: const TextStyle(fontSize: 13)),
        const SizedBox(height: 6),
        DshField(
          key: ValueKey('mcp-$label'),
          controller: controller,
          hint: hint,
          maxLines: lines,
          secret: secret,
          enabled: !busy && !locked,
        ),
      ],
    ),
  );

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: AlertDialog(
      title: Text(
        widget.initial == null ? '添加 MCP 服务器' : '编辑 MCP 服务器',
        style: const TextStyle(fontSize: 17),
      ),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              field(name, '名称', locked: widget.initial != null),
              DshSelect<String>(
                options: const {'stdio': '本地命令（stdio）', 'http': 'HTTP / HTTPS'},
                value: transport,
                onChanged: busy
                    ? null
                    : (value) => setState(() => transport = value),
              ),
              const SizedBox(height: 16),
              if (transport == 'stdio') ...[
                field(command, '可执行程序', hint: 'npx / python / 可执行文件路径'),
                field(args, '参数（JSON 数组）', hint: '["server.js"]', lines: 2),
                field(cwd, '工作目录', hint: '留空使用运行目录'),
                field(
                  env,
                  '环境变量（JSON 对象）',
                  hint: widget.initial == null
                      ? '{"API_KEY":"..."}'
                      : '留空保留已有值；{} 清空',
                  secret: true,
                ),
              ] else ...[
                field(endpoint, '服务器地址', hint: 'https://example.com/mcp'),
                field(
                  headers,
                  '请求头（JSON 对象）',
                  hint: widget.initial == null
                      ? '{"Authorization":"Bearer ..."}'
                      : '留空保留已有值；{} 清空',
                  secret: true,
                ),
              ],
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                title: const Text('启用服务器', style: TextStyle(fontSize: 13)),
                value: enabled,
                onChanged: busy
                    ? null
                    : (value) => setState(() => enabled = value),
              ),
              Text(
                '保存并启用将启动本地命令或连接服务器；凭证保存在本机。',
                style: TextStyle(fontSize: 12, color: DshColors(context).muted),
              ),
              if (error != null)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: Text(
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
          onPressed: busy ? null : save,
          child: Text(busy ? '保存中…' : '保存'),
        ),
      ],
    ),
  );
}
