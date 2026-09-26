import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:xterm/xterm.dart' show TerminalView;
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:file_selector/file_selector.dart';
import 'package:dsh_client/dsh_client.dart';

import '../design/primitives.dart';
import '../design/shortcuts.dart';
import '../src/controller.dart';
import '../src/preferences.dart';
import '../src/conversation.dart';
import 'conversation/feedback_controller.dart';
import 'conversation/session_log_export.dart';
import 'settings/settings_shell.dart';
import 'workbench/workbench_panel.dart';
import 'workbench/plan_preview.dart';
import '../src/resource_diagnostics.dart';
import 'workspace_tree_row.dart';
import 'workspace_source_dialog.dart';

String relativeSessionAge(int updatedAt, {int? nowMillis}) {
  final elapsed =
      ((nowMillis ?? DateTime.now().millisecondsSinceEpoch) - updatedAt).clamp(
        0,
        1 << 62,
      );
  const minute = 60000, hour = 60 * minute, day = 24 * hour;
  if (elapsed < minute) return '刚刚';
  if (elapsed < hour) return '${elapsed ~/ minute}分钟';
  if (elapsed < day) return '${elapsed ~/ hour}小时';
  if (elapsed < 30 * day) return '${elapsed ~/ day}天';
  if (elapsed < 365 * day) return '${elapsed ~/ (30 * day)}个月';
  return '${elapsed ~/ (365 * day)}年';
}

class Workbench extends StatefulWidget {
  const Workbench({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<Workbench> createState() => _WorkbenchState();
}

class _WorkbenchState extends State<Workbench> implements ResourceDiagnostics {
  final planPreviews = PlanPreviewStore();
  DshClient? previewClient;
  String? previewSession;
  @override
  Map<String, int> get resourceDiagnostics => {
    'planPreviewCacheEntries': planPreviews.items.length,
    'planPreviewCacheBytes': planPreviews.retainedBytes,
    'planPreviewCacheBudgetBytes': PlanPreviewStore.maxRetainedBytes,
  };
  void previewScopeChanged() {
    if (identical(previewClient, c.client) && previewSession == c.selectedId) {
      return;
    }
    previewClient = c.client;
    previewSession = c.selectedId;
    planPreviews.scope(c.client, c.selectedId);
    if (dockTab == 'plans') dockTab = 'files';
    if (mounted) setState(() {});
  }

  void openPlan(TranscriptItem item) {
    planPreviews.scope(c.client, c.selectedId);
    planPreviews.open(item);
    openDock('plans');
  }

  void closeDock() {
    scaffoldKey.currentState?.closeEndDrawer();
    planPreviews.clear();
    setState(() {
      dockOpen = false;
      if (dockTab == 'plans') dockTab = 'files';
    });
  }

  void planSource() {
    conversationViewRequest.value = 'conversation';
    if (availableWidth < 1100) {
      closeDock();
    }
    c.composerFocus.value++;
  }

  DesktopController get c => widget.controller;
  final search = TextEditingController();
  final searchFocus = FocusNode();
  final shellFocus = FocusNode();
  final scaffoldKey = GlobalKey<ScaffoldState>();
  final workspaceAnchor = GlobalKey();
  final conversationViewRequest = ValueNotifier<String>('');
  final Map<String, bool> groupExpansion = {};
  bool showSearch = false, sideOpen = true, dockOpen = false;
  double sidebarWidth = 280, dockWidth = 470, chatWidth = 0;
  double availableWidth = 0;
  String dockTab = 'files';
  FileOpenRequest? fileRequest;
  DshClient? fileRequestClient;
  String? fileRequestSession;
  FileOpenRequest? get currentFileRequest =>
      fileRequestClient == c.client && fileRequestSession == c.selectedId
      ? fileRequest
      : null;
  Future<void> openFile(String path) async {
    final client = c.client, session = c.selectedId;
    try {
      final reference = UploadedFileReceipt.referenceFromPath(path);
      if (reference != null) {
        if (client == null || session == null) return;
        final result = await client.call('session.fileAttachment', {
          'sessionId': session,
          'attachment': reference,
        });
        if (!mounted || c.client != client || c.selectedId != session) return;
        path = result['path'] as String;
      }
    } catch (error) {
      if (mounted && c.client == client && c.selectedId == session) {
        c.error = '$error';
        c.emit();
      }
      return;
    }
    fileRequest = FileOpenRequest(path);
    fileRequestClient = c.client;
    fileRequestSession = c.selectedId;
    openDock('files');
  }

  @override
  void initState() {
    super.initState();
    previewClient = c.client;
    previewSession = c.selectedId;
    planPreviews.scope(c.client, c.selectedId);
    c.addListener(previewScopeChanged);
    FocusManager.instance.addEarlyKeyEventHandler(onShortcut);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && ModalRoute.of(context)?.isCurrent != false) {
        shellFocus.requestFocus();
      }
    });
    sidebarWidth =
        (c.preferences.layout['sidebarWidth'] as num?)?.toDouble() ?? 280;
    dockWidth = (c.preferences.layout['dockWidth'] as num?)?.toDouble() ?? 470;
    chatWidth = (c.preferences.layout['chatWidth'] as num?)?.toDouble() ?? 0;
    if (c.preferences.layout['chatWidthMode'] != 'manual') chatWidth = 0;
    sideOpen = c.preferences.layout['sideOpen'] != false;
    for (final entry in object(
      c.preferences.layout['groupExpansion'],
    ).entries.take(256)) {
      if (entry.value is bool) {
        groupExpansion[entry.key] = entry.value as bool;
      }
    }
  }

  @override
  void didUpdateWidget(Workbench oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(previewScopeChanged);
      c.addListener(previewScopeChanged);
      previewScopeChanged();
    }
  }

  @override
  void dispose() {
    c.removeListener(previewScopeChanged);
    planPreviews.dispose();
    FocusManager.instance.removeEarlyKeyEventHandler(onShortcut);
    shellFocus.dispose();
    search.dispose();
    searchFocus.dispose();
    conversationViewRequest.dispose();
    super.dispose();
  }

  void saveLayout() {
    c.preferences.layout.addAll({
      'sidebarWidth': sidebarWidth,
      'dockWidth': dockWidth,
      'chatWidth': chatWidth,
      'chatWidthMode': chatWidth == 0 ? 'auto' : 'manual',
      'sideOpen': sideOpen,
      'groupExpansion': Map<String, bool>.of(groupExpansion),
    });
    unawaited(c.run(c.preferences.save));
  }

  String get archiveFilter => switch (c.preferences.layout['archiveFilter']) {
    'all' => 'all',
    'archived' => 'archived',
    _ => 'hidden',
  };

  void setArchiveFilter(String value) {
    setState(() => c.preferences.layout['archiveFilter'] = value);
    saveLayout();
  }

  String shortcutHint(String action, String label) =>
      '$label · ${shortcutLabel(configuredShortcuts(c)[action]!)}';

  Widget shortcutBadge(String action) => Tooltip(
    message: shortcutHint(action, shortcutNames[action]!),
    child: ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 84),
      child: Text(
        shortcutLabel(configuredShortcuts(c)[action]!),
        key: ValueKey('sidebar-shortcut-$action'),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(fontSize: 11, color: DshColors(context).muted),
      ),
    ),
  );

  void startConversation() {
    final workspace = c.workspaceId;
    if (workspace != null) setGroupExpanded(workspace, true);
    unawaited(c.run(c.startConversation));
    c.composerFocus.value++;
  }

  bool groupExpanded(String id) => groupExpansion[id] ?? c.workspaceId == id;

  void setGroupExpanded(String id, bool expanded) {
    setState(() {
      groupExpansion.remove(id);
      groupExpansion[id] = expanded;
      if (groupExpansion.length > 256) {
        groupExpansion.remove(groupExpansion.keys.first);
      }
    });
    saveLayout();
  }

  void openDock([String tab = 'files']) {
    setState(() {
      dockOpen = true;
      dockTab = tab;
    });
    if (availableWidth < 1100) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) scaffoldKey.currentState?.openEndDrawer();
      });
    }
  }

  void toggleSidebar() {
    if (availableWidth < 900) {
      scaffoldKey.currentState?.openDrawer();
      return;
    }
    setState(() => sideOpen = !sideOpen);
    saveLayout();
  }

  void openSearch() {
    setState(() {
      sideOpen = true;
      showSearch = true;
    });
    saveLayout();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) searchFocus.requestFocus();
    });
    if (availableWidth < 900) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) scaffoldKey.currentState?.openDrawer();
      });
    }
  }

  KeyEventResult onShortcut(KeyEvent event) {
    if (!mounted ||
        event is! KeyDownEvent ||
        ModalRoute.of(context)?.isCurrent == false) {
      return KeyEventResult.ignored;
    }
    var terminal = false;
    FocusManager.instance.primaryFocus?.context?.visitAncestorElements((
      element,
    ) {
      if (element.widget is TerminalView) terminal = true;
      return !terminal;
    });
    final editable = FocusManager.instance.primaryFocus?.context
        ?.findAncestorWidgetOfExactType<EditableText>();
    if (terminal ||
        (editable?.controller.value.composing.isValid == true &&
            editable?.controller.value.composing.isCollapsed == false)) {
      return KeyEventResult.ignored;
    }
    final action = configuredShortcuts(c).entries
        .where((e) => e.value.accepts(event, HardwareKeyboard.instance))
        .firstOrNull
        ?.key;
    if (action == null) return KeyEventResult.ignored;
    switch (action) {
      case 'sidebar':
        toggleSidebar();
      case 'new':
        startConversation();
      case 'search':
        openSearch();
      case 'settings':
        settings();
      case 'composer':
        c.composerFocus.value++;
      case 'workbench':
        if (c.selectedId != null) {
          if (dockOpen) {
            closeDock();
          } else {
            openDock(dockTab);
          }
        }
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      availableWidth = constraints.maxWidth;
      final wide = constraints.maxWidth >= 900;
      final canDock = constraints.maxWidth >= 1100;
      final maxDockWidth = canDock
          ? (constraints.maxWidth -
                    (sideOpen ? sidebarWidth.clamp(220, 340) + 1 : 56) -
                    360)
                .clamp(330.0, constraints.maxWidth * .55)
                .toDouble()
          : 330.0;
      return Focus(
        focusNode: shellFocus,
        autofocus: true,
        child: Scaffold(
          key: scaffoldKey,
          onEndDrawerChanged: (open) {
            if (!open && dockOpen && availableWidth < 1100) closeDock();
          },
          drawer: wide ? null : Drawer(width: 280, child: sidebar()),
          body: SafeArea(
            child: Row(
              children: [
                if (wide && sideOpen) ...[
                  SizedBox(
                    width: sidebarWidth.clamp(220, 340),
                    child: sidebar(),
                  ),
                  _divider(
                    (dx) => setState(
                      () => sidebarWidth = (sidebarWidth + dx).clamp(220, 340),
                    ),
                  ),
                ],
                if (wide && !sideOpen)
                  SizedBox(width: 56, child: collapsedSidebar()),
                Expanded(
                  child: Column(
                    children: [
                      if (c.error != null)
                        Container(
                          width: double.infinity,
                          color: Theme.of(context).colorScheme.errorContainer,
                          padding: const EdgeInsets.symmetric(
                            horizontal: 16,
                            vertical: 7,
                          ),
                          child: Row(
                            children: [
                              const DshGlyph(LucideIcons.circleAlert, size: 16),
                              const SizedBox(width: 8),
                              Expanded(
                                child: Text(
                                  c.error!,
                                  maxLines: 3,
                                  overflow: TextOverflow.ellipsis,
                                  style: const TextStyle(fontSize: 14),
                                ),
                              ),
                              DshIcon(
                                LucideIcons.x,
                                label: '关闭提示',
                                onPressed: c.clearError,
                              ),
                            ],
                          ),
                        ),
                      Expanded(
                        child: Stack(
                          children: [
                            Conversation(
                              controller: c,
                              headerInset: !wide ? 56 : 28,
                              maxContentWidth: chatWidth,
                              onOpenSettings: () => settings('models'),
                              onOpenWorkbench: openDock,
                              onOpenPath: openFile,
                              onOpenPlan: openPlan,
                              onSelectWorkspace: chooseWorkspace,
                              workspaceAnchor: workspaceAnchor,
                              viewRequest: conversationViewRequest,
                            ),
                            if (!wide)
                              Positioned(
                                top: 10,
                                left: 12,
                                child: Builder(
                                  builder: (context) => DshIcon(
                                    LucideIcons.panelLeft,
                                    label: '展开侧边栏',
                                    onPressed: () {
                                      if (wide) {
                                        setState(() => sideOpen = true);
                                        saveLayout();
                                      } else {
                                        Scaffold.of(context).openDrawer();
                                      }
                                    },
                                  ),
                                ),
                              ),
                            if (c.selectedId != null && !c.blankConversation)
                              Positioned(
                                top: 8,
                                right: 28,
                                child: Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    SessionLogExportAction(
                                      controller: c,
                                      sessionId: c.selectedId!,
                                    ),
                                    const SizedBox(width: 8),
                                    DshIcon(
                                      LucideIcons.messageCircle,
                                      label: '会话反馈',
                                      asset: 'assets/icons/web-session-feedback.svg',
                                      glyphSize: 18,
                                      onPressed: c.selectedId == null
                                          ? null
                                          : sessionFeedback,
                                    ),
                                    if (c.teamSettings['showButton'] != false)
                                      const SizedBox(width: 8),
                                    if (c.teamSettings['showButton'] != false)
                                      DshIcon(
                                        LucideIcons.users,
                                        label: '协作',
                                        asset: 'assets/icons/web-team.svg',
                                        onPressed: c.selectedId == null
                                            ? null
                                            : () => openDock('team'),
                                      ),
                                    const SizedBox(width: 8),
                                    DshIcon(
                                      LucideIcons.panelRight,
                                      label: '显示工作台',
                                      active: dockOpen,
                                      onPressed: c.selectedId == null
                                          ? null
                                          : () {
                                              if (dockOpen) {
                                                closeDock();
                                              } else {
                                                openDock(dockTab);
                                              }
                                            },
                                    ),
                                  ],
                                ),
                              ),
                            if (constraints.maxWidth > 1000)
                              Positioned(
                                right: 0,
                                top: 60,
                                bottom: 40,
                                child: Tooltip(
                                  message: '拖动调整对话宽度；双击恢复默认',
                                  child: GestureDetector(
                                    onDoubleTap: () {
                                      setState(() => chatWidth = 0);
                                      saveLayout();
                                    },
                                    onHorizontalDragUpdate: (d) => setState(
                                      () => chatWidth =
                                          ((chatWidth > 0
                                                      ? chatWidth
                                                      : ((availableWidth -
                                                                    (sideOpen
                                                                        ? sidebarWidth +
                                                                              1
                                                                        : 56) -
                                                                    (dockOpen
                                                                        ? dockWidth
                                                                        : 0)) *
                                                                .64)
                                                            .clamp(0, 920)) -
                                                  d.delta.dx * 2)
                                              .clamp(520, 1100),
                                    ),
                                    onHorizontalDragEnd: (_) => saveLayout(),
                                    child: MouseRegion(
                                      cursor: SystemMouseCursors.resizeColumn,
                                      child: Container(
                                        width: 5,
                                        color: Colors.transparent,
                                      ),
                                    ),
                                  ),
                                ),
                              ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
                if (canDock && dockOpen && c.selectedId != null) ...[
                  _divider(
                    (dx) => setState(
                      () =>
                          dockWidth = (dockWidth.clamp(330, maxDockWidth) - dx)
                              .clamp(330, maxDockWidth),
                    ),
                  ),
                  SizedBox(
                    width: dockWidth.clamp(330, maxDockWidth),
                    child: WorkbenchPanel(
                      key: ValueKey('dock-${c.selectedId}'),
                      controller: c,
                      initialTab: dockTab,
                      fileRequest: currentFileRequest,
                      planPreviews: planPreviews,
                      onPlanSource: planSource,
                      onClose: closeDock,
                    ),
                  ),
                ],
              ],
            ),
          ),
          endDrawer: !canDock && dockOpen && c.selectedId != null
              ? Drawer(
                  width: constraints.maxWidth * .9,
                  child: WorkbenchPanel(
                    controller: c,
                    initialTab: dockTab,
                    fileRequest: currentFileRequest,
                    planPreviews: planPreviews,
                    onPlanSource: planSource,
                    onClose: closeDock,
                  ),
                )
              : null,
        ),
      );
    },
  );

  Future<void> accountSettings() => showDialog<void>(
    context: context,
    barrierColor: Colors.transparent,
    barrierDismissible: false,
    builder: (_) => SettingsShell(
      controller: c,
      initialPage: 'models',
      initialModelTab: 'accounts',
    ),
  );

  Widget accountEntries({bool compact = false}) => Column(
    mainAxisSize: MainAxisSize.min,
    children: [
      for (final account in c.subscriptionAccounts.where(
        (a) => a['signedIn'] == true,
      ))
        Padding(
          padding: EdgeInsets.symmetric(vertical: compact ? 8 : 2),
          child: compact
              ? DshIcon(
                  LucideIcons.workflow,
                  label: '${account['name']} · 已连接',
                  size: 36,
                  color: DshColors(context).text,
                  onPressed: accountSettings,
                )
              : Align(
                  alignment: Alignment.centerLeft,
                  child: DshButton(
                    icon: LucideIcons.workflow,
                    onPressed: accountSettings,
                    child: Text(
                      '${account['name']}',
                      style: const TextStyle(fontSize: 13),
                    ),
                  ),
                ),
        ),
    ],
  );

  Widget collapsedSidebar() {
    final colors = DshColors(context);
    return Material(
      color: colors.sidebar,
      child: Column(
        children: [
          const SizedBox(height: 12),
          Tooltip(
            message: shortcutHint('sidebar', '展开侧边栏'),
            child: InkWell(
              onTap: toggleSidebar,
              borderRadius: BorderRadius.circular(8),
              child: SizedBox(
                width: 36,
                height: 36,
                child: Center(
                  child: SizedBox(
                    width: 24,
                    height: 24,
                    child: ClipRect(
                      child: OverflowBox(
                        alignment: Alignment.centerLeft,
                        minWidth: 182,
                        maxWidth: 182,
                        child: SvgPicture.asset(
                          colors.dark
                              ? 'assets/brand-dark.svg'
                              : 'assets/brand.svg',
                          width: 182,
                          height: 24,
                          key: ValueKey('brand-rail-${colors.dark}'),
                          theme: SvgTheme(currentColor: colors.text),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(height: 12),
          DshIcon(
            LucideIcons.circlePlus,
            label: shortcutHint('new', '新建会话'),
            size: 36,
            color: colors.text,
            onPressed: c.connected ? startConversation : null,
          ),
          const SizedBox(height: 12),
          DshIcon(
            LucideIcons.grid2x2,
            label: '插件',
            size: 36,
            color: colors.text,
            onPressed: () => settings('plugins'),
          ),
          const SizedBox(height: 12),
          DshIcon(
            LucideIcons.folderPlus,
            label: '添加工作区',
            size: 36,
            color: colors.text,
            onPressed: c.connected ? addWorkspace : null,
          ),
          const SizedBox(height: 12),
          DshIcon(
            LucideIcons.search,
            label: shortcutHint('search', '搜索会话'),
            size: 36,
            color: colors.text,
            onPressed: openSearch,
          ),
          const Spacer(),
          accountEntries(compact: true),
          DshIcon(
            LucideIcons.settings,
            label: shortcutHint('settings', '设置'),
            size: 36,
            color: colors.text,
            onPressed: () => settings(),
          ),
          const SizedBox(height: 12),
        ],
      ),
    );
  }

  Widget _divider(ValueChanged<double> move) => MouseRegion(
    cursor: SystemMouseCursors.resizeColumn,
    child: GestureDetector(
      onHorizontalDragUpdate: (d) => move(d.delta.dx),
      onHorizontalDragEnd: (_) => saveLayout(),
      child: Container(
        width: 1,
        color: DshColors(context).border,
        child: const SizedBox.expand(),
      ),
    ),
  );
  Widget sidebar() {
    final colors = DshColors(context);
    final query = search.text.toLowerCase();
    final rows = <Widget>[];
    final known = <String>{};
    final visibleSessions = c.sessions.where((session) {
      final archived = c.archivedSessionIds.contains(session.id);
      return switch (archiveFilter) {
        'all' => true,
        'archived' => archived,
        _ => !archived,
      };
    }).toList();
    bool hasVisibleContent(SessionSummary session) =>
        !session.blank ||
        c.archivedSessionIds.contains(session.id) ||
        (session.id == c.selectedId && query.isEmpty);
    var groupCount = 0;
    for (final workspace in c.workspaces) {
      final id = workspace['workspaceId'] as String;
      final ids = (workspace['sessionIds'] as List? ?? []).cast<String>();
      final entries = visibleSessions
          .where((s) => ids.contains(s.id) || s.cwd == workspace['path'])
          .where(hasVisibleContent)
          .where(
            (s) =>
                query.isEmpty ||
                '${s.title} ${s.cwd}'.toLowerCase().contains(query),
          )
          .toList();
      known.addAll(entries.map((e) => e.id));
      if ((query.isNotEmpty || archiveFilter == 'archived') &&
          entries.isEmpty) {
        continue;
      }
      rows.add(
        Padding(
          padding: EdgeInsets.only(top: groupCount++ == 0 ? 0 : 4),
          child: WorkspaceTreeRow(
            title: '${workspace['title']}',
            path: '${workspace['path']}',
            expanded: groupExpanded(id),
            active: c.workspaceId == id || c.selected?.cwd == workspace['path'],
            onPressed: () => setGroupExpanded(id, !groupExpanded(id)),
            onMenu: (position) => workspaceMenu(workspace, position),
          ),
        ),
      );
      if (groupExpanded(id)) {
        for (final (index, entry) in entries.indexed) {
          rows.add(
            Padding(
              padding: EdgeInsets.only(top: index == 0 ? 2 : 0),
              child: sessionRow(entry),
            ),
          );
        }
      }
    }
    final loose = visibleSessions
        .where(
          (s) =>
              !known.contains(s.id) &&
              hasVisibleContent(s) &&
              (query.isEmpty ||
                  '${s.title} ${s.cwd}'.toLowerCase().contains(query)),
        )
        .toList();
    if (loose.isNotEmpty) {
      rows.add(
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 12, 0, 6),
          child: Text(
            '会话',
            style: TextStyle(fontSize: 12, color: colors.muted),
          ),
        ),
      );
      rows.addAll(loose.map(sessionRow));
    }
    return Material(
      color: colors.sidebar,
      child: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 22, 12, 22),
            child: Row(
              children: [
                Expanded(
                  child: GestureDetector(
                    onTap: startConversation,
                    child: SvgPicture.asset(
                      colors.dark
                          ? 'assets/brand-dark.svg'
                          : 'assets/brand.svg',
                      height: 24,
                      key: ValueKey('brand-expanded-${colors.dark}'),
                      alignment: Alignment.centerLeft,
                      theme: SvgTheme(currentColor: colors.text),
                    ),
                  ),
                ),
                DshIcon(
                  LucideIcons.panelLeftClose,
                  label: shortcutHint('sidebar', '收起侧边栏'),
                  onPressed: () {
                    setState(() => sideOpen = false);
                    saveLayout();
                  },
                ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 14),
            child: DshButton(
              key: const Key('new-task'),
              height: 38,
              width: double.infinity,
              outline: true,
              icon: LucideIcons.circlePlus,
              trailing: shortcutBadge('new'),
              onPressed: c.connected ? startConversation : null,
              child: const Text('新会话'),
            ),
          ),
          const SizedBox(height: 12),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            child: Align(
              alignment: Alignment.centerLeft,
              child: Semantics(
                label: '插件',
                button: true,
                child: DshButton(
                  key: const Key('open-plugins'),
                  icon: LucideIcons.grid2x2,
                  onPressed: () => settings('plugins'),
                  child: const Text('插件'),
                ),
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(18, 18, 10, 4),
            child: Row(
              children: [
                Text(
                  '工作区',
                  style: TextStyle(fontSize: 13, color: colors.muted),
                ),
                const Spacer(),
                DshIcon(
                  LucideIcons.search,
                  label: shortcutHint('search', '搜索会话'),
                  active: showSearch,
                  onPressed: () => setState(() => showSearch = !showSearch),
                ),
                SizedBox(
                  width: 28,
                  height: 28,
                  child: PopupMenuButton<String>(
                    tooltip: '视图选项',
                    padding: EdgeInsets.zero,
                    constraints: const BoxConstraints(
                      minWidth: 220,
                      maxWidth: 280,
                    ),
                    icon: DshGlyph(
                      LucideIcons.slidersHorizontal,
                      size: 17,
                      color: colors.muted,
                    ),
                    onSelected: (v) {
                      if (['hidden', 'all', 'archived'].contains(v)) {
                        setArchiveFilter(v);
                      }
                      if (v == 'refresh') unawaited(c.run(c.refreshSessions));
                      if (v == 'archive') settings('archive');
                    },
                    itemBuilder: (_) => [
                      for (final entry in const {
                        'hidden': '隐藏已归档',
                        'all': '全部对话',
                        'archived': '仅显示已归档',
                      }.entries)
                        CheckedPopupMenuItem(
                          value: entry.key,
                          checked: archiveFilter == entry.key,
                          child: Text(entry.value),
                        ),
                      const PopupMenuDivider(),
                      const PopupMenuItem(
                        value: 'refresh',
                        child: Text('刷新会话'),
                      ),
                      const PopupMenuItem(
                        value: 'archive',
                        child: Text('归档管理'),
                      ),
                    ],
                  ),
                ),
                DshIcon(
                  LucideIcons.folderPlus,
                  label: '添加工作区',
                  onPressed: c.connected ? addWorkspace : null,
                ),
              ],
            ),
          ),
          if (showSearch)
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 4, 12, 8),
              child: DshField(
                controller: search,
                focusNode: searchFocus,
                hint: '搜索会话',
                autofocus: true,
                onChanged: (_) => setState(() {}),
              ),
            ),
          Expanded(
            child: rows.isEmpty
                ? Align(
                    alignment: Alignment.topLeft,
                    child: Padding(
                      padding: const EdgeInsets.all(24),
                      child: Text(
                        c.connecting ? '正在连接…' : '暂无会话',
                        style: TextStyle(fontSize: 14, color: colors.muted),
                      ),
                    ),
                  )
                : ListView.builder(
                    padding: const EdgeInsets.symmetric(horizontal: 10),
                    itemCount: rows.length,
                    itemBuilder: (_, i) => rows[i],
                  ),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            child: accountEntries(),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 4, 12, 12),
            child: Row(
              children: [
                Expanded(
                  child: Align(
                    alignment: Alignment.centerLeft,
                    child: DshButton(
                      icon: LucideIcons.settings,
                      trailing: shortcutBadge('settings'),
                      onPressed: () => settings(),
                      child: const Text('设置'),
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

  Widget sessionRow(SessionSummary session) => Padding(
    padding: const EdgeInsets.only(bottom: 2),
    child: Material(
      color: c.selectedId == session.id
          ? DshColors(context).hover
          : Colors.transparent,
      borderRadius: BorderRadius.circular(7),
      child: InkWell(
        key: ValueKey('session-${session.id}'),
        borderRadius: BorderRadius.circular(7),
        onTap: () => c.run(() => c.select(session.id)),
        onSecondaryTapDown: (d) => sessionMenu(session, d.globalPosition),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(28, 5, 7, 5),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  session.displayTitle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: 14),
                ),
              ),
              if (session.running)
                const SizedBox(
                  width: 10,
                  height: 10,
                  child: CircularProgressIndicator(strokeWidth: 1.4),
                ),
              if (c.archivedSessionIds.contains(session.id))
                Tooltip(
                  message: '已归档',
                  child: DshGlyph(
                    LucideIcons.archive,
                    size: 13,
                    color: DshColors(context).muted,
                  ),
                ),
              if (c.pending.values.any((f) => f.sessionId == session.id))
                const DshGlyph(
                  LucideIcons.circleHelp,
                  size: 13,
                  color: Colors.orange,
                ),
              if (!session.blank && session.updatedAt > 0)
                Text(
                  relativeSessionAge(session.updatedAt),
                  style: TextStyle(
                    fontSize: 12,
                    color: DshColors(context).muted,
                  ),
                ),
            ],
          ),
        ),
      ),
    ),
  );
  Future<void> sessionMenu(SessionSummary session, Offset pos) async {
    final action = await showMenu<String>(
      context: context,
      position: RelativeRect.fromLTRB(pos.dx, pos.dy, pos.dx, pos.dy),
      items: [
        const PopupMenuItem(value: 'copy-id', child: Text('复制对话 ID')),
        const PopupMenuItem(value: 'rename', child: Text('重命名')),
        const PopupMenuItem(value: 'fork', child: Text('创建分支')),
        if (c.archivedSessionIds.contains(session.id))
          const PopupMenuItem(value: 'restore', child: Text('恢复归档'))
        else
          const PopupMenuItem(value: 'archive', child: Text('归档')),
      ],
    );
    if (!mounted) return;
    if (action == 'copy-id') {
      await Clipboard.setData(ClipboardData(text: session.id));
      return;
    }
    if (action == 'rename') {
      final owner = c.client;
      if (owner == null) {
        c.error = '请先连接服务';
        c.emit();
        return;
      }
      var base = c.titleEditBase(session.id);
      await editTextDialog(
        context,
        '重命名会话',
        session.title,
        onSubmit: (title) async {
          if (title.trim().isEmpty) throw const FormatException('请输入会话名称。');
          if (base == null) {
            throw DshException('title-unloaded', '标题状态尚未加载，请读取最新状态。');
          }
          await c.renameSession(
            session.id,
            title.trim(),
            expectedTitle: base,
            expectedClient: owner,
          );
        },
        recoveryRequired: (error) =>
            error is DshException &&
            (error.code == 'title-conflict' ||
                error.code == 'title-unloaded' ||
                error.outcomeUnknown),
        onRecover: () async {
          if (!identical(owner, c.client)) {
            throw StateError('服务连接已改变，请重新打开标题编辑。');
          }
          await c.refreshSessions();
          base = c.titleEditBase(session.id);
          if (base == null) throw StateError('无法读取当前标题状态。');
          return '当前标题：${base!['value'] ?? '新会话'}；编辑草稿已保留。';
        },
      );
    }
    if (action == 'archive') await c.run(() => c.archive(session.id));
    if (action == 'restore') {
      await c.run(() => c.archive(session.id, restore: true));
    }
    if (action == 'fork') {
      await c.run(() async {
        final result = await c.client!.call('session.fork', {
          'sessionId': session.id,
        }, true);
        await c.refreshSessions();
        await c.select(result['sessionId'] as String);
      });
    }
  }

  Future<void> workspaceMenu(Json workspace, Offset pos) async {
    final api = c.client;
    if (api == null) return;
    final action = await showMenu<String>(
      context: context,
      position: RelativeRect.fromLTRB(pos.dx, pos.dy, pos.dx, pos.dy),
      items: [
        const PopupMenuItem(value: 'rename', child: Text('重命名工作区')),
        const PopupMenuItem(value: 'open', child: Text('在文件管理器中打开')),
        const PopupMenuItem(value: 'delete', child: Text('删除工作区')),
      ],
    );
    if (!mounted) return;
    if (action == 'delete') {
      final confirmed = await confirmAction(
        context,
        '删除工作区',
        '将把“${workspace['title']}”从工作区列表移除。文件夹和会话记录保留，会话移至未分组。',
        action: '删除工作区',
      );
      if (!confirmed || !mounted || c.client != api) return;
      await c.run(() async {
        await api.call('workspace.delete', {
          'workspaceId': workspace['workspaceId'],
        }, true);
        if (c.client != api) return;
        if (c.workspaceId == workspace['workspaceId']) c.workspaceId = null;
        groupExpansion.remove('${workspace['workspaceId']}');
        await c.refreshSessions();
        saveLayout();
      });
      return;
    }
    if (action == 'rename') {
      final title = await editTextDialog(
        context,
        '重命名工作区',
        '${workspace['title']}',
      );
      if (title != null) {
        await c.run(() async {
          await c.client!.call('workspace.rename', {
            'workspaceId': workspace['workspaceId'],
            'title': title,
          }, true);
          await c.refreshSessions();
        });
      }
    }
    if (action == 'open') {
      await c.run(() async {
        await c.client!.call('host.openPath', {
          'path': workspace['path'],
        }, true);
      });
    }
  }

  Future<void> addWorkspace() async {
    final api = c.client;
    if (api == null) return;
    final path = await getDirectoryPath();
    if (!mounted || path == null || c.client != api) return;
    await showDialog<String>(
      context: context,
      barrierDismissible: false,
      builder: (_) => NewTaskDialog(
        initialPath: path,
        onCreate: (path, scratch) async {
          if (c.client != api) throw StateError('连接已切换，请重新选择工作区。');
          await api.request(
            '/__dsh-artifacts/workspace-settings',
            body: {'path': path, 'location': scratch},
            mutation: true,
          );
          if (c.client != api) throw StateError('连接已切换，请重新选择工作区。');
          await c.addWorkspace(path);
        },
      ),
    );
  }

  Future<void> chooseWorkspace() async {
    final overlay =
        Overlay.of(context).context.findRenderObject()! as RenderBox;
    final anchor = workspaceAnchor.currentContext?.findRenderObject();
    final position = anchor is RenderBox
        ? anchor.localToGlobal(Offset(0, anchor.size.height), ancestor: overlay)
        : const Offset(280, 120);
    final id = await showMenu<String>(
      context: context,
      position: RelativeRect.fromRect(
        Rect.fromLTWH(position.dx, position.dy, 0, 0),
        Offset.zero & overlay.size,
      ),
      constraints: const BoxConstraints(minWidth: 280, maxWidth: 360),
      items: [
        for (final w in c.workspaces)
          PopupMenuItem(
            value: '${w['workspaceId']}',
            height: 38,
            child: Tooltip(
              message: displayPath('${w['path']}'),
              child: Row(
                children: [
                  const DshGlyph(LucideIcons.folder, size: 16),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      '${w['title']}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: 14),
                    ),
                  ),
                  if (w['workspaceId'] == c.workspaceId)
                    const DshGlyph(LucideIcons.check, size: 14),
                ],
              ),
            ),
          ),
        const PopupMenuDivider(),
        const PopupMenuItem(
          value: '__add',
          height: 38,
          child: Row(
            children: [
              DshGlyph(LucideIcons.plus, size: 16),
              SizedBox(width: 10),
              Text('添加工作目录'),
            ],
          ),
        ),
        const PopupMenuItem(
          value: '__git',
          height: 38,
          child: Text('从 Git 克隆工作目录'),
        ),
        const PopupMenuItem(
          value: '__cloud',
          height: 38,
          child: Text('Cloud · 云端 Git 仓库'),
        ),
        const PopupMenuItem(
          value: '__ssh',
          height: 38,
          child: Text('SSH 远程工作目录'),
        ),
      ],
    );
    if (!mounted) return;
    if (id == '__add') {
      await addWorkspace();
    } else if (['__git', '__cloud', '__ssh'].contains(id)) {
      final api = c.client;
      if (api == null) return;
      final requestClient = DshClient(
        api.baseUri.toString(),
        timeout: const Duration(minutes: 10),
      );
      Json? result;
      try {
        result = await showDialog<Json>(
          context: context,
          barrierDismissible: false,
          builder: (_) =>
              WorkspaceSourceDialog(api: requestClient, kind: id!.substring(2)),
        );
      } finally {
        await requestClient.close();
      }
      if (!mounted || c.client != api || result == null) return;
      if (result['url'] is String) {
        await c.run(() => c.connect(result!['url'] as String));
      } else if (result['workspaceId'] is String) {
        await c.run(c.refreshSessions);
        if (!mounted || c.client != api) return;
        c.workspaceId = result['workspaceId'] as String;
        setGroupExpanded(c.workspaceId!, true);
        await c.run(c.startConversation);
      }
    } else if (id != null) {
      c.workspaceId = id;
      setGroupExpanded(id, true);
      await c.run(c.startConversation);
    }
  }

  Future<void> settings([String page = 'general']) => showDialog<void>(
    context: context,
    barrierColor: Colors.transparent,
    barrierDismissible: false,
    builder: (_) => SettingsShell(
      controller: c,
      initialPage: page,
      onOpenPlugin: (row) {
        final id = '${row['id'] ?? row['entryId'] ?? row['name'] ?? ''}';
        Navigator.of(context).pop();
        if (id.contains('workbench') || id.contains('sidebar')) {
          openDock('files');
        } else if (id.contains('context')) {
          conversationViewRequest.value = 'user-message-rail';
        } else if (id == 'dsh-artifacts') {
          conversationViewRequest.value = 'artifacts';
        } else {
          conversationViewRequest.value = 'conversation';
          c.composerFocus.value++;
        }
      },
    ),
  );

  Future<void> sessionFeedback() async {
    final session = c.selectedId, api = c.client;
    if (session == null || api == null) return;
    await showDialog<void>(
      context: context,
      builder: (_) => SessionFeedbackDialog(api: api, sessionId: session),
    );
  }
}

class SessionFeedbackDialog extends StatefulWidget {
  const SessionFeedbackDialog({
    super.key,
    required this.api,
    required this.sessionId,
  });
  final DshClient api;
  final String sessionId;
  @override
  State<SessionFeedbackDialog> createState() => _SessionFeedbackDialogState();
}

class _SessionFeedbackDialogState extends State<SessionFeedbackDialog> {
  final note = TextEditingController();
  String category = '';
  bool busy = false;
  String? error;
  String? savedPayload;
  String requestId = newRequestId();
  static const categories = {
    'task-result': '任务结果',
    'instruction-following': '指令遵循',
    'product-interaction': '交互体验',
    'service-stability': '服务稳定性',
    'resource-cost': '资源与费用',
    'security-privacy-permission': '安全、隐私与权限',
    'other': '其他',
  };

  @override
  void dispose() {
    note.dispose();
    super.dispose();
  }

  Future<void> save() async {
    if (busy) return;
    if (note.text.trim().isEmpty && category.isEmpty) return;
    final payloadKey = '${category.length}:$category${note.text.trim()}';
    if (savedPayload != payloadKey) {
      savedPayload = payloadKey;
      requestId = newRequestId();
    }
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final value = feedbackValue(
        await widget.api.call('sessionFeedback.record', {
          'sessionId': widget.sessionId,
          'requestId': requestId,
          if (note.text.trim().isNotEmpty) 'text': note.text.trim(),
          'category': ?(category.isEmpty ? null : category),
        }, true),
      );
      if (value['recorded'] != true) {
        throw DshException('protocol', '服务未确认保存反馈。');
      }
      if (mounted) Navigator.pop(context);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('会话反馈', style: TextStyle(fontSize: 17)),
    content: SizedBox(
      width: 500,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('记录对整个会话的意见，不会启动新的模型请求。', style: TextStyle(fontSize: 13)),
          const SizedBox(height: 14),
          DropdownButtonFormField<String>(
            key: ValueKey(category),
            initialValue: category.isEmpty ? null : category,
            decoration: const InputDecoration(
              labelText: '反馈分类（可选）',
              border: OutlineInputBorder(),
            ),
            items: [
              for (final entry in categories.entries)
                DropdownMenuItem(value: entry.key, child: Text(entry.value)),
            ],
            onChanged: busy
                ? null
                : (value) => setState(() => category = value ?? ''),
          ),
          const SizedBox(height: 12),
          TextField(
            controller: note,
            minLines: 4,
            maxLines: 8,
            enabled: !busy,
            decoration: const InputDecoration(
              labelText: '补充说明（可选）',
              border: OutlineInputBorder(),
            ),
          ),
          if (error != null)
            Padding(
              padding: const EdgeInsets.only(top: 10),
              child: Text(error!, style: const TextStyle(color: Colors.red)),
            ),
        ],
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
        child: Text(busy ? '保存中…' : '保存反馈'),
      ),
    ],
  );
}

class NewTaskDialog extends StatefulWidget {
  const NewTaskDialog({super.key, required this.initialPath, this.onCreate});
  final String initialPath;
  final Future<void> Function(String path, String scratch)? onCreate;
  @override
  State<NewTaskDialog> createState() => _NewTaskDialogState();
}

class _NewTaskDialogState extends State<NewTaskDialog> {
  late final input = TextEditingController(
    text: displayPath(widget.initialPath),
  );
  final scratch = TextEditingController();
  bool advanced = false, busy = false;
  String? error;
  @override
  void dispose() {
    input.dispose();
    scratch.dispose();
    super.dispose();
  }

  Future<void> create() async {
    if (busy || input.text.trim().isEmpty) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await widget.onCreate?.call(input.text.trim(), scratch.text.trim());
      if (mounted) Navigator.pop(context, input.text.trim());
    } catch (e) {
      if (mounted) {
        setState(() {
          busy = false;
          error = '$e';
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: AlertDialog(
      title: const Text('添加工作区', style: TextStyle(fontSize: 18)),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (widget.initialPath.isNotEmpty)
                Container(
                  width: double.infinity,
                  padding: const EdgeInsets.symmetric(
                    horizontal: 12,
                    vertical: 10,
                  ),
                  decoration: BoxDecoration(
                    color: DshColors(context).layer,
                    borderRadius: BorderRadius.circular(8),
                  ),
                  child: SelectableText(
                    displayPath(widget.initialPath),
                    style: const TextStyle(fontSize: 14),
                  ),
                )
              else
                DshField(
                  key: const Key('working-directory'),
                  controller: input,
                  hint: '工作目录',
                  enabled: !busy,
                ),
              const SizedBox(height: 16),
              DshButton(
                icon: advanced
                    ? LucideIcons.chevronDown
                    : LucideIcons.chevronRight,
                onPressed: busy
                    ? null
                    : () => setState(() => advanced = !advanced),
                child: const Text('高级设置'),
              ),
              if (advanced) ...[
                const SizedBox(height: 12),
                const Text('垃圾槽位置', style: TextStyle(fontSize: 14)),
                const SizedBox(height: 8),
                Row(
                  children: [
                    Expanded(
                      child: DshField(
                        controller: scratch,
                        hint: '使用全局位置',
                        enabled: !busy,
                      ),
                    ),
                    const SizedBox(width: 8),
                    DshButton(
                      outline: true,
                      onPressed: busy
                          ? null
                          : () async {
                              final path = await getDirectoryPath();
                              if (mounted && path != null) {
                                scratch.text = displayPath(path);
                              }
                            },
                      child: const Text('选择目录'),
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                const Text(
                  '留空使用全局位置。可选择空目录或已有垃圾槽，应用于此工作区的新运行。',
                  style: TextStyle(fontSize: 12, height: 1.5),
                ),
              ],
              if (error != null)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: Text(
                    error!,
                    style: const TextStyle(color: Colors.red),
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
          key: const Key('create-task'),
          primary: true,
          onPressed: busy ? null : create,
          child: Text(busy ? '正在添加…' : '添加'),
        ),
      ],
    ),
  );
}

Future<void> connectionSettings(BuildContext context, DesktopController c) =>
    showDialog<void>(
      context: context,
      builder: (_) => _ConnectionDialog(controller: c),
    );

class _ConnectionDialog extends StatefulWidget {
  const _ConnectionDialog({required this.controller});
  final DesktopController controller;
  @override
  State<_ConnectionDialog> createState() => _ConnectionDialogState();
}

class _ConnectionDialogState extends State<_ConnectionDialog> {
  late final address = TextEditingController(
    text: widget.controller.preferences.address,
  );
  late final exe = TextEditingController(
    text: widget.controller.preferences.executable,
  );
  bool busy = false;
  String? error;
  @override
  void dispose() {
    address.dispose();
    exe.dispose();
    super.dispose();
  }

  Future<void> submit(bool start) async {
    setState(() => busy = true);
    try {
      localHostUri(address.text);
      final c = widget.controller;
      c.preferences.address = address.text;
      c.preferences.executable = exe.text;
      if (start) {
        await c.startHost();
      } else {
        await c.connect(address.text);
      }
      if (mounted) Navigator.pop(context);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('本机服务', style: TextStyle(fontSize: 18)),
    content: SizedBox(
      width: 500,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          DshField(controller: address, hint: 'http://127.0.0.1:58080'),
          const SizedBox(height: 8),
          Align(
            alignment: Alignment.centerLeft,
            child: DshButton(
              outline: true,
              onPressed: busy
                  ? null
                  : () {
                      address.text = 'http://127.0.0.1:58080';
                      exe.text = HostLauncher.discover();
                      submit(false);
                    },
              child: const Text('使用已安装版本的配置'),
            ),
          ),
          if (widget.controller.host != null)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: SelectableText(
                '配置目录：${displayPath(widget.controller.host!.home)}',
                style: const TextStyle(fontSize: 12),
              ),
            ),
          const SizedBox(height: 16),
          Row(
            children: [
              Expanded(
                child: DshField(controller: exe, hint: 'Host 程序位置'),
              ),
              DshIcon(
                LucideIcons.folderOpen,
                label: '选择程序',
                onPressed: () async {
                  final file = await openFile(
                    acceptedTypeGroups: [
                      const XTypeGroup(label: '程序', extensions: ['exe']),
                    ],
                  );
                  if (mounted && file != null) exe.text = file.path;
                },
              ),
            ],
          ),
          if (error != null)
            Text(error!, style: const TextStyle(color: Colors.red)),
          if (busy) const LinearProgressIndicator(),
        ],
      ),
    ),
    actions: [
      DshButton(
        onPressed: busy ? null : () => Navigator.pop(context),
        child: const Text('取消'),
      ),
      DshButton(
        onPressed: busy ? null : () => submit(true),
        outline: true,
        child: const Text('启动并连接'),
      ),
      DshButton(
        onPressed: busy ? null : () => submit(false),
        primary: true,
        child: const Text('连接'),
      ),
    ],
  );
}
