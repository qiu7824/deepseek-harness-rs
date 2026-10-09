import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:pdfrx/pdfrx.dart';
import 'package:xterm/xterm.dart';
import 'package:file_selector/file_selector.dart';
import 'package:dsh_client/dsh_client.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../design/bounded_image.dart';
import '../../src/controller.dart';
import '../../src/resource_diagnostics.dart';
import 'computer_use_panel.dart';
import 'subagent_panel.dart';
import 'plan_preview.dart';
import 'line_index.dart';
import 'reclaimable_preview.dart';
import 'start_panel.dart';
import '../conversation/artifacts_view.dart' show ArtifactsView;
import '../../l10n/workbench_zh.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

String previewUrl(String operation, String session, [Json values = const {}]) =>
    Uri(
      path: '/__dsh-preview/$operation',
      queryParameters: {
        'sessionId': session,
        for (final entry in values.entries)
          if (entry.value != null) entry.key: '${entry.value}',
      },
    ).toString();

class _WorkbenchError extends StatelessWidget {
  const _WorkbenchError({required this.error, this.onRetry, this.onDismiss});
  final Object error;
  final VoidCallback? onRetry, onDismiss;

  @override
  Widget build(BuildContext context) => ConstrainedBox(
    constraints: BoxConstraints(
      maxHeight: math.min(260, MediaQuery.sizeOf(context).height * .45),
    ),
    child: SingleChildScrollView(
      child: DshErrorView(error: error, onRetry: onRetry, onDismiss: onDismiss),
    ),
  );
}

/// A distinct request also reopens a file already selected in another tab.
class FileOpenRequest {
  const FileOpenRequest(this.path);
  final String path;
}

/// Each explicit landing-page click may create one terminal after the Host
/// has admitted the conversation. Opening a workbench tab has no such effect.
class TerminalCreateRequest {
  TerminalCreateRequest();
}

/// Preview routes take workspace-relative paths; upload receipts contain native
/// absolute paths. Keep unmatched paths intact for the Host to reject.
String workspacePreviewPath(String path, String cwd) {
  String normalize(String value) {
    value = value.replaceAll('\\', '/');
    if (value.startsWith('//?/UNC/')) return '//${value.substring(8)}';
    if (value.startsWith('//?/')) return value.substring(4);
    return value;
  }

  final target = normalize(path);
  final root = normalize(cwd).replaceFirst(RegExp(r'/+$'), '');
  if (root.isEmpty || target.split('/').contains('..')) return path;
  final windows =
      RegExp(r'^[A-Za-z]:/').hasMatch('$root/') || root.startsWith('//');
  final prefix = '$root/';
  final contained = windows
      ? target.toLowerCase().startsWith(prefix.toLowerCase())
      : target.startsWith(prefix);
  return contained ? target.substring(prefix.length) : path;
}

class WorkbenchPanel extends StatefulWidget {
  const WorkbenchPanel({
    super.key,
    required this.controller,
    required this.onClose,
    this.initialTab = 'files',
    this.openRequest = 0,
    this.onTabChanged,
    this.onTabClosed,
    this.fileRequest,
    this.onFileRequestHandled,
    this.planPreviews,
    this.onPlanSource,
    this.onOpenSettings,
    this.onOpenStartTool,
    this.onSelectWorkspace,
    this.preparingTool = false,
    this.terminalRequest,
    this.onTerminalRequestHandled,
  });
  final DesktopController controller;
  final VoidCallback onClose;
  final String initialTab;
  final int openRequest;
  final ValueChanged<String>? onTabChanged;
  final ValueChanged<String>? onTabClosed;
  final FileOpenRequest? fileRequest;
  final ValueChanged<FileOpenRequest>? onFileRequestHandled;
  final PlanPreviewStore? planPreviews;
  final VoidCallback? onPlanSource;

  /// Opens the settings page that enables Computer Use.
  final VoidCallback? onOpenSettings;
  final ValueChanged<String>? onOpenStartTool;
  final VoidCallback? onSelectWorkspace;
  final bool preparingTool;
  final TerminalCreateRequest? terminalRequest;
  final ValueChanged<TerminalCreateRequest>? onTerminalRequestHandled;
  @override
  State<WorkbenchPanel> createState() => WorkbenchPanelState();
}

class WorkbenchPanelState extends State<WorkbenchPanel>
    with ResourceDiagnosticScope {
  static const labels = {
    'start': DshWorkbenchZh.start,
    'files': DshConversationZh.file,
    'artifacts': '产物与改动',
    'git': 'Git',
    'terminal': DshConversationZh.terminal,
    'tasks': DshConversationZh.backgroundJobs,
    'team': DshConversationZh.subagents,
    'computer-use': 'Computer Use',
    'plans': DshConversationZh.planPreview,
  };
  final tabs = <String>[];
  final computerUse = GlobalKey<ComputerUsePanelState>();
  final tabAnchors = <String, GlobalKey>{};
  final tabFocusNodes = <String, FocusNode>{};
  String? tab;
  FileOpenRequest? fileRequest;
  Object? _host;
  String? _session;
  @override
  String get resourceScopeKind => 'workbench';
  @override
  Map<String, int> get resourceDiagnostics => {'workbenchTabs': tabs.length};

  @override
  void initState() {
    super.initState();
    _host = widget.controller.client;
    _session = widget.controller.selectedId;
    activateTab(widget.initialTab, notify: false);
    acceptFileRequest();
  }

  /// Keyboard navigation is scoped by the shell to the active workbench.
  void cycleTab({bool reverse = false}) {
    if (tabs.isEmpty) return;
    final next = (tabs.indexOf(tab ?? '') + (reverse ? -1 : 1)) % tabs.length;
    setState(() => activateTab(tabs[next]));
    focusActiveTab();
  }

  void focusActiveTab() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) tabFocusNodes[tab]?.requestFocus();
    });
  }

  void activateTab(String value, {bool notify = true}) {
    value = labels.containsKey(value) ? value : 'files';
    if (!tabs.contains(value)) tabs.add(value);
    tab = value;
    if (notify) widget.onTabChanged?.call(value);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || tab != value) return;
      final anchor = tabAnchors[value]?.currentContext;
      if (anchor != null) {
        unawaited(Scrollable.ensureVisible(anchor, alignment: .5));
      }
    });
  }

  void acceptFileRequest() {
    final request = widget.fileRequest;
    if (request == null) return;
    fileRequest = request;
    activateTab('files', notify: false);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) widget.onFileRequestHandled?.call(request);
    });
  }

  void close(String value) {
    final index = tabs.indexOf(value);
    if (index < 0) return;
    setState(() {
      tabs.removeAt(index);
      tabAnchors.remove(value);
      tabFocusNodes.remove(value)?.dispose();
      if (value == 'files') fileRequest = null;
      if (value == 'plans') widget.planPreviews?.clear();
      if (tab == value) {
        tab = tabs.isEmpty ? null : tabs[(index - 1).clamp(0, tabs.length - 1)];
      }
    });
    if (value == 'terminal' && widget.terminalRequest != null) {
      widget.onTerminalRequestHandled?.call(widget.terminalRequest!);
    }
    widget.onTabClosed?.call(value);
    if (tab != null) widget.onTabChanged?.call(tab!);
    focusActiveTab();
  }

  @override
  void didUpdateWidget(covariant WorkbenchPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(_host, widget.controller.client) ||
        _session != widget.controller.selectedId) {
      _host = widget.controller.client;
      _session = widget.controller.selectedId;
      tabs.clear();
      tabAnchors.clear();
      for (final node in tabFocusNodes.values) {
        node.dispose();
      }
      tabFocusNodes.clear();
      fileRequest = null;
      activateTab(widget.initialTab, notify: false);
    }
    if (oldWidget.initialTab != widget.initialTab ||
        oldWidget.openRequest != widget.openRequest) {
      activateTab(widget.initialTab, notify: false);
    }
    if (oldWidget.fileRequest != widget.fileRequest) acceptFileRequest();
  }

  @override
  void dispose() {
    for (final node in tabFocusNodes.values) {
      node.dispose();
    }
    tabFocusNodes.clear();
    super.dispose();
  }

  Widget panel(String value) {
    if (value == 'start') {
      final controller = widget.controller;
      final path = controller.currentWorkspace?['path'] as String?;
      return WorkbenchStartPanel(
        connected: controller.connected,
        hasWorkspace:
            controller.selectedId != null || (path?.trim().isNotEmpty ?? false),
        busy: widget.preparingTool,
        onOpen:
            widget.onOpenStartTool ??
            (tool) => setState(() => activateTab(tool)),
        onSelectWorkspace: widget.onSelectWorkspace,
      );
    }
    if (value == 'plans') {
      return widget.planPreviews == null
          ? const DshEmpty(DshConversationZh.planPreviewExpired)
          : PlanPreviewPanel(
              store: widget.planPreviews!,
              onSource: widget.onPlanSource ?? widget.onClose,
            );
    }
    final controller = widget.controller;
    final api = controller.client;
    final session = controller.selectedId;
    if (api == null || session == null) {
      return const DshEmpty(DshConversationZh.workbenchConnectionHint);
    }
    final key = ValueKey((api, session, value));
    return switch (value) {
      'terminal' => NativeTerminalPanel(
        key: key,
        api: api,
        session: session,
        createRequest: widget.terminalRequest,
        onCreateRequestHandled: widget.onTerminalRequestHandled,
        onInputError: (message) {
          if (controller.client == api && controller.selectedId == session) {
            controller.error = DshConversationZh.terminalInputFailed(
              error: message,
            );
            controller.emit();
          }
        },
      ),
      'git' => GitPanel(key: key, api: api, session: session),
      'artifacts' => ArtifactsView(key: key, api: api, session: session),
      'team' => SubagentPanel(key: key, api: api, parent: session),
      'tasks' => TaskPanel(key: key, controller: widget.controller),
      'computer-use' => ComputerUsePanel(
        key: computerUse,
        api: api,
        session: session,
        binding: controller.computerUse,
        onOpenSettings: widget.onOpenSettings,
      ),
      _ => FilePanel(
        key: key,
        api: api,
        session: session,
        cwd: widget.controller.selected?.cwd ?? '',
        fileRequest: fileRequest,
      ),
    };
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return ColoredBox(
      color: colors.base,
      child: Column(
        children: [
          Container(
            constraints: const BoxConstraints(minHeight: 44),
            padding: const EdgeInsets.symmetric(horizontal: 8),
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: colors.border)),
            ),
            child: Row(
              children: [
                Expanded(
                  child: SingleChildScrollView(
                    scrollDirection: Axis.horizontal,
                    child: Row(
                      children: [
                        for (final value in tabs)
                          Container(
                            key: ValueKey('workbench-tab-$value'),
                            decoration: BoxDecoration(
                              color: tab == value ? colors.layer : null,
                              border: Border(
                                bottom: BorderSide(
                                  color: tab == value
                                      ? colors.blue
                                      : Colors.transparent,
                                  width: 2,
                                ),
                              ),
                            ),
                            child: Row(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                Semantics(
                                  key: tabAnchors.putIfAbsent(
                                    value,
                                    GlobalKey.new,
                                  ),
                                  selected: tab == value,
                                  child: DshButton(
                                    key: ValueKey('workbench-select-$value'),
                                    focusNode: tabFocusNodes.putIfAbsent(
                                      value,
                                      () => FocusNode(debugLabel: 'Tool tab'),
                                    ),
                                    height: 30,
                                    padding: const EdgeInsets.symmetric(
                                      horizontal: 10,
                                    ),
                                    onPressed: () =>
                                        setState(() => activateTab(value)),
                                    child: Text(
                                      labels[value]!,
                                      style: const TextStyle(
                                        fontSize: DshTypography.sizeCaption,
                                      ),
                                    ),
                                  ),
                                ),
                                DshIcon(
                                  DshIcons.close.data,
                                  key: ValueKey('workbench-close-$value'),
                                  label: DshConversationZh.closeToolTab(
                                    title: labels[value],
                                  ),
                                  size: 24,
                                  glyphSize: 13,
                                  onPressed: () => close(value),
                                ),
                              ],
                            ),
                          ),
                      ],
                    ),
                  ),
                ),
                PopupMenuButton<String>(
                  key: const Key('workbench-add-tab'),
                  tooltip: DshConversationZh.openToolTab,
                  padding: EdgeInsets.zero,
                  icon: DshGlyph(DshIcons.plus.data, size: 16),
                  onSelected: (value) => setState(() => activateTab(value)),
                  itemBuilder: (_) => [
                    for (final entry in labels.entries)
                      if (entry.key != 'plans' ||
                          (widget.planPreviews?.items.isNotEmpty ?? false))
                        PopupMenuItem(
                          key: ValueKey('workbench-open-${entry.key}'),
                          value: entry.key,
                          enabled:
                              entry.key == 'start' ||
                              widget.controller.selectedId != null,
                          child: Row(
                            children: [
                              Expanded(child: Text(entry.value)),
                              if (tabs.contains(entry.key))
                                DshGlyph(DshIcons.check.data, size: 14),
                            ],
                          ),
                        ),
                  ],
                ),
                DshIcon(
                  DshIcons.close.data,
                  label: DshConversationZh.closeWorkbench,
                  onPressed: widget.onClose,
                ),
              ],
            ),
          ),
          Expanded(
            child: tabs.isEmpty
                ? const DshEmpty(DshConversationZh.emptyWorkbenchHint)
                : Stack(
                    fit: StackFit.expand,
                    children: [
                      for (final value in tabs)
                        Offstage(
                          key: ValueKey((
                            widget.controller.client,
                            widget.controller.selectedId,
                            value,
                          )),
                          offstage: tab != value,
                          child: TickerMode(
                            enabled: tab == value,
                            child: ExcludeFocus(
                              excluding: tab != value,
                              child: panel(value),
                            ),
                          ),
                        ),
                    ],
                  ),
          ),
        ],
      ),
    );
  }
}

class FilePanel extends StatefulWidget {
  const FilePanel({
    super.key,
    required this.api,
    required this.session,
    required this.cwd,
    this.fileRequest,
  });
  final DshClient api;
  final String session, cwd;
  final FileOpenRequest? fileRequest;
  @override
  State<FilePanel> createState() => _FilePanelState();
}

class _FilePanelState extends State<FilePanel> with ResourceDiagnosticScope {
  @override
  String get resourceScopeKind => 'file-panel';
  @override
  Map<String, int> get resourceDiagnostics => {
    'filePanels': 1,
    'openFileTabs': files.length,
    'documentCacheBytes': cache.bytes,
    'documentCacheEntries': cache.length,
    'documentCacheBudgetBytes':
        16 * 1024 * 1024 - PlanPreviewStore.maxRetainedBytes,
  };
  String directory = '', active = '';
  List<Json> entries = [];
  final files = <String>[];
  Object? error;
  bool loading = false;
  int listGeneration = 0;
  String listTarget = '';
  RequestScope scope = RequestScope();
  final cache = ResourceCache<String, String>(
    maxBytes: 16 * 1024 * 1024 - PlanPreviewStore.maxRetainedBytes,
    maxEntries: 8,
    sizeOf: (text) => text.length * 2,
  );
  final reading = <String, FilePreviewPosition>{};
  @override
  void initState() {
    super.initState();
    list('');
    if (widget.fileRequest != null) open(widget.fileRequest!.path);
  }

  @override
  void didUpdateWidget(covariant FilePanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.api, widget.api) ||
        oldWidget.session != widget.session) {
      scope.cancel();
      cache.clear();
      files.clear();
      reading.clear();
      entries = [];
      directory = active = '';
      list('');
    }
    if (widget.fileRequest != null &&
        oldWidget.fileRequest != widget.fileRequest) {
      open(widget.fileRequest!.path);
    }
  }

  @override
  void dispose() {
    scope.cancel();
    cache.clear();
    files.clear();
    reading.clear();
    super.dispose();
  }

  Future<void> list(String path) async {
    final generation = ++listGeneration;
    scope.cancel();
    final requestScope = scope = RequestScope();
    final api = widget.api, session = widget.session;
    bool current() =>
        mounted &&
        !requestScope.cancelled &&
        generation == listGeneration &&
        identical(widget.api, api) &&
        widget.session == session;
    setState(() {
      loading = true;
      error = null;
      listTarget = path;
    });
    try {
      final value = await api.request(
        previewUrl('list', session, {'path': path}),
        scope: requestScope,
        maxBytes: 2 * 1024 * 1024,
      );
      if (current()) {
        setState(() {
          entries = objects(value['entries']);
          directory = '${value['path'] ?? path}';
          error = null;
        });
      }
    } catch (e) {
      if (current()) {
        setState(() => error = e);
      }
    } finally {
      if (current()) {
        setState(() => loading = false);
      }
    }
  }

  void open(String path) {
    path = workspacePreviewPath(path, widget.cwd);
    setState(() {
      files.remove(path);
      files.add(path);
      while (files.length > 8) {
        final removed = files.removeAt(0);
        cache.remove(removed);
        reading.remove(removed);
      }
      while (reading.length > 24) {
        final oldest = reading.keys.firstWhere((name) => !files.contains(name));
        reading.remove(oldest);
      }
      active = path;
    });
  }

  void close(String path) {
    setState(() {
      files.remove(path);
      cache.remove(path);
      if (active == path) active = files.lastOrNull ?? '';
    });
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Column(
      children: [
        if (files.isNotEmpty)
          Container(
            height: 44 + (MediaQuery.textScalerOf(context).scale(12) - 12),
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: colors.border)),
            ),
            child: ListView(
              scrollDirection: Axis.horizontal,
              children: [
                for (final path in files)
                  Container(
                    color: path == active ? colors.layer : null,
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        DshButton(
                          height: 32,
                          onPressed: () => setState(() => active = path),
                          child: Text(
                            path.replaceAll('\\', '/').split('/').last,
                            style: const TextStyle(
                              fontSize: DshTypography.sizeCaption,
                            ),
                          ),
                        ),
                        DshIcon(
                          DshIcons.close.data,
                          label: DshConversationZh.closeNamedFile(
                            name: path.replaceAll('\\', '/').split('/').last,
                          ),
                          size: 23,
                          onPressed: () => close(path),
                        ),
                      ],
                    ),
                  ),
              ],
            ),
          ),
        Expanded(
          child: Row(
            children: [
              Expanded(
                child: active.isEmpty
                    ? DshEmpty(
                        DshConversationZh.selectFileHint,
                        icon: DshIcons.files.data,
                      )
                    : ReclaimablePreview(
                        key: ValueKey(active),
                        onRelease: cache.clear,
                        builder: (_) => NativeFileViewer(
                          key: ValueKey(active),
                          api: widget.api,
                          session: widget.session,
                          path: active,
                          cache: cache,
                          position: reading.putIfAbsent(
                            active,
                            FilePreviewPosition.new,
                          ),
                          onPage: (_) {},
                        ),
                      ),
              ),
              Container(
                width: 166,
                decoration: BoxDecoration(
                  color: colors.sidebar,
                  border: Border(left: BorderSide(color: colors.border)),
                ),
                child: Column(
                  children: [
                    Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 6,
                        vertical: 4,
                      ),
                      child: Row(
                        children: [
                          DshIcon(
                            DshIcons.arrowUp.data,
                            label: DshConversationZh.parentDirectory,
                            size: 26,
                            onPressed: directory.isEmpty
                                ? null
                                : () => list(
                                    directory.contains('/')
                                        ? directory.substring(
                                            0,
                                            directory.lastIndexOf('/'),
                                          )
                                        : '',
                                  ),
                          ),
                          Expanded(
                            child: Text(
                              directory.isEmpty
                                  ? DshConversationZh.workspace
                                  : directory.split('/').last,
                              style: const TextStyle(
                                fontSize: DshTypography.sizeCaption,
                              ),
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                          DshIcon(
                            DshIcons.refreshCw.data,
                            label: DshConversationZh.refreshDirectory,
                            size: 25,
                            onPressed: () => list(directory),
                          ),
                        ],
                      ),
                    ),
                    if (loading) const LinearProgressIndicator(minHeight: 1),
                    if (error != null)
                      Padding(
                        padding: const EdgeInsets.all(8),
                        child: _WorkbenchError(
                          error: error!,
                          onRetry: loading ? null : () => list(listTarget),
                        ),
                      ),
                    Expanded(
                      child: ListView.builder(
                        itemCount: entries.length,
                        itemBuilder: (context, i) {
                          final entry = entries[i],
                              folder = entries[i]['kind'] == 'directory';
                          return InkWell(
                            onTap: () => folder
                                ? list('${entry['path']}')
                                : open('${entry['path']}'),
                            child: Padding(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 8,
                                vertical: 7,
                              ),
                              child: Row(
                                children: [
                                  DshGlyph(
                                    folder
                                        ? DshIcons.folder.data
                                        : DshIcons.file.data,
                                    size: 14,
                                    color: folder ? colors.blue : colors.muted,
                                  ),
                                  const SizedBox(width: 6),
                                  Expanded(
                                    child: Text(
                                      '${entry['name']}',
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: const TextStyle(
                                        fontSize: DshTypography.sizeCaption,
                                      ),
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          );
                        },
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Lightweight view state. Contains no source text, images or document handles.
class FilePreviewPosition {
  int page = 1, line = 1, match = -1;
  double offset = 0;
  String query = '';
  bool source = false, wrap = false;
}

class NativeFileViewer extends StatefulWidget {
  const NativeFileViewer({
    super.key,
    required this.api,
    required this.session,
    required this.path,
    required this.cache,
    required this.onPage,
    this.initialPage = 1,
    this.initialLine = 1,
    this.position,
  });
  final DshClient api;
  final String session, path;
  final ResourceCache<String, String> cache;
  final ValueChanged<int> onPage;
  final int initialPage;
  final int initialLine;
  final FilePreviewPosition? position;
  @override
  State<NativeFileViewer> createState() => _NativeFileViewerState();
}

class _NativeFileViewerState extends State<NativeFileViewer>
    with ResourceDiagnosticScope {
  static const pdfCacheBudget = 24 * 1024 * 1024;
  @override
  String get resourceScopeKind => 'file-viewer';
  @override
  Map<String, int> get resourceDiagnostics => {
    'fileViewers': 1,
    'fileBinaryBytes': binary?.length ?? 0,
    'pdfViewers': pdf && binary != null ? 1 : 0,
    'imageViewers': image && binary != null ? 1 : 0,
    'pdfImageCacheBudgetBytes': pdf && binary != null ? pdfCacheBudget : 0,
    'indexedLinesBytes': lines.retainedBytes,
    'fileTextUnits': text?.length ?? 0,
    'fileRequests': loading && !scope.cancelled ? 1 : 0,
  };
  RequestScope scope = RequestScope(), exportScope = RequestScope();
  final search = TextEditingController(), lineInput = TextEditingController();
  final scroll = ScrollController();
  String? text;
  Object? error;
  Uint8List? binary;
  LineIndex lines = LineIndex.empty();
  bool source = false, wrap = false, loading = true, saving = false;
  final pdfController = PdfViewerController();
  late FilePreviewPosition position;
  int matchIndex = -1, loadGeneration = 0;
  String get ext => widget.path.split('.').last.toLowerCase();
  bool get pdf =>
      ['pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx'].contains(ext);
  bool get image => ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'].contains(ext);
  @override
  void initState() {
    super.initState();
    restorePosition();
    search.addListener(() => position.query = search.text);
    scroll.addListener(() {
      position.offset = scroll.offset;
      if ((source || ext != 'md') && !wrap) {
        position.line = (scroll.offset / lineHeight).floor() + 1;
      }
    });
    load();
  }

  void restorePosition() {
    position =
        widget.position ??
        (FilePreviewPosition()
          ..page = widget.initialPage
          ..line = widget.initialLine
          ..source = widget.initialLine > 1);
    source = position.source;
    wrap = position.wrap;
    matchIndex = position.match;
    search.text = position.query;
  }

  @override
  void didUpdateWidget(covariant NativeFileViewer oldWidget) {
    super.didUpdateWidget(oldWidget);
    final ownerChanged =
        !identical(oldWidget.api, widget.api) ||
        oldWidget.session != widget.session;
    if (ownerChanged || oldWidget.path != widget.path) {
      exportScope.cancel();
      exportScope = RequestScope();
      saving = false;
      if (ownerChanged && identical(oldWidget.cache, widget.cache)) {
        widget.cache.clear();
      }
      binary = null;
      text = null;
      lines = LineIndex.empty();
      restorePosition();
      load();
    }
  }

  @override
  void dispose() {
    scope.cancel();
    exportScope.cancel();
    search.dispose();
    lineInput.dispose();
    scroll.dispose();
    binary = null;
    lines = LineIndex.empty();
    text = null;
    super.dispose();
  }

  Future<void> load() async {
    scope.cancel();
    final requestScope = scope = RequestScope();
    final generation = ++loadGeneration;
    final api = widget.api, session = widget.session, path = widget.path;
    bool current() =>
        mounted &&
        !requestScope.cancelled &&
        generation == loadGeneration &&
        identical(widget.api, api) &&
        widget.session == session &&
        widget.path == path;
    setState(() {
      loading = true;
      error = null;
    });
    try {
      if (pdf || image) {
        final office = ext != 'pdf' && pdf;
        final loaded = await api.bytes(
          office
              ? '/__dsh-preview/office'
              : previewUrl('file', session, {'path': path}),
          body: office ? {'sessionId': session, 'path': path} : null,
          scope: requestScope,
          maxBytes: 16 * 1024 * 1024,
        );
        if (!current()) return;
        binary = loaded;
      } else {
        text = widget.cache.get(path);
        if (text == null) {
          final result = await api.request(
            previewUrl('source', session, {'path': path}),
            scope: requestScope,
            maxBytes: 8 * 1024 * 1024,
          );
          if (!current()) return;
          text = '${result['text'] ?? ''}';
          widget.cache.put(path, text!);
        }
        lines = LineIndex.fromText(text!);
        if (matchIndex < 0 && position.line > 1 && lines.isNotEmpty) {
          matchIndex = (position.line - 1).clamp(0, lines.length - 1);
        }
      }
      if (current()) setState(() => loading = false);
      if (current()) {
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (current() && scroll.hasClients) {
            scroll.jumpTo(
              (position.offset > 0
                      ? position.offset
                      : math.max(0, matchIndex) * lineHeight)
                  .clamp(0, scroll.position.maxScrollExtent),
            );
          }
        });
      }
    } catch (e) {
      if (current()) {
        setState(() {
          error = e;
          loading = false;
        });
      }
    }
  }

  Future<void> export() async {
    if (saving) return;
    final api = widget.api, session = widget.session, path = widget.path;
    final requestScope = exportScope;
    bool current() =>
        mounted &&
        !requestScope.cancelled &&
        identical(widget.api, api) &&
        widget.session == session &&
        widget.path == path;
    setState(() => saving = true);
    try {
      final name = path.replaceAll('\\', '/').split('/').last;
      final converted = pdf && ext != 'pdf';
      final target = await getSaveLocation(
        suggestedName: converted
            ? '${name.substring(0, name.lastIndexOf('.'))}.pdf'
            : name,
      );
      if (target == null || !current()) return;
      if (converted) {
        if (binary == null) throw StateError(DshConversationZh.pdfNotGenerated);
        await XFile.fromData(binary!).saveTo(target.path);
      } else {
        await api.downloadTo(
          previewUrl('file', session, {'path': path}),
          File(target.path),
          scope: requestScope,
        );
      }
    } catch (e) {
      if (mounted && current()) {
        showDshError(context, e, operation: DshConversationZh.exportPdf);
      }
    } finally {
      if (current()) setState(() => saving = false);
    }
  }

  void findNext() {
    if (search.text.isEmpty || lines.isEmpty) return;
    for (var i = 1; i <= lines.length; i++) {
      final index = (matchIndex + i) % lines.length;
      if (lines[index].toLowerCase().contains(search.text.toLowerCase())) {
        setState(() => position.match = matchIndex = index);
        if (scroll.hasClients) {
          scroll.jumpTo(
            (index * lineHeight).clamp(0, scroll.position.maxScrollExtent),
          );
        }
        break;
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          constraints: const BoxConstraints(minHeight: 44),
          padding: const EdgeInsets.symmetric(horizontal: 6),
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: colors.border)),
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  widget.path.replaceAll('\\', '/').split('/').last,
                  style: const TextStyle(fontSize: DshTypography.sizeCaption),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              if (text != null) ...[
                DshIcon(
                  DshIcons.code.data,
                  asset: 'assets/icons/source-code.svg',
                  label: source
                      ? DshConversationZh.preview
                      : DshConversationZh.source,
                  active: source,
                  size: 25,
                  onPressed: () =>
                      setState(() => position.source = source = !source),
                ),
                DshIcon(
                  DshIcons.wrapText.data,
                  label: DshConversationZh.wrapLines,
                  active: wrap,
                  size: 25,
                  onPressed: () => setState(() => position.wrap = wrap = !wrap),
                ),
                DshIcon(
                  DshIcons.copy.data,
                  label: DshConversationZh.copyFileContent,
                  size: 25,
                  onPressed: () =>
                      Clipboard.setData(ClipboardData(text: text!)),
                ),
              ],
              DshIcon(
                DshIcons.download.data,
                label: pdf && ext != 'pdf'
                    ? DshConversationZh.exportPdf
                    : DshConversationZh.saveOriginalCopy,
                size: 25,
                onPressed: loading || saving ? null : export,
              ),
            ],
          ),
        ),
        if (text != null)
          Padding(
            padding: const EdgeInsets.all(6),
            child: Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: search,
                    onSubmitted: (_) => findNext(),
                    style: const TextStyle(fontSize: DshTypography.sizeCaption),
                    decoration: const InputDecoration(
                      isDense: true,
                      hintText: DshConversationZh.findContent,
                      border: OutlineInputBorder(),
                      contentPadding: EdgeInsets.all(8),
                    ),
                  ),
                ),
                DshIcon(
                  DshIcons.search.data,
                  label: DshConversationZh.findNext,
                  size: 28,
                  onPressed: findNext,
                ),
              ],
            ),
          ),
        Expanded(
          child: loading
              ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
              : error != null
              ? SingleChildScrollView(
                  padding: const EdgeInsets.all(12),
                  child: DshErrorView(error: error!, onRetry: load),
                )
              : pdf
              ? PdfViewer.data(
                  binary!,
                  // pdfrx caches references by sourceName. An anonymous view
                  // identity prevents reuse across Hosts or refreshed downloads.
                  sourceName: 'file-preview-$resourceScopeId',
                  controller: pdfController,
                  initialPageNumber: position.page,
                  // The Host-authorized bytes are the sole complete buffer;
                  // native reads do not duplicate the full PDF in memory.
                  maxSizeToCacheOnMemory: 0,
                  params: PdfViewerParams(
                    maxImageBytesCachedOnMemory: pdfCacheBudget,
                    verticalCacheExtent: 0.3,
                    horizontalCacheExtent: 0.0,
                    onPageChanged: (page) {
                      if (page != null) {
                        position.page = page;
                        widget.onPage(page);
                      }
                    },
                  ),
                )
              : image
              ? InteractiveViewer(
                  minScale: .1,
                  maxScale: 5,
                  child: Center(
                    child: DshBoundedImage(
                      image: MemoryImage(binary!),
                      evictOnDispose: true,
                      filterQuality: FilterQuality.medium,
                      errorBuilder: (_, e, _) => DshEmpty('$e'),
                    ),
                  ),
                )
              : ext == 'md' && !source
              ? SingleChildScrollView(
                  controller: scroll,
                  padding: const EdgeInsets.all(18),
                  child: MarkdownBody(data: text!, selectable: true),
                )
              : codeView(colors),
        ),
      ],
    );
  }

  double get lineHeight => MediaQuery.textScalerOf(context).scale(13) * 1.6;

  Widget codeView(DshColors colors) => ListView.builder(
    controller: scroll,
    itemExtent: wrap ? null : lineHeight,
    itemCount: lines.length,
    itemBuilder: (context, i) => Container(
      color: i == matchIndex ? colors.blue.withValues(alpha: .12) : null,
      padding: const EdgeInsets.only(right: 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: math.max(
              48,
              lines.length.toString().length *
                      MediaQuery.textScalerOf(context).scale(13) *
                      .7 +
                  12,
            ),
            child: Text(
              '${i + 1}',
              textAlign: TextAlign.right,
              style: TextStyle(
                fontFamily: DshTypography.monospaceFamily,
                fontFamilyFallback: DshTypography.monospaceFallback,
                fontSize: DshTypography.sizeAuxiliary,
                height: 1.6,
                color: colors.muted,
              ),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: SelectableText(
              lines[i],
              maxLines: wrap ? null : 1,
              style: TextStyle(
                fontFamily: DshTypography.monospaceFamily,
                fontFamilyFallback: DshTypography.monospaceFallback,
                fontSize: DshTypography.sizeAuxiliary,
                height: 1.6,
                color: colors.text,
              ),
            ),
          ),
        ],
      ),
    ),
  );
}

class GitPanel extends StatefulWidget {
  const GitPanel({super.key, required this.api, required this.session});
  final DshClient api;
  final String session;
  @override
  State<GitPanel> createState() => _GitPanelState();
}

class _GitPanelState extends State<GitPanel> {
  RequestScope statusScope = RequestScope(), diffScope = RequestScope();
  Json? status;
  String? path;
  Object? error;
  List<String> diff = [];
  bool busy = false, staged = false, diffLoading = false, retryDiff = false;
  int diffGeneration = 0, statusGeneration = 0, errorGeneration = 0;
  @override
  void initState() {
    super.initState();
    load();
  }

  @override
  void dispose() {
    statusScope.cancel();
    diffScope.cancel();
    diff = [];
    super.dispose();
  }

  @override
  void didUpdateWidget(covariant GitPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.api, widget.api) ||
        oldWidget.session != widget.session) {
      diffScope.cancel();
      diffGeneration++;
      diffLoading = false;
      status = null;
      path = null;
      diff = [];
      load();
    }
  }

  Future<void> load() async {
    statusScope.cancel();
    final requestScope = statusScope = RequestScope();
    final generation = ++statusGeneration;
    final errorToken = ++errorGeneration;
    final api = widget.api, session = widget.session;
    bool current() =>
        mounted &&
        !requestScope.cancelled &&
        generation == statusGeneration &&
        identical(widget.api, api) &&
        widget.session == session;
    setState(() {
      busy = true;
      error = null;
      retryDiff = false;
    });
    try {
      final result = await api.request(
        previewUrl('git-status', session),
        scope: requestScope,
      );
      if (current()) {
        setState(() {
          status = result;
          if (errorToken == errorGeneration) error = null;
        });
      }
    } catch (e) {
      if (current() && errorToken == errorGeneration) setState(() => error = e);
    } finally {
      if (current()) setState(() => busy = false);
    }
  }

  Future<void> select(Json entry) async {
    diffScope.cancel();
    final requestScope = diffScope = RequestScope();
    final generation = ++diffGeneration;
    final errorToken = ++errorGeneration;
    final api = widget.api, session = widget.session;
    final requestedPath = '${entry['path']}';
    final requestedStaged = entry['group'] == 'staged';
    setState(() {
      path = requestedPath;
      staged = requestedStaged;
      diff = [];
      error = null;
      diffLoading = true;
      retryDiff = true;
    });
    bool current() =>
        mounted &&
        !requestScope.cancelled &&
        identical(widget.api, api) &&
        widget.session == session &&
        generation == diffGeneration &&
        path == requestedPath &&
        staged == requestedStaged;
    try {
      final result = await api.request(
        previewUrl('git-diff', session, {
          'path': requestedPath,
          'staged': requestedStaged ? '1' : '0',
        }),
        scope: requestScope,
        maxBytes: 8 * 1024 * 1024,
      );
      if (current()) {
        setState(() => diff = '${result['diff'] ?? ''}'.split('\n'));
      }
    } catch (e) {
      if (current() && errorToken == errorGeneration) setState(() => error = e);
    } finally {
      if (current()) setState(() => diffLoading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.all(8),
          child: Row(
            children: [
              DshGlyph(DshIcons.gitBranch.data, size: 15, color: colors.muted),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  '${status?['branch'] ?? 'Git'}',
                  style: const TextStyle(fontSize: DshTypography.sizeCaption),
                ),
              ),
              DshIcon(
                DshIcons.refreshCw.data,
                label: DshConversationZh.refreshChanges,
                onPressed: load,
              ),
            ],
          ),
        ),
        if (busy || diffLoading) const LinearProgressIndicator(minHeight: 1),
        if (error != null)
          Padding(
            padding: const EdgeInsets.all(10),
            child: _WorkbenchError(
              error: error!,
              onRetry: busy || diffLoading
                  ? null
                  : () {
                      if (retryDiff && path != null) {
                        select({
                          'path': path,
                          'group': staged ? 'staged' : 'unstaged',
                        });
                      } else {
                        load();
                      }
                    },
            ),
          ),
        Expanded(
          child: Row(
            children: [
              Expanded(
                child: path == null
                    ? DshEmpty(
                        DshConversationZh.selectChangeHint,
                        icon: DshIcons.gitCompareArrows.data,
                      )
                    : Column(
                        children: [
                          Padding(
                            padding: const EdgeInsets.all(8),
                            child: Text(
                              path!,
                              style: const TextStyle(
                                fontSize: DshTypography.sizeCaption,
                              ),
                            ),
                          ),
                          Expanded(
                            child: ListView.builder(
                              itemCount: diff.length,
                              itemBuilder: (context, i) {
                                final line = diff[i];
                                final color = line.startsWith('+')
                                    ? colors.success
                                    : line.startsWith('-')
                                    ? colors.error
                                    : line.startsWith('@@')
                                    ? colors.blue
                                    : null;
                                return Container(
                                  color: color?.withValues(alpha: .10),
                                  padding: const EdgeInsets.symmetric(
                                    horizontal: 10,
                                    vertical: 1,
                                  ),
                                  child: SelectableText(
                                    line,
                                    style: TextStyle(
                                      fontFamily: DshTypography.monospaceFamily,
                                      fontFamilyFallback:
                                          DshTypography.monospaceFallback,
                                      fontSize: DshTypography.sizeCaption,
                                      color: color ?? colors.text,
                                    ),
                                  ),
                                );
                              },
                            ),
                          ),
                        ],
                      ),
              ),
              Container(
                width: 158,
                decoration: BoxDecoration(
                  border: Border(left: BorderSide(color: colors.border)),
                ),
                child: ListView(
                  children: [
                    for (final entry in objects(status?['entries']))
                      ListTile(
                        dense: true,
                        contentPadding: const EdgeInsets.symmetric(
                          horizontal: 8,
                        ),
                        title: Text(
                          displayPath('${entry['path']}'),
                          style: const TextStyle(
                            fontSize: DshTypography.sizeCaption,
                          ),
                        ),
                        subtitle: Text(
                          '${entry['status']}',
                          style: const TextStyle(
                            fontSize: DshTypography.sizeCaption,
                          ),
                        ),
                        selected: path == entry['path'],
                        onTap: () => select(entry),
                      ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Host terminal input is capped at 8 KiB per request, including UTF-8 bytes.
Iterable<String> terminalInputChunks(String text) sync* {
  var chunk = StringBuffer(), bytes = 0;
  for (final rune in text.runes) {
    final size = rune <= 0x7f
        ? 1
        : rune <= 0x7ff
        ? 2
        : rune <= 0xffff
        ? 3
        : 4;
    if (bytes + size > 8192) {
      yield chunk.toString();
      chunk = StringBuffer();
      bytes = 0;
    }
    chunk.writeCharCode(rune);
    bytes += size;
  }
  if (bytes > 0) yield chunk.toString();
}

/// Reconcile one bounded Host page using metadata from that same response.
/// A missing overlap requires a fresh snapshot, never an uncertain append.
class TerminalOutputWindow {
  int? _total;
  String _tail = '';
  List<String> _anchor = [];
  bool get initialized => _total != null;

  String? apply(Json page, {bool reset = false}) {
    final total = (page['totalLines'] as num?)?.toInt();
    final text = page['text'];
    if (total == null || total < 0 || text is! String) {
      throw const FormatException(DshConversationZh.terminalOutputInvalid);
    }
    final lines = text.split('\n');
    String append;
    if (_total == null || reset) {
      append = '\x1bc$text';
    } else {
      final added = total - _total!;
      if (added < 0 || added >= lines.length) return null;
      final index = lines.length - added - 1;
      final overlap = index < _anchor.length ? index : _anchor.length;
      if (added == 0 && _anchor.isNotEmpty && overlap == 0) return null;
      for (var i = 0; i < overlap; i++) {
        if (lines[index - overlap + i] !=
            _anchor[_anchor.length - overlap + i]) {
          return null;
        }
      }
      final overlapping = lines[index];
      if (!overlapping.startsWith(_tail)) return null;
      append = overlapping.substring(_tail.length);
      if (added > 0) {
        append += '\n${lines.skip(lines.length - added).join('\n')}';
      }
    }
    _total = total;
    _tail = lines.last;
    _anchor = lines.sublist(
      (lines.length - 9).clamp(0, lines.length - 1),
      lines.length - 1,
    );
    // The Host returns logical lines with CRLF normalized to LF.
    return append.replaceAll('\n', '\r\n');
  }
}

class NativeTerminalPanel extends StatefulWidget {
  const NativeTerminalPanel({
    super.key,
    required this.api,
    required this.session,
    this.onInputError,
    this.createRequest,
    this.onCreateRequestHandled,
  });
  final DshClient api;
  final String session;
  final ValueChanged<String>? onInputError;
  final TerminalCreateRequest? createRequest;
  final ValueChanged<TerminalCreateRequest>? onCreateRequestHandled;
  @override
  State<NativeTerminalPanel> createState() => _NativeTerminalPanelState();
}

class _NativeTerminalPanelState extends State<NativeTerminalPanel>
    with WidgetsBindingObserver, ResourceDiagnosticScope {
  @override
  String get resourceScopeKind => 'terminal-view';
  static final inputTails = <(DshClient, String, String), Future<void>>{};
  @override
  Map<String, int> get resourceDiagnostics => {
    'terminalPanels': 1,
    'terminalBufferLines': terminal.buffer.lines.length,
    'terminalQueuedInputUnits': pendingInput.length + queuedInputLength,
    'terminalInputOwners': inputTails.keys
        .where((key) => identical(key.$1, inputApi) && key.$2 == inputSession)
        .length,
    'terminalPollTimers': timer?.isActive == true ? 1 : 0,
    'terminalResizeTimers': resizeTimer?.isActive == true ? 1 : 0,
    'terminalInputTimers': inputTimer?.isActive == true ? 1 : 0,
    'terminalPollRequests': pollingEpoch == null ? 0 : 1,
    'terminalListRequests': listing && !listScope.cancelled ? 1 : 0,
  };
  late Terminal terminal;
  final scope = RequestScope();
  final inputScope = RequestScope();
  late final inputApi = widget.api;
  late final inputSession = widget.session;
  RequestScope readScope = RequestScope();
  RequestScope listScope = RequestScope();
  RequestScope? pollingScope;
  List<Json> entries = [];
  String? active, failedRead;
  Object? error;
  Timer? timer, resizeTimer, inputTimer;
  bool panelVisible = true, appVisible = true;
  bool listing = false;
  bool get visible => panelVisible && appVisible;
  int? pollingEpoch;
  int listGeneration = 0, queuedInputLength = 0;
  Future<void> inputWrites = Future.value();
  TerminalOutputWindow output = TerminalOutputWindow();
  String pendingInput = '';
  int epoch = 0;
  TerminalCreateRequest? handledCreateRequest;
  @override
  void initState() {
    super.initState();
    final lifecycle = WidgetsBinding.instance.lifecycleState;
    appVisible = lifecycle == null || lifecycle == AppLifecycleState.resumed;
    WidgetsBinding.instance.addObserver(this);
    terminal = makeTerminal();
    if (widget.createRequest == null) {
      load();
    } else {
      unawaited(createRequestedTerminal());
    }
  }

  @override
  void didUpdateWidget(covariant NativeTerminalPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.createRequest != oldWidget.createRequest &&
        widget.createRequest != null) {
      unawaited(createRequestedTerminal());
    }
  }

  Future<void> createRequestedTerminal() async {
    final request = widget.createRequest;
    if (request == null || identical(request, handledCreateRequest)) return;
    handledCreateRequest = request;
    await load();
    if (!mounted ||
        scope.cancelled ||
        !visible ||
        !identical(widget.createRequest, request)) {
      return;
    }
    widget.onCreateRequestHandled?.call(request);
    if (failedRead != 'list') await open();
  }

  Terminal makeTerminal() {
    final value = Terminal(maxLines: 3000);
    configureTerminal(value);
    return value;
  }

  void configureTerminal(Terminal value) {
    final owner = active, generation = epoch;
    value.onOutput = (data) {
      if (!mounted ||
          !visible ||
          owner == null ||
          owner != active ||
          generation != epoch) {
        return;
      }
      if (queuedInputLength + pendingInput.length + data.length > 65536) {
        setState(() {
          error = DshConversationZh.terminalInputBacklog;
          failedRead = null;
        });
        return;
      }
      pendingInput += data;
      inputTimer ??= Timer(const Duration(milliseconds: 15), flushInput);
    };
    value.onResize = (cols, rows, _, _) {
      if (owner == null || generation != epoch || !mounted || !visible) return;
      resizeTimer?.cancel();
      resizeTimer = Timer(
        const Duration(milliseconds: 120),
        () => action(
          'resize',
          extra: {'cols': cols, 'rows': rows},
          terminalId: owner,
          requestEpoch: generation,
        ),
      );
    };
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    updateVisibility(app: state == AppLifecycleState.resumed);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    updateVisibility(panel: TickerMode.valuesOf(context).enabled);
  }

  void updateVisibility({bool? app, bool? panel}) {
    final wasVisible = visible;
    appVisible = app ?? appVisible;
    panelVisible = panel ?? panelVisible;
    if (visible == wasVisible) return;
    if (!visible) {
      unawaited(flushInput());
      timer?.cancel();
      readScope.cancel();
      listScope.cancel();
      listing = false;
      epoch++;
      resizeTimer?.cancel();
    } else {
      readScope = RequestScope();
      configureTerminal(terminal);
      load();
      poll();
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    // Accepted keystrokes stay bound to their original terminal even when the
    // view closes before the batching delay or an earlier write completes.
    unawaited(flushInput().whenComplete(inputScope.cancel));
    timer?.cancel();
    resizeTimer?.cancel();
    inputTimer?.cancel();
    scope.cancel();
    readScope.cancel();
    listScope.cancel();
    terminal.onOutput = null;
    terminal.onResize = null;
    pendingInput = '';
    super.dispose();
  }

  Future<void> flushInput() async {
    inputTimer?.cancel();
    inputTimer = null;
    final text = pendingInput, owner = active, generation = epoch;
    pendingInput = '';
    if (text.isEmpty) return inputWrites;
    if (owner == null || inputScope.cancelled) return;
    queuedInputLength += text.length;
    final target = (inputApi, inputSession, owner);
    final previousLocal = inputWrites, previousOwner = inputTails[target];
    late final Future<void> write;
    // A reopened view joins the same terminal's existing write tail. Each job
    // only awaits tails captured before publication, so dependencies cannot cycle.
    write =
        Future.wait<void>([
              previousLocal,
              if (previousOwner != null &&
                  !identical(previousOwner, previousLocal))
                previousOwner,
            ])
            .then((_) async {
              try {
                if (!inputScope.cancelled) {
                  for (final chunk in terminalInputChunks(text)) {
                    if (inputScope.cancelled ||
                        !await action(
                          'input',
                          extra: {'text': chunk},
                          terminalId: owner,
                          requestEpoch: generation,
                        )) {
                      break;
                    }
                  }
                }
              } finally {
                queuedInputLength -= text.length;
              }
            })
            .whenComplete(() {
              if (identical(inputTails[target], write)) {
                inputTails.remove(target);
              }
            });
    inputWrites = write;
    inputTails[target] = write;
    await write;
  }

  Future<bool> action(
    String action, {
    Json extra = const {},
    String? terminalId,
    int? requestEpoch,
  }) async {
    final owner = terminalId ?? active, generation = requestEpoch ?? epoch;
    final requestScope = action == 'input' ? inputScope : scope;
    if (owner == null || requestScope.cancelled) return false;
    try {
      await (action == 'input' ? inputApi : widget.api).request(
        '/__dsh-preview/terminal-action',
        body: {
          'sessionId': action == 'input' ? inputSession : widget.session,
          'terminalId': owner,
          'action': action,
          ...extra,
        },
        mutation: true,
        scope: requestScope,
      );
      return true;
    } catch (e) {
      if (action == 'input' && !mounted && !inputScope.cancelled) {
        inputScope.cancel();
        widget.onInputError?.call(DshError.redact('$e'));
      }
      if (mounted &&
          generation == epoch &&
          active == owner &&
          !requestScope.cancelled) {
        setState(() {
          error = e;
          failedRead = null;
        });
      }
      return false;
    }
  }

  Future<void> load() async {
    if (!visible || scope.cancelled) return;
    final generation = ++listGeneration;
    listScope.cancel();
    final requestScope = listScope = RequestScope();
    final api = widget.api, session = widget.session;
    bool current() =>
        mounted &&
        !requestScope.cancelled &&
        generation == listGeneration &&
        identical(widget.api, api) &&
        widget.session == session;
    setState(() => listing = true);
    try {
      final value = await api.request(
        previewUrl('terminal-list', session),
        scope: requestScope,
      );
      if (!current()) return;
      setState(() {
        entries = objects(value['entries']);
        if (failedRead == 'list') {
          error = null;
          failedRead = null;
        }
      });
      if (active == null && entries.isNotEmpty) {
        select('${entries.first['id']}');
      }
    } catch (e) {
      if (current() && (error == null || failedRead != null)) {
        setState(() {
          error = e;
          failedRead = 'list';
        });
      }
    } finally {
      if (current()) setState(() => listing = false);
    }
  }

  Future<void> retryReads() async {
    if (!visible) return;
    timer?.cancel();
    readScope.cancel();
    readScope = RequestScope();
    setState(() {
      error = null;
      failedRead = null;
    });
    await load();
    if (mounted && visible) await poll();
  }

  void select(String? id) {
    if (id == active) return;
    unawaited(flushInput());
    timer?.cancel();
    resizeTimer?.cancel();
    readScope.cancel();
    readScope = RequestScope();
    epoch++;
    active = id;
    error = null;
    failedRead = null;
    output = TerminalOutputWindow();
    terminal.onOutput = null;
    terminal.onResize = null;
    terminal = makeTerminal();
    setState(() {});
    poll();
  }

  Future<void> open() async {
    if (entries.length >= 3) {
      setState(() {
        error = DshConversationZh.terminalCountLimit;
        failedRead = null;
      });
      return;
    }
    final generation = epoch;
    try {
      final value = await widget.api.request(
        '/__dsh-preview/terminal-action',
        body: {
          'sessionId': widget.session,
          'action': 'open',
          'name': DshConversationZh.terminalName(index: entries.length + 1),
        },
        mutation: true,
        scope: scope,
      );
      await load();
      if (mounted && generation == epoch) select('${value['id']}');
    } catch (e) {
      if (mounted && generation == epoch) {
        setState(() {
          error = e;
          failedRead = null;
        });
      }
    }
  }

  Future<void> closeTerminal() async {
    final owner = active, generation = epoch;
    if (owner == null) return;
    await flushInput();
    final closed = await action(
      'close',
      terminalId: owner,
      requestEpoch: generation,
    );
    if (!mounted || !closed) return;
    if (epoch == generation && active == owner) select(null);
    await load();
    if (mounted) setState(() {});
  }

  Future<void> poll() async {
    timer?.cancel();
    if ((pollingEpoch == epoch && identical(pollingScope, readScope)) ||
        active == null ||
        !visible ||
        scope.cancelled) {
      return;
    }
    pollingEpoch = epoch;
    final generation = epoch, id = active!;
    final requestScope = pollingScope = readScope;
    final api = widget.api, session = widget.session;
    bool current() =>
        mounted &&
        !requestScope.cancelled &&
        generation == epoch &&
        identical(readScope, requestScope) &&
        identical(widget.api, api) &&
        widget.session == session;
    try {
      final page = await api.request(
        previewUrl('terminal-read', session, {
          'terminalId': id,
          'count': output.initialized ? 64 : 2000,
        }),
        scope: requestScope,
        maxBytes: 1024 * 1024,
      );
      if (!current()) return;
      var update = output.apply(page);
      if (update == null) {
        final snapshot = await api.request(
          previewUrl('terminal-read', session, {
            'terminalId': id,
            'count': 2000,
          }),
          scope: requestScope,
          maxBytes: 1024 * 1024,
        );
        if (!current()) return;
        update = output.apply(snapshot, reset: true)!;
      }
      terminal.write(update);
      if (failedRead == 'output') {
        setState(() {
          error = null;
          failedRead = null;
        });
      }
    } catch (e) {
      if (current() && (error == null || failedRead != null)) {
        setState(() {
          error = e;
          failedRead = 'output';
        });
      }
    } finally {
      if (pollingEpoch == generation && identical(pollingScope, requestScope)) {
        pollingEpoch = null;
        pollingScope = null;
      }
      if (current() && visible) {
        timer = Timer(Duration(milliseconds: error == null ? 450 : 3000), poll);
      }
    }
  }

  @override
  Widget build(BuildContext context) => ColoredBox(
    color: const Color(0xff101318),
    child: Column(
      children: [
        SizedBox(
          height: 38,
          child: Row(
            children: [
              Expanded(
                child: ListView(
                  scrollDirection: Axis.horizontal,
                  children: [
                    for (final entry in entries)
                      TextButton(
                        onPressed: () => select('${entry['id']}'),
                        child: Text(
                          '${entry['name'] ?? entry['id']}',
                          style: TextStyle(
                            fontSize: DshTypography.sizeCaption,
                            color: entry['id'] == active
                                ? Colors.white
                                : Colors.grey,
                          ),
                        ),
                      ),
                  ],
                ),
              ),
              IconButton(
                tooltip: DshConversationZh.newTerminal,
                onPressed: open,
                icon: DshGlyph(
                  DshIcons.plus.data,
                  size: 16,
                  color: Colors.white,
                ),
              ),
              IconButton(
                tooltip: DshConversationZh.closeCurrentTerminal,
                onPressed: active == null ? null : closeTerminal,
                icon: DshGlyph(
                  DshIcons.close.data,
                  size: 16,
                  color: Colors.white,
                ),
              ),
            ],
          ),
        ),
        if (listing) const LinearProgressIndicator(minHeight: 1),
        if (error != null)
          Padding(
            padding: const EdgeInsets.all(8),
            child: _WorkbenchError(
              error: error!,
              onRetry: failedRead == null || listing ? null : retryReads,
              onDismiss: () => setState(() {
                error = null;
                failedRead = null;
              }),
            ),
          ),
        Expanded(
          child: active == null
              ? Center(
                  child: DshButton(
                    primary: true,
                    onPressed: open,
                    child: const Text(DshConversationZh.newTerminal),
                  ),
                )
              : Padding(
                  padding: const EdgeInsets.all(8),
                  child: TerminalView(
                    terminal,
                    key: ValueKey(active),
                    autofocus: true,
                    textStyle: TerminalStyle(
                      fontFamily: DshTypography.monospaceFamily,
                      fontFamilyFallback: DshTypography.monospaceFallback,
                      fontSize: DshTypography.sizeCaption,
                    ),
                  ),
                ),
        ),
      ],
    ),
  );
}

class TaskPanel extends StatefulWidget {
  const TaskPanel({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<TaskPanel> createState() => _TaskPanelState();
}

class _TaskPanelState extends State<TaskPanel>
    with WidgetsBindingObserver, ResourceDiagnosticScope {
  @override
  String get resourceScopeKind => 'background-tasks-view';
  @override
  Map<String, int> get resourceDiagnostics => {
    'taskPanels': 1,
    'taskPollTimers': timer?.isActive == true ? 1 : 0,
    'taskPollRequests': busy && !readScope.cancelled ? 1 : 0,
  };
  final scope = RequestScope();
  late final api = widget.controller.client!;
  late final session = widget.controller.selectedId!;
  RequestScope readScope = RequestScope();
  List<Json> entries = [];
  Object? error;
  bool busy = false;
  bool panelVisible = true, appVisible = true;
  bool get visible => panelVisible && appVisible;
  Timer? timer;
  @override
  void initState() {
    super.initState();
    final lifecycle = WidgetsBinding.instance.lifecycleState;
    appVisible = lifecycle == null || lifecycle == AppLifecycleState.resumed;
    WidgetsBinding.instance.addObserver(this);
    load();
    schedulePoll();
  }

  void schedulePoll() {
    timer?.cancel();
    timer = visible
        ? Timer.periodic(const Duration(seconds: 4), (_) => load())
        : null;
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    updateVisibility(panel: TickerMode.valuesOf(context).enabled);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    updateVisibility(app: state == AppLifecycleState.resumed);
  }

  void updateVisibility({bool? app, bool? panel}) {
    final wasVisible = visible;
    appVisible = app ?? appVisible;
    panelVisible = panel ?? panelVisible;
    if (visible == wasVisible) return;
    schedulePoll();
    if (!visible) {
      readScope.cancel();
    } else {
      readScope = RequestScope();
      busy = false;
      load();
    }
  }

  @override
  void dispose() {
    timer?.cancel();
    WidgetsBinding.instance.removeObserver(this);
    scope.cancel();
    readScope.cancel();
    super.dispose();
  }

  Future<void> load() async {
    bool ownsSession() =>
        identical(widget.controller.client, api) &&
        widget.controller.selectedId == session;
    if (busy || !visible || !ownsSession()) return;
    setState(() {
      busy = true;
      error = null;
    });
    final requestScope = readScope;
    try {
      final value = await api.request(
        previewUrl('job-list', session),
        scope: requestScope,
      );
      if (mounted && !requestScope.cancelled && ownsSession()) {
        setState(() {
          entries = objects(
            value['entries'] ?? value['items'] ?? value['agents'],
          );
          error = null;
        });
      }
    } catch (e) {
      if (mounted && !requestScope.cancelled && ownsSession()) {
        setState(() => error = e);
      }
    } finally {
      if (mounted && identical(requestScope, readScope)) {
        setState(() => busy = false);
      }
    }
  }

  Future<void> retry() async {
    readScope.cancel();
    readScope = RequestScope();
    busy = false;
    await load();
  }

  @override
  Widget build(BuildContext context) => Column(
    children: [
      Padding(
        padding: const EdgeInsets.all(10),
        child: Row(
          children: [
            Expanded(
              child: Text(
                DshConversationZh.backgroundJobs,
                style: const TextStyle(
                  fontSize: DshTypography.sizeAuxiliary,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
            DshIcon(
              DshIcons.refreshCw.data,
              label: DshConversationZh.refresh,
              onPressed: retry,
            ),
          ],
        ),
      ),
      if (busy) const LinearProgressIndicator(minHeight: 1),
      if (error != null)
        Padding(
          padding: const EdgeInsets.all(10),
          child: _WorkbenchError(error: error!, onRetry: busy ? null : retry),
        ),
      Expanded(
        child: entries.isEmpty
            ? const DshEmpty(DshConversationZh.noBackgroundJobs)
            : ListView.builder(
                itemCount: entries.length,
                itemBuilder: (context, i) {
                  final row = entries[i];
                  return ListTile(
                    title: Text(
                      '${row['title'] ?? row['name'] ?? row['id']}',
                      style: const TextStyle(
                        fontSize: DshTypography.sizeCaption,
                      ),
                    ),
                    subtitle: Text(
                      '${row['status'] ?? row['state'] ?? ''}',
                      style: const TextStyle(
                        fontSize: DshTypography.sizeCaption,
                      ),
                    ),
                    onTap: () => showDialog<void>(
                      context: context,
                      builder: (_) => AlertDialog(
                        title: const Text(DshConversationZh.taskDetails),
                        content: SingleChildScrollView(
                          child: SelectableText(
                            const JsonEncoder.withIndent('  ').convert(row),
                            style: TextStyle(
                              fontFamily: DshTypography.monospaceFamily,
                              fontFamilyFallback:
                                  DshTypography.monospaceFallback,
                              fontSize: DshTypography.sizeCaption,
                            ),
                          ),
                        ),
                      ),
                    ),
                  );
                },
              ),
      ),
    ],
  );
}
