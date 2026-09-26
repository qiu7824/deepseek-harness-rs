import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:pdfrx/pdfrx.dart';
import 'package:xterm/xterm.dart';
import 'package:file_selector/file_selector.dart';
import 'package:dsh_client/dsh_client.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';
import '../../src/resource_diagnostics.dart';
import 'subagent_panel.dart';
import 'project_tasks.dart';
import 'plan_preview.dart';
import 'line_index.dart';

String previewUrl(String operation, String session, [Json values = const {}]) =>
    Uri(
      path: '/__dsh-preview/$operation',
      queryParameters: {
        'sessionId': session,
        for (final entry in values.entries)
          if (entry.value != null) entry.key: '${entry.value}',
      },
    ).toString();

/// A distinct request also reopens a file already selected in another tab.
class FileOpenRequest {
  const FileOpenRequest(this.path);
  final String path;
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
    this.fileRequest,
    this.onFileRequestHandled,
    this.planPreviews,
    this.onPlanSource,
  });
  final DesktopController controller;
  final VoidCallback onClose;
  final String initialTab;
  final int openRequest;
  final ValueChanged<String>? onTabChanged;
  final FileOpenRequest? fileRequest;
  final ValueChanged<FileOpenRequest>? onFileRequestHandled;
  final PlanPreviewStore? planPreviews;
  final VoidCallback? onPlanSource;
  @override
  State<WorkbenchPanel> createState() => _WorkbenchPanelState();
}

class _WorkbenchPanelState extends State<WorkbenchPanel>
    implements ResourceDiagnostics {
  static const labels = {
    'files': '文件',
    'git': 'Git',
    'terminal': '终端',
    'project-tasks': '项目任务',
    'tasks': '后台任务',
    'team': '子任务',
    'plans': '计划预览',
  };
  final tabs = <String>[];
  final tabAnchors = <String, GlobalKey>{};
  String? tab;
  FileOpenRequest? fileRequest;
  @override
  Map<String, int> get resourceDiagnostics => {'workbenchTabs': tabs.length};

  @override
  void initState() {
    super.initState();
    activateTab(widget.initialTab, notify: false);
    acceptFileRequest();
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
      if (value == 'files') fileRequest = null;
      if (value == 'plans') widget.planPreviews?.clear();
      if (tab == value) {
        tab = tabs.isEmpty ? null : tabs[(index - 1).clamp(0, tabs.length - 1)];
      }
    });
    if (tab != null) widget.onTabChanged?.call(tab!);
  }

  @override
  void didUpdateWidget(covariant WorkbenchPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.initialTab != widget.initialTab ||
        oldWidget.openRequest != widget.openRequest) {
      activateTab(widget.initialTab, notify: false);
    }
    if (oldWidget.fileRequest != widget.fileRequest) acceptFileRequest();
  }

  Widget panel(String value) {
    if (value == 'plans') {
      return widget.planPreviews == null
          ? const DshEmpty('此计划预览已失效，请从原计划卡重新打开。')
          : PlanPreviewPanel(
              store: widget.planPreviews!,
              onSource: widget.onPlanSource ?? widget.onClose,
            );
    }
    final controller = widget.controller;
    final api = controller.client;
    final session = controller.selectedId;
    if (api == null || session == null) {
      return const DshEmpty('连接服务并选择会话后打开工具。');
    }
    final key = ValueKey((api, session, value));
    return switch (value) {
      'project-tasks' => ProjectTasks(key: key, api: api, session: session),
      'terminal' => NativeTerminalPanel(
        key: key,
        api: api,
        session: session,
        onInputError: (message) {
          if (controller.client == api && controller.selectedId == session) {
            controller.error = '终端输入发送失败：$message';
            controller.emit();
          }
        },
      ),
      'git' => GitPanel(key: key, api: api, session: session),
      'team' => SubagentPanel(key: key, api: api, parent: session),
      'tasks' => TaskPanel(key: key, controller: widget.controller),
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
            height: 44,
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
                                    height: 30,
                                    padding: const EdgeInsets.symmetric(
                                      horizontal: 10,
                                    ),
                                    onPressed: () =>
                                        setState(() => activateTab(value)),
                                    child: Text(
                                      labels[value]!,
                                      style: const TextStyle(fontSize: 12),
                                    ),
                                  ),
                                ),
                                DshIcon(
                                  LucideIcons.x,
                                  key: ValueKey('workbench-close-$value'),
                                  label: '关闭${labels[value]}标签',
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
                  tooltip: '打开工具标签',
                  padding: EdgeInsets.zero,
                  icon: const DshGlyph(LucideIcons.plus, size: 16),
                  onSelected: (value) => setState(() => activateTab(value)),
                  itemBuilder: (_) => [
                    for (final entry in labels.entries)
                      if (entry.key != 'plans' ||
                          (widget.planPreviews?.items.isNotEmpty ?? false))
                        PopupMenuItem(
                          key: ValueKey('workbench-open-${entry.key}'),
                          value: entry.key,
                          child: Row(
                            children: [
                              Expanded(child: Text(entry.value)),
                              if (tabs.contains(entry.key))
                                const DshGlyph(LucideIcons.check, size: 14),
                            ],
                          ),
                        ),
                  ],
                ),
                DshIcon(
                  LucideIcons.x,
                  label: '关闭工作台',
                  onPressed: widget.onClose,
                ),
              ],
            ),
          ),
          Expanded(
            child: tabs.isEmpty
                ? const DshEmpty('使用 + 打开文件、终端或其他工具标签。')
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

class _FilePanelState extends State<FilePanel> implements ResourceDiagnostics {
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
  String? error;
  bool loading = false;
  int listGeneration = 0;
  final scope = RequestScope();
  final cache = ResourceCache<String, String>(
    maxBytes: 16 * 1024 * 1024 - PlanPreviewStore.maxRetainedBytes,
    maxEntries: 8,
    sizeOf: (text) => text.length * 2,
  );
  final reading = <String, int>{};
  @override
  void initState() {
    super.initState();
    list('');
    if (widget.fileRequest != null) open(widget.fileRequest!.path);
  }

  @override
  void didUpdateWidget(covariant FilePanel oldWidget) {
    super.didUpdateWidget(oldWidget);
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
    setState(() => loading = true);
    try {
      final value = await widget.api.request(
        previewUrl('list', widget.session, {'path': path}),
        scope: scope,
        maxBytes: 2 * 1024 * 1024,
      );
      if (mounted && generation == listGeneration) {
        setState(() {
          entries = objects(value['entries']);
          directory = '${value['path'] ?? path}';
          error = null;
        });
      }
    } catch (e) {
      if (mounted && generation == listGeneration) {
        setState(() => error = '$e');
      }
    } finally {
      if (mounted && generation == listGeneration) {
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
      active = path;
    });
  }

  void close(String path) {
    setState(() {
      files.remove(path);
      cache.remove(path);
      reading.remove(path);
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
            height: 34,
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
                            style: const TextStyle(fontSize: 11),
                          ),
                        ),
                        DshIcon(
                          LucideIcons.x,
                          label:
                              '关闭 ${path.replaceAll('\\', '/').split('/').last}',
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
                    ? const DshEmpty('选择文件以查看内容', icon: LucideIcons.files)
                    : NativeFileViewer(
                        key: ValueKey(active),
                        api: widget.api,
                        session: widget.session,
                        path: active,
                        cache: cache,
                        initialPage: reading[active] ?? 1,
                        onPage: (page) => reading[active] = page,
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
                            LucideIcons.arrowUp,
                            label: '上级目录',
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
                                  ? '工作区'
                                  : directory.split('/').last,
                              style: const TextStyle(fontSize: 11),
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                          DshIcon(
                            LucideIcons.refreshCw,
                            label: '刷新目录',
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
                        child: Text(
                          error!,
                          style: const TextStyle(
                            fontSize: 10,
                            color: Colors.red,
                          ),
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
                                        ? LucideIcons.folder
                                        : LucideIcons.file,
                                    size: 14,
                                    color: folder ? colors.blue : colors.muted,
                                  ),
                                  const SizedBox(width: 6),
                                  Expanded(
                                    child: Text(
                                      '${entry['name']}',
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: const TextStyle(fontSize: 11),
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
  });
  final DshClient api;
  final String session, path;
  final ResourceCache<String, String> cache;
  final ValueChanged<int> onPage;
  final int initialPage;
  final int initialLine;
  @override
  State<NativeFileViewer> createState() => _NativeFileViewerState();
}

class _NativeFileViewerState extends State<NativeFileViewer>
    implements ResourceDiagnostics {
  @override
  Map<String, int> get resourceDiagnostics => {
    'fileViewers': 1,
    'fileBinaryBytes': binary?.length ?? 0,
    'pdfViewers': pdf ? 1 : 0,
    'imageViewers': image ? 1 : 0,
    'pdfImageCacheBudgetBytes': pdf ? 48 * 1024 * 1024 : 0,
    'indexedLinesBytes': lines.retainedBytes,
  };
  final scope = RequestScope();
  final search = TextEditingController(), lineInput = TextEditingController();
  final scroll = ScrollController();
  String? text, error;
  Uint8List? binary;
  LineIndex lines = LineIndex.empty();
  bool source = false, wrap = false, loading = true, saving = false;
  final pdfController = PdfViewerController();
  int matchIndex = -1;
  String get ext => widget.path.split('.').last.toLowerCase();
  bool get pdf =>
      ['pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx'].contains(ext);
  bool get image => ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'].contains(ext);
  @override
  void initState() {
    super.initState();
    source = widget.initialLine > 1;
    load();
  }

  @override
  void dispose() {
    scope.cancel();
    search.dispose();
    lineInput.dispose();
    scroll.dispose();
    binary = null;
    lines = LineIndex.empty();
    text = null;
    super.dispose();
  }

  Future<void> load() async {
    try {
      if (pdf || image) {
        final office = ext != 'pdf' && pdf;
        final loaded = await widget.api.bytes(
          office
              ? '/__dsh-preview/office'
              : previewUrl('file', widget.session, {'path': widget.path}),
          body: office
              ? {'sessionId': widget.session, 'path': widget.path}
              : null,
          scope: scope,
          maxBytes: 16 * 1024 * 1024,
        );
        if (!mounted || scope.cancelled) return;
        binary = loaded;
      } else {
        text = widget.cache.get(widget.path);
        if (text == null) {
          final result = await widget.api.request(
            previewUrl('source', widget.session, {'path': widget.path}),
            scope: scope,
            maxBytes: 8 * 1024 * 1024,
          );
          if (!mounted || scope.cancelled) return;
          text = '${result['text'] ?? ''}';
          widget.cache.put(widget.path, text!);
        }
        lines = LineIndex.fromText(text!);
        if (widget.initialLine > 1 && lines.isNotEmpty) {
          matchIndex = (widget.initialLine - 1).clamp(0, lines.length - 1);
        }
      }
      if (mounted) setState(() => loading = false);
      if (mounted && matchIndex >= 0) {
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted && scroll.hasClients) {
            scroll.jumpTo(
              (matchIndex * 22.0).clamp(0, scroll.position.maxScrollExtent),
            );
          }
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

  Future<void> export() async {
    if (saving) return;
    setState(() => saving = true);
    try {
      final name = widget.path.replaceAll('\\', '/').split('/').last;
      final converted = pdf && ext != 'pdf';
      final target = await getSaveLocation(
        suggestedName: converted
            ? '${name.substring(0, name.lastIndexOf('.'))}.pdf'
            : name,
      );
      if (target == null || !mounted) return;
      if (converted) {
        if (binary == null) throw StateError('PDF 预览尚未生成');
        await XFile.fromData(binary!).saveTo(target.path);
      } else {
        await widget.api.downloadTo(
          previewUrl('file', widget.session, {'path': widget.path}),
          File(target.path),
          scope: scope,
        );
      }
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => saving = false);
    }
  }

  void findNext() {
    if (search.text.isEmpty || lines.isEmpty) return;
    for (var i = 1; i <= lines.length; i++) {
      final index = (matchIndex + i) % lines.length;
      if (lines[index].toLowerCase().contains(search.text.toLowerCase())) {
        setState(() => matchIndex = index);
        if (scroll.hasClients) {
          scroll.jumpTo(
            (index * 22.0).clamp(0, scroll.position.maxScrollExtent),
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
          height: 38,
          padding: const EdgeInsets.symmetric(horizontal: 6),
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: colors.border)),
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  widget.path.replaceAll('\\', '/').split('/').last,
                  style: const TextStyle(fontSize: 11),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              if (text != null) ...[
                DshIcon(
                  LucideIcons.code,
                  asset: 'assets/icons/source-code.svg',
                  label: source ? '预览' : '源码',
                  active: source,
                  size: 25,
                  onPressed: () => setState(() => source = !source),
                ),
                DshIcon(
                  LucideIcons.wrapText,
                  label: '自动换行',
                  active: wrap,
                  size: 25,
                  onPressed: () => setState(() => wrap = !wrap),
                ),
                DshIcon(
                  LucideIcons.copy,
                  label: '复制文件内容',
                  size: 25,
                  onPressed: () =>
                      Clipboard.setData(ClipboardData(text: text!)),
                ),
              ],
              DshIcon(
                LucideIcons.download,
                label: pdf && ext != 'pdf' ? '导出 PDF' : '保存原文件副本',
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
                    style: const TextStyle(fontSize: 11),
                    decoration: const InputDecoration(
                      isDense: true,
                      hintText: '查找内容',
                      border: OutlineInputBorder(),
                      contentPadding: EdgeInsets.all(8),
                    ),
                  ),
                ),
                DshIcon(
                  LucideIcons.search,
                  label: '查找下一个',
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
              ? DshEmpty(error!, icon: LucideIcons.circleAlert)
              : pdf
              ? PdfViewer.data(
                  binary!,
                  sourceName: '${widget.session}/${widget.path}',
                  controller: pdfController,
                  initialPageNumber: widget.initialPage,
                  maxSizeToCacheOnMemory: 1024 * 1024,
                  params: PdfViewerParams(
                    maxImageBytesCachedOnMemory: 48 * 1024 * 1024,
                    verticalCacheExtent: 0.3,
                    horizontalCacheExtent: 0.0,
                    onPageChanged: (page) {
                      if (page != null) widget.onPage(page);
                    },
                  ),
                )
              : image
              ? InteractiveViewer(
                  minScale: .1,
                  maxScale: 5,
                  child: Center(
                    child: Image.memory(
                      binary!,
                      cacheWidth: 1600,
                      filterQuality: FilterQuality.medium,
                      errorBuilder: (_, e, _) => DshEmpty('$e'),
                    ),
                  ),
                )
              : ext == 'md' && !source
              ? SingleChildScrollView(
                  padding: const EdgeInsets.all(18),
                  child: MarkdownBody(data: text!, selectable: true),
                )
              : codeView(colors),
        ),
      ],
    );
  }

  Widget codeView(DshColors colors) => ListView.builder(
    controller: scroll,
    itemExtent: wrap ? null : 22,
    itemCount: lines.length,
    itemBuilder: (context, i) => Container(
      color: i == matchIndex ? colors.blue.withValues(alpha: .12) : null,
      padding: const EdgeInsets.only(right: 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 43,
            child: Text(
              '${i + 1}',
              textAlign: TextAlign.right,
              style: TextStyle(
                fontFamily: 'Consolas',
                fontSize: 11,
                height: 1.7,
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
                fontFamily: 'Consolas',
                fontSize: 12,
                height: 1.65,
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
  final scope = RequestScope();
  Json? status;
  String? path, error;
  List<String> diff = [];
  bool busy = false, staged = false;
  int diffGeneration = 0;
  @override
  void initState() {
    super.initState();
    load();
  }

  @override
  void dispose() {
    scope.cancel();
    diff = [];
    super.dispose();
  }

  Future<void> load() async {
    setState(() => busy = true);
    try {
      final result = await widget.api.request(
        previewUrl('git-status', widget.session),
        scope: scope,
      );
      if (mounted) {
        setState(() {
          status = result;
          error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> select(Json entry) async {
    final generation = ++diffGeneration;
    final requestedPath = '${entry['path']}';
    final requestedStaged = entry['group'] == 'staged';
    setState(() {
      path = requestedPath;
      staged = requestedStaged;
      diff = [];
      error = null;
    });
    bool current() =>
        mounted &&
        generation == diffGeneration &&
        path == requestedPath &&
        staged == requestedStaged;
    try {
      final result = await widget.api.request(
        previewUrl('git-diff', widget.session, {
          'path': requestedPath,
          'staged': requestedStaged ? '1' : '0',
        }),
        scope: scope,
        maxBytes: 8 * 1024 * 1024,
      );
      if (current()) {
        setState(() => diff = '${result['diff'] ?? ''}'.split('\n'));
      }
    } catch (e) {
      if (current()) setState(() => error = '$e');
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
              DshGlyph(LucideIcons.gitBranch, size: 15, color: colors.muted),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  '${status?['branch'] ?? 'Git'}',
                  style: const TextStyle(fontSize: 12),
                ),
              ),
              DshIcon(LucideIcons.refreshCw, label: '刷新修改', onPressed: load),
            ],
          ),
        ),
        if (busy) const LinearProgressIndicator(minHeight: 1),
        if (error != null)
          Padding(
            padding: const EdgeInsets.all(10),
            child: Text(
              error!,
              style: const TextStyle(color: Colors.red, fontSize: 12),
            ),
          ),
        Expanded(
          child: Row(
            children: [
              Expanded(
                child: path == null
                    ? const DshEmpty(
                        '选择修改文件查看差异',
                        icon: LucideIcons.gitCompareArrows,
                      )
                    : Column(
                        children: [
                          Padding(
                            padding: const EdgeInsets.all(8),
                            child: Text(
                              path!,
                              style: const TextStyle(fontSize: 11),
                            ),
                          ),
                          Expanded(
                            child: ListView.builder(
                              itemCount: diff.length,
                              itemBuilder: (context, i) {
                                final line = diff[i];
                                final color = line.startsWith('+')
                                    ? Colors.green
                                    : line.startsWith('-')
                                    ? Colors.red
                                    : line.startsWith('@@')
                                    ? Colors.blue
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
                                      fontFamily: 'Consolas',
                                      fontSize: 11,
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
                          style: const TextStyle(fontSize: 11),
                        ),
                        subtitle: Text(
                          '${entry['status']}',
                          style: const TextStyle(fontSize: 10),
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
      throw const FormatException('终端输出格式无效');
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
  });
  final DshClient api;
  final String session;
  final ValueChanged<String>? onInputError;
  @override
  State<NativeTerminalPanel> createState() => _NativeTerminalPanelState();
}

class _NativeTerminalPanelState extends State<NativeTerminalPanel>
    with WidgetsBindingObserver
    implements ResourceDiagnostics {
  static final inputTails = <(DshClient, String, String), Future<void>>{};
  @override
  Map<String, int> get resourceDiagnostics => {
    'terminalPanels': 1,
    'terminalBufferLines': terminal.buffer.lines.length,
    'terminalQueuedInputUnits': pendingInput.length + queuedInputLength,
    'terminalInputOwners': inputTails.length,
  };
  late Terminal terminal;
  final scope = RequestScope();
  final inputScope = RequestScope();
  late final inputApi = widget.api;
  late final inputSession = widget.session;
  RequestScope readScope = RequestScope();
  List<Json> entries = [];
  String? active, error;
  Timer? timer, resizeTimer, inputTimer;
  bool panelVisible = true, appVisible = true;
  bool get visible => panelVisible && appVisible;
  int? pollingEpoch;
  int listGeneration = 0, queuedInputLength = 0;
  Future<void> inputWrites = Future.value();
  TerminalOutputWindow output = TerminalOutputWindow();
  String pendingInput = '';
  int epoch = 0;
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    terminal = makeTerminal();
    load();
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
        setState(() => error = '终端输入积压过多，请等待发送完成后重试');
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
      epoch++;
      resizeTimer?.cancel();
    } else {
      readScope = RequestScope();
      configureTerminal(terminal);
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
        widget.onInputError?.call('$e');
      }
      if (mounted &&
          generation == epoch &&
          active == owner &&
          !requestScope.cancelled) {
        setState(() => error = '$e');
      }
      return false;
    }
  }

  Future<void> load() async {
    final generation = ++listGeneration;
    try {
      final value = await widget.api.request(
        previewUrl('terminal-list', widget.session),
        scope: scope,
      );
      if (!mounted || generation != listGeneration) return;
      setState(() => entries = objects(value['entries']));
      if (active == null && entries.isNotEmpty) {
        select('${entries.first['id']}');
      }
    } catch (e) {
      if (mounted && generation == listGeneration && !scope.cancelled) {
        setState(() => error = '$e');
      }
    }
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
    output = TerminalOutputWindow();
    terminal.onOutput = null;
    terminal.onResize = null;
    terminal = makeTerminal();
    setState(() {});
    poll();
  }

  Future<void> open() async {
    if (entries.length >= 3) {
      setState(() => error = '最多同时保留 3 个终端，请先关闭不需要的终端');
      return;
    }
    final generation = epoch;
    try {
      final value = await widget.api.request(
        '/__dsh-preview/terminal-action',
        body: {
          'sessionId': widget.session,
          'action': 'open',
          'name': '终端 ${entries.length + 1}',
        },
        mutation: true,
        scope: scope,
      );
      await load();
      if (mounted && generation == epoch) select('${value['id']}');
    } catch (e) {
      if (mounted) setState(() => error = '$e');
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
    if (pollingEpoch == epoch ||
        active == null ||
        !visible ||
        scope.cancelled) {
      return;
    }
    pollingEpoch = epoch;
    final generation = epoch, id = active!;
    final requestScope = readScope;
    try {
      final page = await widget.api.request(
        previewUrl('terminal-read', widget.session, {
          'terminalId': id,
          'count': output.initialized ? 64 : 2000,
        }),
        scope: requestScope,
        maxBytes: 1024 * 1024,
      );
      if (!mounted || generation != epoch) return;
      var update = output.apply(page);
      if (update == null) {
        final snapshot = await widget.api.request(
          previewUrl('terminal-read', widget.session, {
            'terminalId': id,
            'count': 2000,
          }),
          scope: requestScope,
          maxBytes: 1024 * 1024,
        );
        if (!mounted || generation != epoch) return;
        update = output.apply(snapshot, reset: true)!;
      }
      terminal.write(update);
    } catch (e) {
      if (mounted && generation == epoch && !requestScope.cancelled) {
        setState(() => error = '$e');
      }
    } finally {
      if (pollingEpoch == generation) pollingEpoch = null;
      if (mounted &&
          generation == epoch &&
          !requestScope.cancelled &&
          visible) {
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
                            fontSize: 11,
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
                tooltip: '新建终端',
                onPressed: open,
                icon: const DshGlyph(
                  LucideIcons.plus,
                  size: 16,
                  color: Colors.white,
                ),
              ),
              IconButton(
                tooltip: '关闭当前终端',
                onPressed: active == null ? null : closeTerminal,
                icon: const DshGlyph(
                  LucideIcons.x,
                  size: 16,
                  color: Colors.white,
                ),
              ),
            ],
          ),
        ),
        if (error != null)
          Padding(
            padding: const EdgeInsets.all(8),
            child: Text(
              error!,
              style: const TextStyle(color: Colors.orange, fontSize: 11),
            ),
          ),
        Expanded(
          child: active == null
              ? Center(
                  child: DshButton(
                    primary: true,
                    onPressed: open,
                    child: const Text('新建终端'),
                  ),
                )
              : Padding(
                  padding: const EdgeInsets.all(8),
                  child: TerminalView(
                    terminal,
                    key: ValueKey(active),
                    autofocus: true,
                    textStyle: const TerminalStyle(
                      fontFamily: 'Consolas',
                      fontSize: 12,
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

class _TaskPanelState extends State<TaskPanel> with WidgetsBindingObserver {
  final scope = RequestScope();
  late final api = widget.controller.client!;
  late final session = widget.controller.selectedId!;
  RequestScope readScope = RequestScope();
  List<Json> entries = [];
  String? error;
  bool busy = false;
  bool panelVisible = true, appVisible = true;
  bool get visible => panelVisible && appVisible;
  Timer? timer;
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    load();
    timer = Timer.periodic(const Duration(seconds: 4), (_) {
      if (visible) load();
    });
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
    if (busy || !visible) return;
    busy = true;
    final requestScope = readScope;
    try {
      final value = await api.request(
        previewUrl('job-list', session),
        scope: requestScope,
      );
      if (mounted && !requestScope.cancelled) {
        setState(
          () => entries = objects(
            value['entries'] ?? value['items'] ?? value['agents'],
          ),
        );
      }
    } catch (e) {
      if (mounted && !requestScope.cancelled) setState(() => error = '$e');
    } finally {
      if (identical(requestScope, readScope)) busy = false;
    }
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
                '后台任务',
                style: const TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
            DshIcon(LucideIcons.refreshCw, label: '刷新', onPressed: load),
          ],
        ),
      ),
      if (error != null)
        Padding(
          padding: const EdgeInsets.all(10),
          child: Text(
            error!,
            style: const TextStyle(fontSize: 11, color: Colors.red),
          ),
        ),
      Expanded(
        child: entries.isEmpty
            ? const DshEmpty('当前没有后台任务')
            : ListView.builder(
                itemCount: entries.length,
                itemBuilder: (context, i) {
                  final row = entries[i];
                  return ListTile(
                    title: Text(
                      '${row['title'] ?? row['name'] ?? row['id']}',
                      style: const TextStyle(fontSize: 12),
                    ),
                    subtitle: Text(
                      '${row['status'] ?? row['state'] ?? ''}',
                      style: const TextStyle(fontSize: 11),
                    ),
                    onTap: () => showDialog<void>(
                      context: context,
                      builder: (_) => AlertDialog(
                        title: const Text('任务详情'),
                        content: SingleChildScrollView(
                          child: SelectableText(
                            const JsonEncoder.withIndent('  ').convert(row),
                            style: const TextStyle(
                              fontFamily: 'Consolas',
                              fontSize: 12,
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
