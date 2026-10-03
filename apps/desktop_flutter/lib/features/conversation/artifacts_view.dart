import 'dart:async';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../design/context_menu.dart';
import '../workbench/workbench_panel.dart' show NativeFileViewer, previewUrl;
import 'session_status.dart' show ProjectionTextEditor;

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

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
  'created': DshConversationZh.added,
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
  Object? error;
  bool mutationFailed = false;
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
      if (mounted) {
        setState(() {
          error = e;
          mutationFailed = true;
        });
      }
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

/// Preview, save and file actions for a session's changed files.
mixin _ArtifactActions<T extends StatefulWidget> on _PollingState<T> {
  String get session;

  Future<void> change(Json row, String action, {String? path}) async {
    await api.request(
      '/__dsh-artifacts/file-action',
      body: {
        'sessionId': session,
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
            title: DshConversationZh.renameArtifact,
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
      if (!await confirmAction(
        context,
        DshConversationZh.moveToTrash,
        path,
        action: DshConversationZh.move,
      )) {
        return;
      }
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
                ArtifactPreview(api: api, session: session, path: path),
          );
          return;
        }
        if (intent == 'save') {
          final target = await getSaveLocation(
            suggestedName: artifactName(path),
          );
          if (target == null || !mounted) return;
          await api.downloadTo(
            previewUrl('file', session, {'path': path}),
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
            'sessionId': session,
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
}

class ArtifactsView extends StatefulWidget {
  const ArtifactsView({super.key, required this.api, required this.session});
  final DshClient api;
  final String session;
  @override
  State<ArtifactsView> createState() => _ArtifactsViewState();
}

class _ArtifactsViewState extends _PollingState<ArtifactsView>
    with _ArtifactActions<ArtifactsView> {
  bool garbage = false;
  @override
  DshClient get api => widget.api;
  @override
  String get session => widget.session;
  @override
  String get operation => 'list';
  @override
  Json get arguments => {'sessionId': widget.session};
  @override
  bool get enabled => !garbage;
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
                        DshConversationZh.artifacts,
                        style: TextStyle(
                          fontSize: DshTypography.sizeBody,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    Text(
                      DshConversationZh.fileCount(count: rows.length),
                      style: const TextStyle(fontSize: DshTypography.sizeBody),
                    ),
                    const SizedBox(width: 4),
                    DshIcon(
                      DshIcons.refreshCw.data,
                      label: DshConversationZh.refresh,
                      onPressed: busy ? null : load,
                    ),
                  ],
                ),
              ),
              notices(),
              if (data['truncated'] == true)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 8),
                  child: Text(
                    DshConversationZh.artifactScanLimit,
                    style: TextStyle(fontSize: DshTypography.sizeCaption),
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
                        ? const DshEmpty(DshConversationZh.noArtifactChanges)
                        : ListView.builder(
                            itemExtent: 51,
                            itemCount: rows.length,
                            itemBuilder: (context, index) => ArtifactRow(
                              row: rows[index],
                              busy: busy,
                              onAction: (intent) => manage(rows[index], intent),
                            ),
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
                    child: const Text(DshConversationZh.viewGeneratedTrash),
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

/// One changed file: what happened to it, where it lives, and its actions.
class ArtifactRow extends StatelessWidget {
  const ArtifactRow({
    super.key,
    required this.row,
    required this.busy,
    required this.onAction,
  });
  final Json row;
  final bool busy;
  final ValueChanged<String> onAction;

  static const intents = {
    'preview': DshConversationZh.preview,
    'copy': DshConversationZh.copyPath,
    'reveal': DshConversationZh.revealInFileManager,
    'open': DshConversationZh.openWithLocalTool,
    'editor': DshConversationZh.openInEditor,
    'save': DshConversationZh.saveOriginalCopy,
    'rename': DshConversationZh.rename,
    'trash': DshConversationZh.moveToTrash,
  };

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final tokens = DshTokens.of(context);
    final change = '${row['change']}';
    final deleted = change == 'deleted';
    final path = '${row['path']}';
    final shown = displayPath(path);
    final cut = shown.replaceAll('\\', '/').lastIndexOf('/');
    final folder = cut > 0 ? shown.substring(0, cut) : '';
    final tone = switch (change) {
      'created' => tokens.success,
      'modified' || 'presented' => tokens.info,
      _ => null,
    };
    final usable = !deleted && !busy;
    return Material(
      color: Colors.transparent,
      borderRadius: BorderRadius.circular(8),
      child: InkWell(
        key: ValueKey('artifact-row-$path'),
        borderRadius: BorderRadius.circular(8),
        hoverColor: colors.hover,
        onTap: usable ? () => onAction('preview') : null,
        onSecondaryTapDown: usable
            ? (event) async {
                final action = await nativeContextMenu(
                  context,
                  event.globalPosition,
                  Map.of(intents)..remove('editor'),
                );
                if (action != null) onAction(action);
              }
            : null,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(10, 7, 4, 7),
          child: Row(
            children: [
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
                decoration: BoxDecoration(
                  color: tone?.background ?? colors.layer,
                  borderRadius: BorderRadius.circular(4),
                ),
                child: Text(
                  artifactLabels[change] ?? change,
                  style: DshTypography.caption.copyWith(
                    color: tone?.foreground ?? colors.muted,
                  ),
                ),
              ),
              const SizedBox(width: 10),
              // Name and folder share one line so sizes and actions align
              // at the right edge; long folders are cut first.
              Expanded(
                child: Text.rich(
                  TextSpan(
                    children: [
                      TextSpan(
                        text: artifactName(path),
                        style: DshTypography.body.copyWith(
                          fontWeight: FontWeight.w500,
                          color: deleted ? colors.muted : colors.text,
                          decoration: deleted
                              ? TextDecoration.lineThrough
                              : null,
                        ),
                      ),
                      if (folder.isNotEmpty)
                        TextSpan(
                          text: '  $folder',
                          style: DshTypography.caption.copyWith(
                            color: colors.muted,
                          ),
                        ),
                    ],
                  ),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              const SizedBox(width: 8),
              Text(
                artifactBytes(row['size']),
                style: DshTypography.caption.copyWith(
                  color: colors.muted,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
              PopupMenuButton<String>(
                tooltip: DshConversationZh.managePath(path: path),
                enabled: !busy,
                onSelected: onAction,
                padding: EdgeInsets.zero,
                constraints: const BoxConstraints(minWidth: 200, maxWidth: 220),
                color: colors.base,
                surfaceTintColor: Colors.transparent,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(10),
                  side: BorderSide(color: colors.border.withValues(alpha: .6)),
                ),
                itemBuilder: (_) => [
                  for (final item in intents.entries)
                    PopupMenuItem(
                      height: 34,
                      value: item.key,
                      enabled: !deleted,
                      child: Text(item.value),
                    ),
                ],
                child: SizedBox(
                  width: 30,
                  height: 30,
                  child: Center(
                    child: DshGlyph(
                      DshIcons.ellipsis.data,
                      size: 16,
                      color: colors.muted,
                    ),
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

/// The files a session changed, shown below its latest reply as other agent
/// tools do. The card loads when a turn settles; the 产物 tab keeps full
/// management and the generated-trash view.
class ArtifactSummary extends StatefulWidget {
  const ArtifactSummary({
    super.key,
    required this.api,
    required this.session,
    this.onOpenAll,
  });
  final DshClient api;
  final String session;
  final VoidCallback? onOpenAll;
  @override
  State<ArtifactSummary> createState() => _ArtifactSummaryState();
}

class _ArtifactSummaryState extends _PollingState<ArtifactSummary>
    with _ArtifactActions<ArtifactSummary> {
  static const collapsedRows = 4;
  bool expanded = false;
  @override
  DshClient get api => widget.api;
  @override
  String get session => widget.session;
  @override
  String get operation => 'list';
  @override
  Json get arguments => {'sessionId': widget.session};

  // The conversation rebuilds this card for each settled turn, so it reads
  // once instead of polling beside the transcript.
  @override
  void schedule() {}

  @override
  Widget build(BuildContext context) {
    final rows = objects(data['entries']);
    if (rows.isEmpty) return const SizedBox.shrink();
    final colors = DshColors(context);
    final shown = expanded ? rows : rows.take(collapsedRows).toList();
    final hidden = rows.length - shown.length;
    return Padding(
      padding: const EdgeInsets.only(top: 4, bottom: 8),
      child: Container(
        key: const ValueKey('artifact-summary-card'),
        decoration: BoxDecoration(
          color: colors.base,
          border: Border.all(color: colors.border),
          borderRadius: BorderRadius.circular(12),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 8, 6, 8),
              child: Row(
                children: [
                  DshGlyph(DshIcons.files.data, size: 16, color: colors.muted),
                  const SizedBox(width: 8),
                  Text(
                    DshConversationZh.sessionArtifacts,
                    style: DshTypography.body.copyWith(
                      fontWeight: FontWeight.w600,
                      color: colors.text,
                    ),
                  ),
                  const SizedBox(width: 8),
                  Text(
                    DshConversationZh.fileCount(count: rows.length),
                    style: DshTypography.caption.copyWith(color: colors.muted),
                  ),
                  const Spacer(),
                  if (widget.onOpenAll != null)
                    DshButton(
                      key: const ValueKey('artifact-summary-open-all'),
                      height: 28,
                      fontSize: DshTypography.sizeCaption,
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      onPressed: widget.onOpenAll,
                      child: const Text(DshConversationZh.viewInArtifacts),
                    ),
                ],
              ),
            ),
            Divider(height: 1, color: colors.border.withValues(alpha: .6)),
            Padding(
              padding: const EdgeInsets.all(4),
              child: Column(
                children: [
                  for (final row in shown)
                    ArtifactRow(
                      row: row,
                      busy: busy,
                      onAction: (intent) => manage(row, intent),
                    ),
                ],
              ),
            ),
            if (rows.length > collapsedRows)
              InkWell(
                key: const ValueKey('artifact-summary-toggle'),
                onTap: () => setState(() => expanded = !expanded),
                borderRadius: const BorderRadius.vertical(
                  bottom: Radius.circular(12),
                ),
                child: Padding(
                  padding: const EdgeInsets.fromLTRB(14, 6, 14, 10),
                  child: Row(
                    children: [
                      Text(
                        expanded
                            ? DshConversationZh.collapseArtifacts
                            : DshConversationZh.moreArtifacts(count: hidden),
                        style: DshTypography.caption.copyWith(
                          color: colors.blue,
                        ),
                      ),
                      const SizedBox(width: 4),
                      DshGlyph(
                        expanded
                            ? DshIcons.chevronUp.data
                            : DshIcons.chevronDown.data,
                        size: 12,
                        color: colors.blue,
                      ),
                    ],
                  ),
                ),
              ),
            if (error != null || working)
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
                child: notices(),
              ),
          ],
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
