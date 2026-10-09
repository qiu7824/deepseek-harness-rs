import 'dart:async';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../design/context_menu.dart';
import '../workbench/workbench_panel.dart'
    show NativeFileViewer, GitPanel, previewUrl;
import 'session_status.dart' show ProjectionTextEditor;
import 'artifact_types.dart';
import 'artifact_changes_view.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

String artifactName(String path) => path.replaceAll('\\', '/').split('/').last;
String artifactDisplayPath(String path) => displayPath(path);
String artifactRecordTime(Object? value) {
  if (value is! num || !value.isFinite || value <= 0 || value > 253402300799) {
    return '';
  }
  final time = DateTime.fromMillisecondsSinceEpoch(value.toInt() * 1000)
      .toLocal();
  String pad(int number) => number.toString().padLeft(2, '0');
  return '${pad(time.month)}-${pad(time.day)} ${pad(time.hour)}:${pad(time.minute)}';
}

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
  'created': DshConversationZh.added,
  'restored': '已恢复',
  'modified': DshConversationZh.modified,
  'presented': DshConversationZh.delivered,
  'deleted': DshConversationZh.removed,
  'active': DshConversationZh.running,
  'retained': DshConversationZh.retained,
  'candidate': DshConversationZh.awaitingDelivery,
  'interrupted': DshConversationZh.runInterrupted,
  'failed': DshConversationZh.failureArtifacts,
  'quarantined': DshConversationZh.recoveryQueue,
  'reclaimed': DshConversationZh.reclaimed,
};
const artifactFileActions = {
  'preview': DshConversationZh.preview,
  'diff': '查看回合差异',
  'copy': DshConversationZh.copyPath,
  'reveal': DshConversationZh.revealInFileManager,
  'open': DshConversationZh.openWithLocalTool,
  'save': DshConversationZh.saveOriginalCopy,
  'rename': DshConversationZh.rename,
  'trash': DshConversationZh.moveToTrash,
};
bool artifactCanAct(Json row, String action) {
  if (action == 'copy' || action == 'diff') return true;
  if (row['change'] == 'deleted' || row['unavailable'] == true) return false;
  return !['rename', 'trash'].contains(action) || row['etag'] is String;
}

/// One bounded request at a time, with screen/app visibility owning polling.
abstract class _PollingState<T extends StatefulWidget> extends State<T>
    with WidgetsBindingObserver {
  DshClient get api;
  String get operation;
  Json get arguments;
  RequestScope actions = RequestScope();
  RequestScope? reader;
  Timer? poll;
  Json data = {};
  Object? error;
  bool mutationFailed = false;
  bool loading = true,
      busy = false,
      working = false,
      foreground = true,
      visible = true;
  int generation = 0, failures = 0;
  bool get enabled => true;
  Object get ownerScope => api;
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
        if (!mutationFailed) error = null;
        failures = 0;
      });
    } catch (e) {
      if (active && token == generation && !mutationFailed) {
        setState(() {
          error = e;
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
    final owner = ownerScope;
    pause();
    setState(() {
      busy = true;
      working = indicate;
      error = null;
      mutationFailed = false;
    });
    try {
      await work();
    } catch (e) {
      if (mounted && ownerScope == owner) {
        setState(() {
          error = e;
          mutationFailed = true;
        });
      }
    } finally {
      if (mounted && ownerScope == owner) {
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
      if (error != null)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 8),
          child: DshErrorView(
            error: error!,
            onRetry: mutationFailed || busy
                ? null
                : () {
                    pause();
                    setState(() {
                      error = null;
                      loading = data.isEmpty;
                    });
                    load();
                  },
            onDismiss: () => setState(() {
              error = null;
              mutationFailed = false;
            }),
          ),
        ),
      if (data['warning'] != null)
        Text(
          DshError.redact('${data['warning']}'),
          style: DshTypography.body.copyWith(
            color: DshTokens.of(context).warning.foreground,
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
  bool garbage = false, workspaceRefresh = false, initialScanDone = false;
  bool sectionChosen = false;
  String section = 'all', category = 'all';
  final search = TextEditingController();
  @override
  Object get ownerScope => (api, widget.session);
  @override
  void didUpdateWidget(ArtifactsView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.api, widget.api) ||
        oldWidget.session != widget.session) {
      pause();
      actions.cancel();
      actions = RequestScope();
      data = {};
      error = null;
      loading = true;
      busy = false;
      working = false;
      mutationFailed = false;
      workspaceRefresh = false;
      initialScanDone = false;
      garbage = false;
      section = 'all';
      sectionChosen = false;
      category = 'all';
      search.clear();
      unawaited(load());
    }
  }

  @override
  void dispose() {
    search.dispose();
    super.dispose();
  }

  @override
  Future<void> load() async {
    await super.load();
    if (active && !sectionChosen && data.containsKey('entries')) {
      setState(() {
        sectionChosen = true;
        if (objects(data['entries'])
            .any((row) => row['source'] == 'delivery')) {
          section = 'delivered';
        }
      });
    }
    if (active &&
        error == null &&
        ((!initialScanDone && data['workspaceScanned'] == false) ||
            data['refreshNeeded'] == true) &&
        reader == null) {
      initialScanDone = true;
      await rescan();
    }
  }

  Future<void> rescan() async {
    if (!active || workspaceRefresh) return;
    pause();
    setState(() {
      workspaceRefresh = true;
      initialScanDone = true;
    });
    try {
      await super.load();
    } finally {
      if (mounted) setState(() => workspaceRefresh = false);
    }
  }

  @override
  DshClient get api => widget.api;
  @override
  String get operation => 'list';
  @override
  Json get arguments => {
    'sessionId': widget.session,
    'refresh': workspaceRefresh,
  };
  @override
  bool get enabled => !garbage && !['changes', 'git'].contains(section);
  Future<void> change(
    Json row,
    String action, {
    String? path,
    Object? owner,
  }) async {
    if (owner != null && ownerScope != owner) {
      throw StateError('账号或会话已变化，请重新打开文件操作');
    }
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
    initialScanDone = false;
  }

  Future<void> manage(Json row, String intent) async {
    final owner = ownerScope;
    final path = '${row['path']}';
    if (intent == 'diff') {
      await showDialog<void>(
        context: context,
        builder: (_) => Dialog(
          child: SizedBox(
            width: 1100,
            height: MediaQuery.sizeOf(context).height * .82,
            child: ArtifactChangesView(
              api: api,
              session: widget.session,
              initialPath: path,
              onClose: () => Navigator.pop(context),
            ),
          ),
        ),
      );
      return;
    }
    if (intent == 'rename') {
      await run(
        () => showDialog<void>(
          context: context,
          barrierDismissible: false,
          builder: (_) => ProjectionTextEditor(
            title: DshConversationZh.renameArtifact,
            maxLines: 1,
            width: 392,
            initial: path,
            onSave: (text) => change(row, 'rename', path: text, owner: owner),
          ),
        ),
        indicate: false,
      );
      return;
    }
    if (intent == 'trash') {
      if (!await confirmAction(
        context,
        DshConversationZh.moveToTrash,
        path,
        action: DshConversationZh.move,
      )) {
        return;
      }
      if (mounted && ownerScope == owner) {
        await run(() => change(row, 'trash', owner: owner));
      }
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
          if (target == null || !mounted || ownerScope != owner) return;
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
    final colors = DshColors(context);
    final all = objects(data['entries']);
    final query = search.text.trim().toLowerCase();
    final rows = all.where((row) {
      final path = '${row['path']}';
      return (section != 'delivered' ||
              row['source'] == 'delivery' ||
              row['change'] == 'presented') &&
          (category == 'all' || artifactCategory(path) == category) &&
          (query.isEmpty || path.toLowerCase().contains(query));
    }).toList();
    return Padding(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(child: Text('产物与改动', style: DshTypography.body)),
              if (!['changes', 'git'].contains(section))
                Text(
                  DshConversationZh.fileCount(count: rows.length),
                  style: DshTypography.caption,
                ),
              if (!['changes', 'git'].contains(section))
                DshIcon(
                  DshIcons.refreshCw.data,
                  label: '刷新工作区产物',
                  onPressed: busy || workspaceRefresh ? null : rescan,
                ),
            ],
          ),
          Wrap(
            spacing: 4,
            runSpacing: 4,
            children: [
              for (final entry in const {
                'all': '全部产物',
                'delivered': '已交付',
                'changes': '回合改动',
                'git': '工作区差异',
              }.entries)
                DshButton(
                  key: ValueKey('artifact-section-${entry.key}'),
                  height: 30,
                  active: section == entry.key,
                  fontSize: DshTypography.sizeCaption,
                  onPressed: () {
                    pause();
                    setState(() {
                      sectionChosen = true;
                      section = entry.key;
                    });
                    if (!['changes', 'git'].contains(section)) {
                      unawaited(load());
                    }
                  },
                  child: Text(entry.value),
                ),
            ],
          ),
          const SizedBox(height: 10),
          if (section == 'changes')
            Expanded(
              child: ArtifactChangesView(api: api, session: widget.session),
            )
          else if (section == 'git')
            Expanded(
              child: GitPanel(api: api, session: widget.session),
            )
          else ...[
            Row(
              children: [
                Expanded(
                  child: SizedBox(
                    height: DshTokens.of(context).controlHeight(context),
                    child: TextField(
                      key: const ValueKey('artifact-search'),
                      controller: search,
                      style: DshTypography.body,
                      decoration: const InputDecoration(
                        hintText: '搜索文件名或路径',
                        contentPadding: EdgeInsets.symmetric(
                          horizontal: 10,
                          vertical: 6,
                        ),
                      ),
                      onChanged: (_) => setState(() {
                        sectionChosen = true;
                      }),
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                PopupMenuButton<String>(
                  tooltip: '筛选产物类型',
                  onSelected: (value) => setState(() {
                    sectionChosen = true;
                    category = value;
                  }),
                  itemBuilder: (_) => [
                    for (final e in artifactCategories.entries)
                      PopupMenuItem(value: e.key, child: Text(e.value)),
                  ],
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 6,
                      vertical: 8,
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(
                          artifactCategories[category]!,
                          style: DshTypography.caption,
                        ),
                        const SizedBox(width: 4),
                        DshGlyph(DshIcons.chevronDown.data, size: 12),
                      ],
                    ),
                  ),
                ),
              ],
            ),
            notices(),
            if (workspaceRefresh) const LinearProgressIndicator(minHeight: 1),
            if (data['truncated'] == true)
              Text(
                DshConversationZh.artifactScanLimit,
                style: DshTypography.caption,
              ),
            const SizedBox(height: 8),
            Expanded(
              child: loading
                  ? const Center(
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : rows.isEmpty
                  ? DshEmpty(
                      query.isNotEmpty || category != 'all'
                          ? '没有匹配的产物'
                          : section == 'delivered'
                          ? '尚无已交付文件'
                          : '尚无产物记录',
                    )
                  : ListView.separated(
                      itemCount: rows.length,
                      separatorBuilder: (_, _) => const SizedBox(height: 6),
                      itemBuilder: (context, index) {
                        final row = rows[index],
                            path = '${rows[index]['path']}';
                        final unavailable =
                            row['change'] == 'deleted' ||
                            row['unavailable'] == true;
                        final type = artifactCategory(path);
                        final owner = ownerScope;
                        return Material(
                          color: colors.layer,
                          shape: RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(8),
                            side: BorderSide(color: colors.border),
                          ),
                          child: GestureDetector(
                            onSecondaryTapDown: busy
                                ? null
                                : (event) async {
                                    final action = await nativeContextMenu(
                                      context,
                                      event.globalPosition,
                                      {
                                        for (final entry
                                            in artifactFileActions.entries)
                                          if (artifactCanAct(row, entry.key))
                                            entry.key: entry.value,
                                      },
                                    );
                                    if (mounted &&
                                        ownerScope == owner &&
                                        action != null) {
                                      await manage(row, action);
                                    }
                                  },
                            child: ListTile(
                              key: ValueKey('artifact-file-$path'),
                              dense: true,
                              contentPadding: const EdgeInsets.symmetric(
                                horizontal: 10,
                                vertical: 4,
                              ),
                              leading: DshGlyph(
                                artifactCategoryIcon(type),
                                size: 20,
                                color: colors.muted,
                              ),
                              title: Text(
                                artifactName(path),
                                maxLines: 2,
                                overflow: TextOverflow.ellipsis,
                                style: DshTypography.body,
                              ),
                              subtitle: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Text(
                                    artifactDisplayPath(path),
                                    maxLines: 2,
                                    overflow: TextOverflow.ellipsis,
                                    style: DshTypography.caption.copyWith(
                                      color: colors.muted,
                                    ),
                                  ),
                                  const SizedBox(height: 4),
                                  Text(
                                    [
                                      artifactCategories[type]!,
                                      row['unavailable'] == true
                                          ? '暂不可读取'
                                          : artifactLabels[row['change']] ??
                                                '文件',
                                      if (row['size'] != null)
                                        artifactBytes(row['size']),
                                      if (row['source'] == 'tool') '工具操作',
                                      if (row['source'] == 'workspace') '工作区记录',
                                      if (artifactRecordTime(row['updatedAt'])
                                          .isNotEmpty)
                                        artifactRecordTime(row['updatedAt']),
                                    ].join(' · '),
                                    style: DshTypography.caption.copyWith(
                                      color: colors.muted,
                                    ),
                                  ),
                                ],
                              ),
                              onTap: busy
                                  ? null
                                  : () => manage(
                                      row,
                                      unavailable ? 'diff' : 'preview',
                                    ),
                              trailing: PopupMenuButton<String>(
                                tooltip: DshConversationZh.managePath(
                                  path: path,
                                ),
                                onSelected: (value) {
                                  if (ownerScope == owner) manage(row, value);
                                },
                                icon: DshGlyph(
                                  DshIcons.ellipsis.data,
                                  size: 16,
                                ),
                                itemBuilder: (_) => [
                                  for (final action
                                      in artifactFileActions.entries)
                                    PopupMenuItem(
                                      value: action.key,
                                      enabled:
                                          !busy &&
                                          artifactCanAct(row, action.key),
                                      child: Text(action.value),
                                    ),
                                ],
                              ),
                            ),
                          ),
                        );
                      },
                    ),
            ),
            Align(
              alignment: Alignment.centerLeft,
              child: DshButton(
                height: 30,
                onPressed: busy
                    ? null
                    : () {
                        pause();
                        setState(() => garbage = true);
                      },
                child: Text('管理临时资源与回收站', style: DshTypography.caption),
              ),
            ),
          ],
        ],
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
              DshIcons.close.data,
              label: DshConversationZh.closeArtifactPreview,
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
                    DshConversationZh.trash,
                    style: TextStyle(
                      fontSize: DshTypography.sizeBody,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  Text(
                    DshConversationZh.itemStorageSummary(
                      count: rows.length,
                      bytes: artifactBytes(total),
                    ),
                  ),
                  DshButton(
                    outline: true,
                    height: 32,
                    onPressed: busy
                        ? null
                        : () async {
                            if (!await confirmAction(
                              context,
                              DshConversationZh.cleanupReclaimable,
                              DshConversationZh.cleanupReclaimableHint,
                              action: DshConversationZh.cleanup,
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
                    child: const Text(DshConversationZh.cleanupReclaimable),
                  ),
                  if (widget.onClose != null)
                    DshButton(
                      outline: true,
                      height: 32,
                      onPressed: busy ? null : widget.onClose,
                      child: const Text(DshConversationZh.backToArtifacts),
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
                      ? const DshEmpty(DshConversationZh.noManagedMaterials)
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
                                          fontSize: DshTypography.sizeBody,
                                          fontWeight: FontWeight.w600,
                                        ),
                                      ),
                                      Text(
                                        inUse
                                            ? DshConversationZh.inUse
                                            : pinned
                                            ? DshConversationZh.pin
                                            : artifactLabels[state] ?? state,
                                        style: TextStyle(
                                          fontSize: DshTypography.sizeCaption,
                                          color: colors.muted,
                                        ),
                                      ),
                                      Text(
                                        row['sizePending'] == true
                                            ? DshConversationZh.measuringStorage
                                            : artifactBytes(row['bytes']),
                                        style: const TextStyle(
                                          fontSize: DshTypography.sizeCaption,
                                        ),
                                      ),
                                    ],
                                  ),
                                  Text(
                                    '${row['owner']}',
                                    style: TextStyle(
                                      fontSize: DshTypography.sizeCaption,
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
                                        fontSize: DshTypography.sizeCaption,
                                        color: colors.muted,
                                      ),
                                    ),
                                  ),
                                  if (row['error'] != null)
                                    DshErrorView(error: row['error']!),
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
                                            child: const Text(
                                              DshConversationZh.viewFile,
                                            ),
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
                                          child: Text(
                                            pinned
                                                ? DshConversationZh.unpin
                                                : DshConversationZh.pin,
                                          ),
                                        ),
                                      if (state == 'candidate')
                                        DshButton(
                                          outline: true,
                                          height: 30,
                                          onPressed: busy || inUse
                                              ? null
                                              : () => change(row, 'release'),
                                          child: const Text(
                                            DshConversationZh.markCompleted,
                                          ),
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
                                          child: const Text(
                                            DshConversationZh
                                                .restoreOriginalFile,
                                          ),
                                        ),
                                      if (state == 'quarantined')
                                        DshButton(
                                          outline: true,
                                          height: 30,
                                          onPressed: busy
                                              ? null
                                              : () => change(row, 'restore'),
                                          child: const Text(
                                            DshConversationZh.restoreMaterial,
                                          ),
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
                                          child: const Text(
                                            DshConversationZh.queueRecovery,
                                          ),
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
  String? text;
  Object? error;
  String requestedPath = '';
  bool requestedFile = false;
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
    final api = widget.api, resourceId = widget.id;
    bool isCurrent() =>
        mounted &&
        token == generation &&
        !current.cancelled &&
        identical(widget.api, api) &&
        widget.id == resourceId;
    setState(() {
      loading = true;
      error = null;
      text = null;
      requestedPath = path;
      requestedFile = file;
    });
    try {
      final value = await api.request(
        '/__dsh-artifacts/${file ? 'resource-read' : 'resource-files'}',
        body: {'id': resourceId, 'path': path},
        scope: current,
        maxBytes: 2 * 1024 * 1024,
      );
      if (isCurrent()) {
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
      if (isCurrent()) setState(() => error = e);
    } finally {
      if (isCurrent()) setState(() => loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final entries = objects(data['entries']);
    return AlertDialog(
      title: const Text(
        DshConversationZh.managedFiles,
        style: TextStyle(fontSize: DshTypography.sizeComposer),
      ),
      content: SizedBox(
        width: 640,
        height: MediaQuery.sizeOf(context).height * .6,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (directory.isNotEmpty)
              DshButton(
                height: 30,
                child: const Text(DshConversationZh.parentLevel),
                onPressed: () => load(
                  directory
                      .split('/')
                      .take(directory.split('/').length - 1)
                      .join('/'),
                ),
              ),
            if (error != null)
              DshErrorView(
                error: error!,
                onRetry: loading
                    ? null
                    : () => load(requestedPath, file: requestedFile),
              ),
            if (data['warning'] != null)
              Text(DshError.redact('${data['warning']}')),
            if (data['truncated'] == true)
              const Text(DshConversationZh.directoryDisplayLimit),
            if (loading) const LinearProgressIndicator(minHeight: 1),
            Expanded(
              child: text != null
                  ? SingleChildScrollView(
                      child: SelectableText(
                        text!,
                        style: TextStyle(
                          fontSize: DshTypography.sizeCaption,
                          fontFamily: DshTypography.monospaceFamily,
                          fontFamilyFallback: DshTypography.monospaceFallback,
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
                child: const Text(DshConversationZh.backToDirectory),
              ),
          ],
        ),
      ),
      actions: [
        DshButton(
          onPressed: () => Navigator.pop(context),
          child: const Text(DshConversationZh.close),
        ),
      ],
    );
  }
}
