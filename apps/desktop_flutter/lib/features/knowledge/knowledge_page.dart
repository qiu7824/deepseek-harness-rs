import 'dart:async';

import 'package:desktop_drop/desktop_drop.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

/// Local knowledge bases over `/__dsh-knowledge/*`. The desktop client only
/// connects to a Host on this machine, so files and folders are imported by
/// path; imports get a long timeout because extraction can take a while.
class KnowledgeApi {
  KnowledgeApi(this.client, [this._importer]);
  final DshClient client;
  DshClient? _importer;
  static const _reads = {'catalog', 'documents', 'search'};

  DshClient get importer => _importer ??= DshClient(
    client.baseUri.toString(),
    timeout: const Duration(minutes: 10),
  );

  Future<Json> call(String operation, [Json body = const {}]) =>
      (operation == 'importPath' ? importer : client).request(
        '/__dsh-knowledge/$operation',
        body: body,
        mutation: !_reads.contains(operation),
        maxBytes: 16 * 1024 * 1024,
      );

  Future<void> close() async {
    await _importer?.close();
  }
}

String formatKnowledgeBytes(num bytes) => bytes < 1024
    ? '$bytes B'
    : bytes < 1048576
    ? '${(bytes / 1024).toStringAsFixed(1)} KB'
    : '${(bytes / 1048576).toStringAsFixed(1)} MB';

class KnowledgePage extends StatefulWidget {
  const KnowledgePage({
    super.key,
    required this.controller,
    required this.onClose,
    this.api,
  });
  final DesktopController controller;
  final VoidCallback onClose;
  final KnowledgeApi? api;
  @override
  State<KnowledgePage> createState() => _KnowledgePageState();
}

class _KnowledgePageState extends State<KnowledgePage> {
  KnowledgeApi? ownedApi;
  Json? catalog;
  String? error, selected;

  KnowledgeApi? get api =>
      widget.api ??
      (ownedApi ??= widget.controller.client == null
          ? null
          : KnowledgeApi(widget.controller.client!));
  List<Json> get bases => objects(catalog?['bases']);

  @override
  void initState() {
    super.initState();
    unawaited(
      load().then((_) {
        if (mounted && selected == null && bases.isNotEmpty) {
          setState(() => selected = bases.first['id'] as String?);
        }
      }),
    );
  }

  @override
  void dispose() {
    unawaited(ownedApi?.close());
    super.dispose();
  }

  Future<void> load() async {
    final api = this.api;
    if (api == null) return;
    try {
      final value = await api.call('catalog');
      if (mounted) {
        setState(() {
          catalog = value;
          error = null;
        });
      }
    } on DshException catch (e) {
      if (mounted) {
        setState(
          () =>
              error = e.code == 'http-404' ? '本机服务不支持知识库，请更新到最新版本' : e.message,
        );
      }
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    }
  }

  Future<void> create() async {
    final api = this.api;
    if (api == null) return;
    final base = await showDialog<Json>(
      context: context,
      builder: (_) => KnowledgeCreateDialog(api: api),
    );
    if (base != null && mounted) {
      setState(() => selected = base['id'] as String?);
      await load();
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final all = bases;
    final current = all.where((base) => base['id'] == selected).firstOrNull;
    return Material(
      color: colors.base,
      child: LayoutBuilder(
        builder: (context, box) {
          final wide = box.maxWidth >= 880;
          final list = _BaseList(
            bases: all,
            loading: catalog == null && error == null,
            selected: selected,
            onSelect: (id) => setState(() => selected = id),
            onCreate: api == null ? null : create,
          );
          final detail = current == null
              ? null
              : KnowledgeBaseDetail(
                  key: ValueKey(current['id']),
                  base: current,
                  api: api!,
                  extensions: [
                    for (final ext in (catalog?['extensions'] as List? ?? []))
                      '$ext',
                  ],
                  onChanged: load,
                  onDeleted: () {
                    setState(() => selected = null);
                    unawaited(load());
                  },
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
                            '知识库',
                            style: TextStyle(
                              fontSize: 22,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                          const SizedBox(height: 4),
                          Text(
                            '导入文档后，智能体会在回答前检索已启用的知识库，并注明引用的文档。数据只保存在本机。',
                            style: TextStyle(fontSize: 13, color: colors.muted),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 12),
                    DshButton(
                      key: const Key('knowledge-create'),
                      primary: true,
                      icon: LucideIcons.plus,
                      onPressed: api == null ? null : create,
                      child: const Text('新建知识库'),
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
                Expanded(
                  child: !wide
                      ? detail ?? list
                      : detail == null
                      ? list
                      : Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            SizedBox(width: 320, child: list),
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

class _BaseList extends StatelessWidget {
  const _BaseList({
    required this.bases,
    required this.loading,
    required this.selected,
    required this.onSelect,
    required this.onCreate,
  });
  final List<Json> bases;
  final bool loading;
  final String? selected;
  final ValueChanged<String> onSelect;
  final VoidCallback? onCreate;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    if (loading) {
      return const Center(child: CircularProgressIndicator(strokeWidth: 2));
    }
    if (bases.isEmpty) {
      return const DshEmpty(
        '还没有知识库\n新建一个知识库，上传文档或导入整个文件夹。\n支持 PDF、Word、PowerPoint、Excel、网页、Markdown、纯文本和代码。',
        icon: LucideIcons.bookOpen,
      );
    }
    return ListView.separated(
      itemCount: bases.length,
      separatorBuilder: (_, _) => const SizedBox(height: 8),
      itemBuilder: (context, index) {
        final base = bases[index];
        final enabled = base['enabled'] == true;
        final description = '${base['description'] ?? ''}';
        return Material(
          key: ValueKey('knowledge-base-${base['id']}'),
          color: colors.layer,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
            side: BorderSide(
              color: base['id'] == selected ? colors.blue : colors.border,
            ),
          ),
          child: InkWell(
            borderRadius: BorderRadius.circular(12),
            onTap: () => onSelect(base['id'] as String),
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
                          color: enabled
                              ? const Color(0xff1e9e5a)
                              : colors.muted,
                          shape: BoxShape.circle,
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          '${base['name']}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                      if (!enabled)
                        Text(
                          '已停用',
                          style: TextStyle(fontSize: 11, color: colors.muted),
                        ),
                    ],
                  ),
                  if (description.isNotEmpty) ...[
                    const SizedBox(height: 4),
                    Text(
                      description,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: colors.muted),
                    ),
                  ],
                  const SizedBox(height: 4),
                  Text(
                    '${base['documentCount'] ?? 0} 个文档 · ${base['chunkCount'] ?? 0} 段',
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

/// One base: name, description, search switch, documents and a search try.
class KnowledgeBaseDetail extends StatefulWidget {
  const KnowledgeBaseDetail({
    super.key,
    required this.base,
    required this.api,
    required this.extensions,
    required this.onChanged,
    required this.onDeleted,
    required this.onClose,
  });
  final Json base;
  final KnowledgeApi api;
  final List<String> extensions;
  final Future<void> Function() onChanged;
  final VoidCallback onDeleted, onClose;
  @override
  State<KnowledgeBaseDetail> createState() => KnowledgeBaseDetailState();
}

class KnowledgeBaseDetailState extends State<KnowledgeBaseDetail> {
  late final name = TextEditingController(text: '${widget.base['name']}');
  late final description = TextEditingController(
    text: '${widget.base['description'] ?? ''}',
  );
  final query = TextEditingController();
  String tab = 'documents';
  String? busy, error, status;
  List<String> failures = [];
  List<Json>? documents, results;
  bool dropping = false;

  bool get dirty =>
      name.text != '${widget.base['name']}' ||
      description.text != '${widget.base['description'] ?? ''}';

  @override
  void initState() {
    super.initState();
    unawaited(loadDocuments());
  }

  @override
  void didUpdateWidget(covariant KnowledgeBaseDetail oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.base['updatedAt'] != widget.base['updatedAt'] && !dirty) {
      name.text = '${widget.base['name']}';
      description.text = '${widget.base['description'] ?? ''}';
    }
  }

  @override
  void dispose() {
    name.dispose();
    description.dispose();
    query.dispose();
    super.dispose();
  }

  String get id => widget.base['id'] as String;

  Future<void> loadDocuments() async {
    try {
      final value = await widget.api.call('documents', {'id': id});
      if (mounted) setState(() => documents = objects(value['documents']));
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    }
  }

  Future<void> run(String name, Future<void> Function() action) async {
    setState(() {
      busy = name;
      error = null;
    });
    try {
      await action();
    } on DshException catch (e) {
      if (mounted) error = e.message;
    } catch (e) {
      if (mounted) error = '$e';
    } finally {
      if (mounted) setState(() => busy = null);
    }
  }

  /// Import files or folders by path, one request each, and summarize.
  Future<void> importPaths(List<String> paths) async {
    if (paths.isEmpty || busy != null) return;
    var added = 0;
    final failed = <String>[];
    var truncated = false;
    setState(() {
      failures = [];
      status = '正在导入…';
    });
    await run('import', () async {
      for (final path in paths) {
        if (!mounted) return;
        setState(() => status = '正在导入 ${path.split(RegExp(r'[\\/]')).last}…');
        try {
          final report = object(
            (await widget.api.call('importPath', {
              'id': id,
              'path': path,
            }))['report'],
          );
          added += (report['added'] as List? ?? []).length;
          truncated |= report['truncated'] == true;
          for (final skipped in objects(report['skipped'])) {
            failed.add('${skipped['name']}：${skipped['reason']}');
          }
        } on DshException catch (e) {
          failed.add('$path：${e.message}');
        }
      }
    });
    if (!mounted) return;
    setState(() {
      status = [
        '已导入 $added 个文档',
        if (failed.isNotEmpty) '跳过 ${failed.length} 个',
        if (truncated) '文件夹内容过多，只导入了前 500 个文件',
      ].join(' · ');
      failures = failed;
    });
    await loadDocuments();
    await widget.onChanged();
  }

  Future<void> pickFiles() async {
    final files = await openFiles(
      acceptedTypeGroups: [
        XTypeGroup(label: '文档', extensions: widget.extensions),
      ],
    );
    await importPaths([for (final file in files) file.path]);
  }

  Future<void> pickFolder() async {
    final path = await getDirectoryPath();
    if (path != null) await importPaths([path]);
  }

  Future<void> search() => run('search', () async {
    final value = await widget.api.call('search', {
      'query': query.text,
      'baseIds': [id],
      'limit': 10,
    });
    if (mounted) setState(() => results = objects(value['results']));
  });

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final base = widget.base;
    final enabled = base['enabled'] == true;
    return Container(
      key: const Key('knowledge-detail'),
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
                  '${base['name']}',
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: 17,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              const Text('参与检索', style: TextStyle(fontSize: 13)),
              const SizedBox(width: 6),
              DshSwitch(
                key: const Key('knowledge-enabled'),
                value: enabled,
                onChanged: busy != null
                    ? null
                    : (value) => run('toggle', () async {
                        await widget.api.call('update', {
                          'id': id,
                          'enabled': value,
                        });
                        await widget.onChanged();
                      }),
              ),
            ],
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              Expanded(
                child: DshField(
                  key: const Key('knowledge-name'),
                  controller: name,
                  hint: '名称',
                  onChanged: (_) => setState(() {}),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                flex: 2,
                child: DshField(
                  key: const Key('knowledge-description'),
                  controller: description,
                  hint: '说明（可选，告诉智能体这个知识库包含什么）',
                  onChanged: (_) => setState(() {}),
                ),
              ),
            ],
          ),
          if (dirty) ...[
            const SizedBox(height: 8),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                Text(
                  '有未保存的修改',
                  style: TextStyle(fontSize: 12, color: colors.muted),
                ),
                const SizedBox(width: 8),
                DshButton(
                  onPressed: busy != null
                      ? null
                      : () => setState(() {
                          name.text = '${base['name']}';
                          description.text = '${base['description'] ?? ''}';
                        }),
                  child: const Text('取消'),
                ),
                const SizedBox(width: 8),
                DshButton(
                  key: const Key('knowledge-save'),
                  primary: true,
                  onPressed: busy != null || name.text.trim().isEmpty
                      ? null
                      : () => run('save', () async {
                          await widget.api.call('update', {
                            'id': id,
                            'name': name.text,
                            'description': description.text,
                          });
                          await widget.onChanged();
                        }),
                  child: Text(busy == 'save' ? '正在保存…' : '保存'),
                ),
              ],
            ),
          ],
          const SizedBox(height: 10),
          Row(
            children: [
              for (final (key, label) in [
                ('documents', '文档 ${base['documentCount'] ?? 0}'),
                ('search', '检索测试'),
              ])
                Padding(
                  padding: const EdgeInsets.only(right: 6),
                  child: DshButton(
                    key: ValueKey('knowledge-tab-$key'),
                    height: 30,
                    pill: true,
                    active: tab == key,
                    onPressed: () => setState(() => tab = key),
                    child: Text(label, style: const TextStyle(fontSize: 13)),
                  ),
                ),
            ],
          ),
          const SizedBox(height: 10),
          if (error != null)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Text(
                error!,
                style: const TextStyle(fontSize: 12, color: Colors.red),
              ),
            ),
          Expanded(
            child: tab == 'documents'
                ? _documents(colors)
                : _search(colors, enabled),
          ),
          const SizedBox(height: 8),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              DshButton(
                key: const Key('knowledge-delete'),
                destructive: true,
                onPressed: busy != null
                    ? null
                    : () async {
                        final confirmed = await confirmAction(
                          context,
                          '删除知识库',
                          '删除“${base['name']}”及其中的 ${base['documentCount'] ?? 0} 个文档？此操作无法撤销。',
                          action: '删除',
                        );
                        if (!confirmed) return;
                        await run('delete', () async {
                          await widget.api.call('delete', {'id': id});
                          widget.onDeleted();
                        });
                      },
                child: const Text('删除知识库'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _documents(DshColors colors) {
    final rows = documents;
    return DropTarget(
      onDragEntered: (_) => setState(() => dropping = true),
      onDragExited: (_) => setState(() => dropping = false),
      onDragDone: (detail) {
        setState(() => dropping = false);
        unawaited(importPaths([for (final file in detail.files) file.path]));
      },
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: double.infinity,
            padding: const EdgeInsets.all(12),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(12),
              border: Border.all(
                color: dropping ? colors.blue : colors.border,
                width: dropping ? 2 : 1,
              ),
            ),
            child: Wrap(
              spacing: 8,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                DshButton(
                  key: const Key('knowledge-upload'),
                  primary: true,
                  icon: LucideIcons.filePlus,
                  onPressed: busy != null ? null : pickFiles,
                  child: const Text('上传文件'),
                ),
                DshButton(
                  key: const Key('knowledge-import-folder'),
                  icon: LucideIcons.folderPlus,
                  onPressed: busy != null ? null : pickFolder,
                  child: const Text('导入文件夹'),
                ),
                Text(
                  '也可以把文件或文件夹拖到这里',
                  style: TextStyle(fontSize: 12, color: colors.muted),
                ),
              ],
            ),
          ),
          if (status != null)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                status!,
                key: const Key('knowledge-status'),
                style: TextStyle(fontSize: 12, color: colors.muted),
              ),
            ),
          for (final line in failures.take(20))
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                line,
                style: const TextStyle(fontSize: 12, color: Colors.red),
              ),
            ),
          const SizedBox(height: 8),
          Expanded(
            child: rows == null
                ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
                : rows.isEmpty
                ? const DshEmpty('还没有文档', icon: LucideIcons.fileText)
                : ListView.separated(
                    itemCount: rows.length,
                    separatorBuilder: (_, _) =>
                        Divider(height: 1, color: colors.border),
                    itemBuilder: (context, index) {
                      final doc = rows[index];
                      final created = DateTime.tryParse('${doc['createdAt']}')
                          ?.toLocal();
                      return Padding(
                        key: ValueKey('knowledge-doc-${doc['id']}'),
                        padding: const EdgeInsets.symmetric(vertical: 8),
                        child: Row(
                          children: [
                            Icon(
                              LucideIcons.fileText,
                              size: 16,
                              color: colors.muted,
                            ),
                            const SizedBox(width: 10),
                            Expanded(
                              child: Tooltip(
                                message: '${doc['source'] ?? doc['name']}',
                                child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Text(
                                      '${doc['name']}',
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: const TextStyle(fontSize: 13),
                                    ),
                                    Text(
                                      [
                                        formatKnowledgeBytes(
                                          (doc['bytes'] as num?) ?? 0,
                                        ),
                                        '${doc['chunkCount']} 段',
                                        '${doc['chars']} 字',
                                        if (created != null)
                                          '${created.year}/${created.month}/${created.day}',
                                      ].join(' · '),
                                      style: TextStyle(
                                        fontSize: 11,
                                        color: colors.muted,
                                      ),
                                    ),
                                  ],
                                ),
                              ),
                            ),
                            DshIcon(
                              LucideIcons.trash2,
                              label: '移除',
                              onPressed: busy != null
                                  ? null
                                  : () => run('remove', () async {
                                      await widget.api.call('deleteDocument', {
                                        'id': doc['id'],
                                      });
                                      await loadDocuments();
                                      await widget.onChanged();
                                    }),
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

  Widget _search(DshColors colors, bool enabled) {
    final rows = results;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Expanded(
              child: DshField(
                key: const Key('knowledge-query'),
                controller: query,
                hint: '输入问题或关键词',
                prefix: LucideIcons.search,
                onChanged: (_) => setState(() {}),
              ),
            ),
            const SizedBox(width: 8),
            DshButton(
              key: const Key('knowledge-search'),
              primary: true,
              onPressed: busy != null || query.text.trim().isEmpty
                  ? null
                  : search,
              child: const Text('检索'),
            ),
          ],
        ),
        const SizedBox(height: 6),
        Text(
          enabled
              ? '这里的结果就是智能体调用 knowledge_search 时看到的内容。'
              : '知识库已停用，智能体不会检索它。',
          style: TextStyle(fontSize: 12, color: colors.muted),
        ),
        const SizedBox(height: 8),
        Expanded(
          child: rows == null
              ? const SizedBox.shrink()
              : rows.isEmpty
              ? const DshEmpty('没有找到相关内容', icon: LucideIcons.searchX)
              : ListView.separated(
                  itemCount: rows.length,
                  separatorBuilder: (_, _) => const SizedBox(height: 8),
                  itemBuilder: (context, index) {
                    final hit = rows[index];
                    return Container(
                      key: ValueKey('knowledge-hit-$index'),
                      padding: const EdgeInsets.all(10),
                      decoration: BoxDecoration(
                        color: colors.base,
                        borderRadius: BorderRadius.circular(10),
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            '${hit['documentName']}  #${((hit['chunk'] as num?) ?? 0) + 1}',
                            style: const TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                          const SizedBox(height: 4),
                          SelectableText(
                            '${hit['snippet']}',
                            style: TextStyle(fontSize: 12, color: colors.muted),
                          ),
                        ],
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class KnowledgeCreateDialog extends StatefulWidget {
  const KnowledgeCreateDialog({super.key, required this.api});
  final KnowledgeApi api;
  @override
  State<KnowledgeCreateDialog> createState() => _KnowledgeCreateDialogState();
}

class _KnowledgeCreateDialogState extends State<KnowledgeCreateDialog> {
  final name = TextEditingController(), description = TextEditingController();
  bool busy = false;
  String? error;

  @override
  void dispose() {
    name.dispose();
    description.dispose();
    super.dispose();
  }

  Future<void> submit() async {
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final value = await widget.api.call('create', {
        'name': name.text,
        'description': description.text,
      });
      if (mounted) Navigator.of(context).pop(object(value['base']));
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
    final colors = DshColors(context);
    return Dialog(
      backgroundColor: colors.base,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 480),
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                '新建知识库',
                style: TextStyle(fontSize: 17, fontWeight: FontWeight.w600),
              ),
              const SizedBox(height: 14),
              DshField(
                key: const Key('knowledge-create-name'),
                controller: name,
                hint: '名称，例如：产品手册',
                autofocus: true,
                onChanged: (_) => setState(() {}),
              ),
              const SizedBox(height: 10),
              DshField(
                key: const Key('knowledge-create-description'),
                controller: description,
                hint: '说明（可选）',
                maxLines: 3,
              ),
              if (error != null) ...[
                const SizedBox(height: 8),
                Text(
                  error!,
                  style: const TextStyle(fontSize: 12, color: Colors.red),
                ),
              ],
              const SizedBox(height: 16),
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  DshButton(
                    onPressed: busy ? null : () => Navigator.of(context).pop(),
                    child: const Text('取消'),
                  ),
                  const SizedBox(width: 8),
                  DshButton(
                    key: const Key('knowledge-create-submit'),
                    primary: true,
                    onPressed: busy || name.text.trim().isEmpty ? null : submit,
                    child: Text(busy ? '正在创建…' : '创建'),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}
