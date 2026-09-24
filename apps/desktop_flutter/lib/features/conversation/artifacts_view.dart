import 'dart:async';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/context_menu.dart';
import '../workbench/workbench_panel.dart' show NativeFileViewer, previewUrl;
import 'session_status.dart' show ProjectionTextEditor;

String artifactName(String path) => path.replaceAll('\\', '/').split('/').last;
String artifactDisplayPath(String path) => displayPath(path);
String artifactBytes(Object? value) {
  final n = value is num ? value : 0;
  return n >= 1073741824
      ? '${(n / 1073741824).toStringAsFixed(2)} GiB'
      : n >= 1048576
      ? '${(n / 1048576).toStringAsFixed(1)} MiB'
      : n >= 1024
      ? '${(n / 1024).toStringAsFixed(1)} KiB'
      : '${n.round()} B';
}

const artifactLabels = {
  'created': '新增',
  'modified': '修改',
  'presented': '已交付',
  'deleted': '已移除',
  'active': '运行中',
  'retained': '保留中',
  'candidate': '待交付',
  'interrupted': '运行中断',
  'failed': '失败材料',
  'quarantined': '恢复队列',
  'reclaimed': '已回收',
};

/// One bounded request at a time, with screen/app visibility owning polling.
abstract class _PollingState<T extends StatefulWidget> extends State<T>
    with WidgetsBindingObserver {
  DshClient get api;
  String get operation;
  Json get arguments;
  final actions = RequestScope();
  RequestScope? reader;
  Timer? poll;
  Json data = {};
  String? error;
  bool loading = true,
      busy = false,
      working = false,
      foreground = true,
      visible = true;
  int generation = 0, failures = 0;
  bool get enabled => true;
  bool get active => mounted && visible && foreground && enabled;
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    foreground =
        WidgetsBinding.instance.lifecycleState == null ||
        WidgetsBinding.instance.lifecycleState == AppLifecycleState.resumed;
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    visible = TickerMode.valuesOf(context).enabled;
    if (active) {
      unawaited(load());
    } else {
      pause();
    }
  }

  void pause() {
    poll?.cancel();
    poll = null;
    reader?.cancel();
    reader = null;
    generation++;
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    foreground = state == AppLifecycleState.resumed;
    if (active) {
      unawaited(load());
    } else {
      pause();
    }
  }

  @override
  void dispose() {
    pause();
    actions.cancel();
    data = {};
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  void schedule() {
    poll?.cancel();
    if (active) {
      poll = Timer(
        Duration(seconds: failures == 0 ? 3 : (3 * failures).clamp(3, 30)),
        () => unawaited(load()),
      );
    }
  }

  Future<void> load() async {
    if (!active || reader != null || busy) {
      schedule();
      return;
    }
    poll?.cancel();
    final token = ++generation, scope = RequestScope();
    reader = scope;
    try {
      final result = await api.request(
        '/__dsh-artifacts/$operation',
        body: arguments,
        scope: scope,
        maxBytes: 2 * 1024 * 1024,
      );
      if (!active || token != generation) return;
      setState(() {
        data = result;
        loading = false;
        error = null;
        failures = 0;
      });
    } catch (e) {
      if (active && token == generation) {
        setState(() {
          error = '$e';
          loading = false;
          failures++;
        });
      }
    } finally {
      if (token == generation) {
        reader = null;
        schedule();
      }
    }
  }

  Future<void> run(
    Future<void> Function() work, {
    bool refresh = true,
    bool indicate = true,
  }) async {
    if (busy) return;
    pause();
    setState(() {
      busy = true;
      working = indicate;
      error = null;
    });
    try {
      await work();
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) {
        setState(() {
          busy = false;
          working = false;
        });
        if (refresh && error == null) {
          await load();
        } else {
          schedule();
        }
      }
    }
  }

  Widget notices() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      if (error != null || data['warning'] != null)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 8),
          child: Text(
            error ?? '${data['warning']}',
            style: const TextStyle(color: Colors.red, fontSize: 13),
          ),
        ),
      if (working) const LinearProgressIndicator(minHeight: 1),
    ],
  );
}

class ArtifactsView extends StatefulWidget {
  const ArtifactsView({super.key, required this.api, required this.session});
  final DshClient api;
  final String session;
  @override
  State<ArtifactsView> createState() => _ArtifactsViewState();
}

class _ArtifactsViewState extends _PollingState<ArtifactsView> {
  bool garbage = false;
  @override
  DshClient get api => widget.api;
  @override
  String get operation => 'list';
  @override
  Json get arguments => {'sessionId': widget.session};
  @override
  bool get enabled => !garbage;
  Future<void> change(Json row, String action, {String? path}) async {
    await api.request(
      '/__dsh-artifacts/file-action',
      body: {
        'sessionId': widget.session,
        'path': row['path'],
        'etag': row['etag'],
        'action': action,
        'newPath': ?path,
      },
      scope: actions,
      mutation: true,
      maxBytes: 65536,
    );
  }

  Future<void> manage(Json row, String intent) async {
    final path = '${row['path']}';
    if (intent == 'rename') {
      await run(
        () => showDialog<void>(
          context: context,
          barrierDismissible: false,
          builder: (_) => ProjectionTextEditor(
            title: '重命名产物',
            maxLines: 1,
            width: 392,
            initial: path,
            onSave: (text) => change(row, 'rename', path: text),
          ),
        ),
        indicate: false,
      );
      return;
    }
    if (intent == 'trash') {
      if (!await confirmAction(context, '移入垃圾槽', path, action: '移入')) return;
      if (mounted) await run(() => change(row, 'trash'));
      return;
    }
    await run(
      () async {
        if (intent == 'copy') {
          await Clipboard.setData(ClipboardData(text: displayPath(path)));
          return;
        }
        if (intent == 'preview') {
          await showDialog<void>(
            context: context,
            builder: (_) =>
                ArtifactPreview(api: api, session: widget.session, path: path),
          );
          return;
        }
        if (intent == 'save') {
          final target = await getSaveLocation(
            suggestedName: artifactName(path),
          );
          if (target == null || !mounted) return;
          await api.downloadTo(
            previewUrl('file', widget.session, {'path': path}),
            File(target.path),
            scope: actions,
          );
          return;
        }
        final office = RegExp(
          r'\.(docx?|xlsx?|pptx?|wps|et|dps)$',
          caseSensitive: false,
        ).hasMatch(path);
        await api.request(
          '/__dsh-preview/file-action',
          body: {
            'sessionId': widget.session,
            'path': path,
            'intent': intent == 'open' && office ? 'office' : intent,
          },
          scope: actions,
          mutation: true,
          maxBytes: 65536,
        );
      },
      refresh: false,
      indicate: intent != 'preview' && intent != 'copy',
    );
  }

  @override
  Widget build(BuildContext context) {
    if (garbage) {
      return ManagedResourcesView(
        api: api,
        session: widget.session,
        onClose: () {
          setState(() => garbage = false);
          unawaited(load());
        },
      );
    }
    final rows = objects(data['entries']), colors = DshColors(context);
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 960),
        child: Padding(
          padding: EdgeInsets.all(
            MediaQuery.sizeOf(context).width < 650 ? 10 : 18,
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Container(
                padding: const EdgeInsets.only(bottom: 12),
                decoration: BoxDecoration(
                  border: Border(bottom: BorderSide(color: colors.border)),
                ),
                child: Row(
                  children: [
                    const Expanded(
                      child: Text(
                        '产物',
                        style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    Text(
                      '${rows.length} 个文件',
                      style: const TextStyle(fontSize: 14),
                    ),
                    const SizedBox(width: 12),
                    DshButton(
                      outline: true,
                      height: 33,
                      onPressed: busy ? null : load,
                      child: const Text('刷新'),
                    ),
                  ],
                ),
              ),
              notices(),
              if (data['truncated'] == true)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 8),
                  child: Text(
                    '工作区较大，扫描已达到文件数量上限；工具记录的变更仍会显示。',
                    style: TextStyle(fontSize: 12),
                  ),
                ),
              Flexible(
                fit: FlexFit.loose,
                child: SizedBox(
                  height: (rows.isEmpty ? 128 : rows.length * 51 + 28)
                      .toDouble(),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(vertical: 14),
                    child: loading
                        ? const Center(
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : rows.isEmpty
                        ? const DshEmpty('此任务暂未记录文件变更')
                        : ListView.builder(
                            itemExtent: 51,
                            itemCount: rows.length,
                            itemBuilder: (context, index) {
                              final row = rows[index],
                                  deleted = row['change'] == 'deleted',
                                  path = '${row['path']}';
                              return Container(
                                padding: const EdgeInsets.symmetric(
                                  vertical: 8,
                                ),
                                decoration: BoxDecoration(
                                  border: Border(
                                    bottom: BorderSide(color: colors.border),
                                  ),
                                ),
                                child: Row(
                                  children: [
                                    Expanded(
                                      child: InkWell(
                                        onSecondaryTapDown: (event) async {
                                          if (deleted || busy) return;
                                          final action =
                                              await nativeContextMenu(
                                                context,
                                                event.globalPosition,
                                                {
                                                  'preview': '预览',
                                                  'copy': '复制路径',
                                                  'reveal': '在资源管理器中显示',
                                                  'open': '使用本地工具打开',
                                                  'save': '保存原文件副本',
                                                  'rename': '重命名',
                                                  'trash': '移入垃圾槽',
                                                },
                                              );
                                          if (mounted && action != null) {
                                            await manage(row, action);
                                          }
                                        },
                                        onTap: deleted || busy
                                            ? null
                                            : () => manage(row, 'preview'),
                                        child: Padding(
                                          padding: const EdgeInsets.symmetric(
                                            horizontal: 10,
                                            vertical: 6,
                                          ),
                                          child: Row(
                                            children: [
                                              Text(
                                                artifactLabels[row['change']] ??
                                                    '${row['change']}',
                                                style: TextStyle(
                                                  fontSize: 12,
                                                  color:
                                                      row['change'] == 'created'
                                                      ? const Color(0xff22c55e)
                                                      : row['change'] ==
                                                            'modified'
                                                      ? colors.blue
                                                      : colors.muted,
                                                ),
                                              ),
                                              const SizedBox(width: 12),
                                              Expanded(
                                                child: Text(
                                                  displayPath(path),
                                                  maxLines: 1,
                                                  overflow:
                                                      TextOverflow.ellipsis,
                                                  style: TextStyle(
                                                    fontSize: 14,
                                                    color: deleted
                                                        ? colors.muted
                                                        : colors.text,
                                                  ),
                                                ),
                                              ),
                                              const SizedBox(width: 12),
                                              Text(
                                                artifactBytes(row['size']),
                                                style: TextStyle(
                                                  fontSize: 12,
                                                  color: colors.muted,
                                                ),
                                              ),
                                            ],
                                          ),
                                        ),
                                      ),
                                    ),
                                    PopupMenuButton<String>(
                                      tooltip: '管理 $path',
                                      enabled: !busy,
                                      onSelected: (intent) =>
                                          manage(row, intent),
                                      padding: EdgeInsets.zero,
                                      constraints: const BoxConstraints(
                                        minWidth: 200,
                                        maxWidth: 200,
                                      ),
                                      menuPadding: const EdgeInsets.all(6),
                                      color: colors.base,
                                      surfaceTintColor: Colors.transparent,
                                      shape: RoundedRectangleBorder(
                                        borderRadius: BorderRadius.circular(9),
                                        side: BorderSide(color: colors.border),
                                      ),
                                      itemBuilder: (_) => [
                                        for (final item in const {
                                          'preview': '预览',
                                          'copy': '复制路径',
                                          'reveal': '在资源管理器中显示',
                                          'open': '使用本地工具打开',
                                          'editor': '在编辑器中打开',
                                          'save': '保存原文件副本',
                                          'rename': '重命名',
                                          'trash': '移入垃圾槽',
                                        }.entries)
                                          PopupMenuItem(
                                            height: 34,
                                            value: item.key,
                                            enabled: !deleted,
                                            child: Text(
                                              item.value,
                                              style: const TextStyle(
                                                fontSize: 14,
                                              ),
                                            ),
                                          ),
                                      ],
                                      child: Container(
                                        width: 36,
                                        height: 34,
                                        decoration: BoxDecoration(
                                          border: Border.all(
                                            color: colors.border,
                                          ),
                                          borderRadius: BorderRadius.circular(
                                            7,
                                          ),
                                        ),
                                        child: DshGlyph(
                                          LucideIcons.ellipsis,
                                          size: 16,
                                          color: colors.muted,
                                        ),
                                      ),
                                    ),
                                  ],
                                ),
                              );
                            },
                          ),
                  ),
                ),
              ),
              Align(
                alignment: Alignment.centerLeft,
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: 12),
                  child: DshButton(
                    outline: true,
                    height: 33,
                    onPressed: busy
                        ? null
                        : () {
                            pause();
                            setState(() => garbage = true);
                          },
                    child: const Text('查看产生的垃圾列表'),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class ArtifactPreview extends StatefulWidget {
  const ArtifactPreview({
    super.key,
    required this.api,
    required this.session,
    required this.path,
    this.line = 1,
  });
  final DshClient api;
  final String session, path;
  final int line;
  @override
  State<ArtifactPreview> createState() => _ArtifactPreviewState();
}

class _ArtifactPreviewState extends State<ArtifactPreview> {
  final cache = ResourceCache<String, String>(
    maxBytes: 16 * 1024 * 1024,
    maxEntries: 1,
    sizeOf: (s) => s.length * 2,
  );
  @override
  void dispose() {
    cache.clear();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Dialog(
    child: SizedBox(
      width: 1100,
      height: MediaQuery.sizeOf(context).height * .82,
      child: Column(
        children: [
          Align(
            alignment: Alignment.centerRight,
            child: DshIcon(
              LucideIcons.x,
              label: '关闭产物预览',
              onPressed: () => Navigator.pop(context),
            ),
          ),
          Expanded(
            child: NativeFileViewer(
              api: widget.api,
              session: widget.session,
              path: widget.path,
              initialLine: widget.line,
              cache: cache,
              onPage: (_) {},
              key: ValueKey(widget.path),
            ),
          ),
        ],
      ),
    ),
  );
}

class ManagedResourcesView extends StatefulWidget {
  const ManagedResourcesView({
    super.key,
    required this.api,
    this.session,
    this.onClose,
  });
  final DshClient api;
  final String? session;
  final VoidCallback? onClose;
  @override
  State<ManagedResourcesView> createState() => _ManagedResourcesViewState();
}

class _ManagedResourcesViewState extends _PollingState<ManagedResourcesView> {
  @override
  DshClient get api => widget.api;
  @override
  String get operation => 'resources';
  @override
  Json get arguments => {'sessionId': ?widget.session};
  Future<void> change(Json row, String action) => run(() async {
    await api.request(
      '/__dsh-artifacts/${action == 'restore-file' ? 'file-action' : 'resource-action'}',
      body: action == 'restore-file'
          ? {'sessionId': row['owner'], 'id': row['id'], 'action': 'restore'}
          : {
              'id': row['id'],
              'action': action,
              'pinned': row['pinned'] != true,
            },
      scope: actions,
      mutation: true,
      maxBytes: 65536,
    );
  });
  @override
  Widget build(BuildContext context) {
    final rows = objects(data['entries']),
        colors = DshColors(context),
        total = rows.fold<num>(
          0,
          (sum, row) => sum + (row['bytes'] is num ? row['bytes'] as num : 0),
        );
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 960),
        child: Padding(
          padding: const EdgeInsets.all(18),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Wrap(
                spacing: 12,
                runSpacing: 6,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  const Text(
                    '垃圾槽',
                    style: TextStyle(fontSize: 14, fontWeight: FontWeight.w600),
                  ),
                  Text('${rows.length} 项 · ${artifactBytes(total)}'),
                  DshButton(
                    outline: true,
                    height: 32,
                    onPressed: busy
                        ? null
                        : () async {
                            if (!await confirmAction(
                              context,
                              '清理可回收项',
                              '移除已到期且未固定、未使用的受管临时材料。',
                              action: '清理',
                            )) {
                              return;
                            }
                            if (mounted) {
                              await run(() async {
                                await api.request(
                                  '/__dsh-artifacts/collect',
                                  body: {},
                                  scope: actions,
                                  mutation: true,
                                  maxBytes: 65536,
                                );
                              });
                            }
                          },
                    child: const Text('清理可回收项'),
                  ),
                  if (widget.onClose != null)
                    DshButton(
                      outline: true,
                      height: 32,
                      onPressed: busy ? null : widget.onClose,
                      child: const Text('返回产物'),
                    ),
                ],
              ),
              notices(),
              const SizedBox(height: 12),
              Flexible(
                fit: FlexFit.loose,
                child: SizedBox(
                  height:
                      (rows.isEmpty
                              ? 120
                              : rows.length *
                                    (MediaQuery.sizeOf(context).width < 650
                                        ? 260
                                        : 180))
                          .toDouble(),
                  child: loading
                      ? const Center(
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : rows.isEmpty
                      ? const DshEmpty('暂无受管临时材料')
                      : ListView.builder(
                          itemCount: rows.length,
                          itemBuilder: (context, index) {
                            final row = rows[index],
                                origin = object(row['origin']),
                                state = '${row['state']}',
                                inUse = row['busy'] == true,
                                pinned = row['pinned'] == true;
                            return Container(
                              padding: const EdgeInsets.symmetric(vertical: 14),
                              decoration: BoxDecoration(
                                border: Border(
                                  bottom: BorderSide(color: colors.border),
                                ),
                              ),
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Wrap(
                                    spacing: 12,
                                    children: [
                                      Text(
                                        '${row['label']}',
                                        style: const TextStyle(
                                          fontSize: 14,
                                          fontWeight: FontWeight.w600,
                                        ),
                                      ),
                                      Text(
                                        inUse
                                            ? '正在使用'
                                            : pinned
                                            ? '固定保留'
                                            : artifactLabels[state] ?? state,
                                        style: TextStyle(
                                          fontSize: 12,
                                          color: colors.muted,
                                        ),
                                      ),
                                      Text(
                                        row['sizePending'] == true
                                            ? '占用统计中'
                                            : artifactBytes(row['bytes']),
                                        style: const TextStyle(fontSize: 12),
                                      ),
                                    ],
                                  ),
                                  Text(
                                    '${row['owner']}',
                                    style: TextStyle(
                                      fontSize: 12,
                                      color: colors.muted,
                                    ),
                                  ),
                                  Padding(
                                    padding: const EdgeInsets.symmetric(
                                      vertical: 12,
                                    ),
                                    child: Text(
                                      artifactDisplayPath('${row['path']}'),
                                      style: TextStyle(
                                        fontSize: 12,
                                        color: colors.muted,
                                      ),
                                    ),
                                  ),
                                  if (row['error'] != null)
                                    Text(
                                      '${row['error']}',
                                      style: const TextStyle(color: Colors.red),
                                    ),
                                  Align(
                                    alignment: Alignment.centerLeft,
                                    child: Wrap(
                                      children: [
                                        if (state != 'reclaimed')
                                          DshButton(
                                            outline: true,
                                            height: 30,
                                            onPressed: busy
                                                ? null
                                                : () => run(
                                                    () => showDialog<void>(
                                                      context: context,
                                                      builder: (_) =>
                                                          ResourceContents(
                                                            api: api,
                                                            id: '${row['id']}',
                                                          ),
                                                    ),
                                                    refresh: false,
                                                    indicate: false,
                                                  ),
                                            child: const Text('查看文件'),
                                          ),
                                      ],
                                    ),
                                  ),
                                  Wrap(
                                    spacing: 8,
                                    runSpacing: 6,
                                    children: [
                                      if (state != 'reclaimed')
                                        DshButton(
                                          outline: true,
                                          height: 30,
                                          onPressed:
                                              busy || state == 'quarantined'
                                              ? null
                                              : () => change(row, 'pin'),
                                          child: Text(pinned ? '取消固定' : '固定保留'),
                                        ),
                                      if (state == 'candidate')
                                        DshButton(
                                          outline: true,
                                          height: 30,
                                          onPressed: busy || inUse
                                              ? null
                                              : () => change(row, 'release'),
                                          child: const Text('标记已完成'),
                                        ),
                                      if (row['kind'] == 'trash' &&
                                          origin['sha256'] != null &&
                                          origin['restored'] != true)
                                        DshButton(
                                          outline: true,
                                          height: 30,
                                          onPressed: busy || inUse
                                              ? null
                                              : () =>
                                                    change(row, 'restore-file'),
                                          child: const Text('恢复原文件'),
                                        ),
                                      if (state == 'quarantined')
                                        DshButton(
                                          outline: true,
                                          height: 30,
                                          onPressed: busy
                                              ? null
                                              : () => change(row, 'restore'),
                                          child: const Text('恢复临时材料'),
                                        )
                                      else if (state != 'reclaimed')
                                        DshButton(
                                          outline: true,
                                          height: 30,
                                          onPressed:
                                              busy ||
                                                  inUse ||
                                                  pinned ||
                                                  state == 'candidate'
                                              ? null
                                              : () => change(row, 'trash'),
                                          child: const Text('移入恢复队列'),
                                        ),
                                    ],
                                  ),
                                ],
                              ),
                            );
                          },
                        ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class ResourceContents extends StatefulWidget {
  const ResourceContents({super.key, required this.api, required this.id});
  final DshClient api;
  final String id;
  @override
  State<ResourceContents> createState() => _ResourceContentsState();
}

class _ResourceContentsState extends State<ResourceContents> {
  String directory = '';
  Json data = {};
  String? text, error;
  bool loading = true;
  RequestScope? scope;
  int generation = 0;
  @override
  void initState() {
    super.initState();
    load('');
  }

  @override
  void dispose() {
    generation++;
    scope?.cancel();
    data = {};
    text = null;
    super.dispose();
  }

  Future<void> load(String path, {bool file = false}) async {
    scope?.cancel();
    final current = RequestScope(), token = ++generation;
    scope = current;
    setState(() {
      loading = true;
      error = null;
      text = null;
    });
    try {
      final value = await widget.api.request(
        '/__dsh-artifacts/${file ? 'resource-read' : 'resource-files'}',
        body: {'id': widget.id, 'path': path},
        scope: current,
        maxBytes: 2 * 1024 * 1024,
      );
      if (mounted && token == generation) {
        setState(() {
          if (file) {
            text = '${value['text'] ?? ''}';
          } else {
            data = value;
            directory = path;
          }
        });
      }
    } catch (e) {
      if (mounted && token == generation) setState(() => error = '$e');
    } finally {
      if (mounted && token == generation) setState(() => loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final entries = objects(data['entries']);
    return AlertDialog(
      title: const Text('受管文件', style: TextStyle(fontSize: 16)),
      content: SizedBox(
        width: 640,
        height: MediaQuery.sizeOf(context).height * .6,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (directory.isNotEmpty)
              DshButton(
                height: 30,
                child: const Text('上一级'),
                onPressed: () => load(
                  directory
                      .split('/')
                      .take(directory.split('/').length - 1)
                      .join('/'),
                ),
              ),
            if (error != null)
              Text(error!, style: const TextStyle(color: Colors.red)),
            if (data['warning'] != null) Text('${data['warning']}'),
            if (data['truncated'] == true) const Text('目录较大，显示前 500 项'),
            if (loading) const LinearProgressIndicator(minHeight: 1),
            Expanded(
              child: text != null
                  ? SingleChildScrollView(
                      child: SelectableText(
                        text!,
                        style: const TextStyle(
                          fontSize: 12,
                          fontFamily: 'Consolas',
                        ),
                      ),
                    )
                  : ListView.builder(
                      itemCount: entries.length,
                      itemBuilder: (context, index) {
                        final row = entries[index];
                        return ListTile(
                          dense: true,
                          title: Text('${row['name']}'),
                          trailing: Text(
                            row['kind'] == 'directory'
                                ? '›'
                                : artifactBytes(row['size']),
                          ),
                          onTap: () => load(
                            '${row['path']}',
                            file: row['kind'] != 'directory',
                          ),
                        );
                      },
                    ),
            ),
            if (text != null)
              DshButton(
                height: 30,
                onPressed: () => load(directory),
                child: const Text('返回目录'),
              ),
          ],
        ),
      ),
      actions: [
        DshButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('关闭'),
        ),
      ],
    );
  }
}
