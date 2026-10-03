import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:xterm/xterm.dart' show TerminalView;
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:file_selector/file_selector.dart';
import 'package:dsh_client/dsh_client.dart';

import '../design/primitives.dart';
import '../design/loading.dart';
import '../design/shortcuts.dart';
import '../design/motion.dart';
import '../design/breakpoints.dart';
import '../design/error.dart';
import '../l10n/zh.dart';
import '../l10n/conversation_zh.dart';
import '../l10n/plugin_settings_zh.dart';
import 'command_palette.dart';
import '../src/controller.dart';
import '../src/preferences.dart';
import '../src/conversation.dart';
import 'conversation/feedback_controller.dart';
import 'conversation/session_log_export.dart';
import 'settings/settings_shell.dart';
import 'settings/plugin_page.dart';
import 'settings/models_page.dart' show AccountLoginDialog;
import 'workbench/workbench_panel.dart';
import 'workbench/plan_preview.dart';
import '../src/resource_diagnostics.dart';
import 'workspace_tree_row.dart';
import 'sidebar_entries.dart';
import 'row_menu.dart';
import 'workspace_source_dialog.dart';
import 'knowledge/knowledge_page.dart';
import 'schedule/schedule_page.dart';
import 'account_menu.dart';

import 'package:dsh_desktop/design/typography.dart';

String relativeSessionAge(int updatedAt, {int? nowMillis}) {
  final elapsed =
      ((nowMillis ?? DateTime.now().millisecondsSinceEpoch) - updatedAt).clamp(
        0,
        1 << 62,
      );
  const minute = 60000, hour = 60 * minute, day = 24 * hour;
  if (elapsed < minute) return DshShellZh.justNow;
  if (elapsed < hour) return DshShellZh.minutesAgo(count: elapsed ~/ minute);
  if (elapsed < day) return DshShellZh.hoursAgo(count: elapsed ~/ hour);
  if (elapsed < 30 * day) return DshShellZh.daysAgo(count: elapsed ~/ day);
  if (elapsed < 365 * day) {
    return DshShellZh.monthsAgo(count: elapsed ~/ (30 * day));
  }
  return DshShellZh.yearsAgo(count: elapsed ~/ (365 * day));
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
    if (!identical(previewClient, c.client)) scheduleFocus = null;
    workbenchKey = GlobalKey<WorkbenchPanelState>();
    if (!identical(previewClient, c.client)) {
      syncingSessions.clear();
      syncFailures.clear();
      syncRetries.clear();
    }
    previewClient = c.client;
    previewSession = c.selectedId;
    planPreviews.scope(c.client, c.selectedId);
    dockTab = 'start';
    fileRequest = null;
    terminalRequest = null;
    if (mounted) setState(() {});
  }

  void openPlan(TranscriptItem item) {
    planPreviews.scope(c.client, c.selectedId);
    planPreviews.open(item);
    openDock('plans');
  }

  void closeDock() {
    cancelStartToolIntent();
    scaffoldKey.currentState?.closeEndDrawer();
    restoreFocus(dockReturnFocus);
    dockReturnFocus = null;
    planPreviews.clear();
    setState(() {
      dockOpen = false;
      terminalRequest = null;
      if (dockTab == 'plans') dockTab = 'start';
    });
  }

  void planSource() {
    conversationViewRequest.value = 'conversation';
    if (DshBreakpoints.overlayWorkbench(availableWidth)) {
      closeDock();
    }
    c.composerFocus.value++;
  }

  DesktopController get c => widget.controller;
  final search = TextEditingController();
  final searchFocus = FocusNode();
  final shellFocus = FocusNode();
  final sidebarFocus = FocusScopeNode(debugLabel: 'sidebar-region');
  final conversationFocus = FocusScopeNode(debugLabel: 'conversation-region');
  final workbenchFocus = FocusScopeNode(debugLabel: 'workbench-region');
  GlobalKey<WorkbenchPanelState> workbenchKey =
      GlobalKey<WorkbenchPanelState>();
  FocusNode? dockReturnFocus;
  List<String> get visibleSessionIds {
    final query = search.text.toLowerCase();
    final sessions = c.sessions.where((session) {
      final archived = c.archivedSessionIds.contains(session.id);
      final inArchive =
          archiveFilter == 'all' ||
          (archiveFilter == 'archived' ? archived : !archived);
      return inArchive &&
          (!session.blank ||
              archived ||
              (session.id == c.selectedId && query.isEmpty)) &&
          (query.isEmpty ||
              '${session.title} ${session.cwd}'.toLowerCase().contains(query));
    }).toList();
    final workspaceIds = c.workspaces.map((w) => w['workspaceId']).toSet();
    return [
      for (final workspace in c.workspaces)
        if (groupExpandedForView('${workspace['workspaceId']}'))
          for (final session in sessions)
            if (c.workspaceOf(session)?['workspaceId'] ==
                workspace['workspaceId'])
              session.id,
      for (final session in sessions)
        if (!workspaceIds.contains(c.workspaceOf(session)?['workspaceId']))
          session.id,
    ];
  }

  final syncingSessions = <String>{};
  final syncFailures = <String, Object>{};
  final syncRetries = <String, Future<void> Function()>{};

  Widget focusRegion(FocusScopeNode node, Widget child) =>
      TweenAnimationBuilder<double>(
        tween: Tween(begin: 0, end: 1),
        duration: DshMotion.duration(context, DshMotion.panel),
        curve: DshMotion.curve,
        builder: (context, opacity, child) =>
            Opacity(opacity: opacity, child: child),
        child: FocusScope(
          node: node,
          child: FocusTraversalGroup(child: child),
        ),
      );

  void refreshShell() {
    if (mounted) setState(() {});
  }

  void restoreFocus(FocusNode? node) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || ModalRoute.of(context)?.isCurrent == false) return;
      if (node?.context != null && node!.canRequestFocus) {
        node.requestFocus();
      } else {
        conversationFocus.requestFocus();
      }
    });
  }

  void cycleFocus({bool reverse = false}) {
    // Drawers share the shell route but remain modal focus boundaries.
    if (scaffoldKey.currentState?.isDrawerOpen == true ||
        scaffoldKey.currentState?.isEndDrawerOpen == true) {
      return;
    }
    final scopes = [
      if (!DshBreakpoints.collapseSidebar(availableWidth) ||
          scaffoldKey.currentState?.isDrawerOpen == true)
        sidebarFocus,
      conversationFocus,
      if (dockOpen) workbenchFocus,
    ];
    final current = scopes.indexWhere((scope) => scope.hasFocus);
    final next = scopes[(current + (reverse ? -1 : 1)) % scopes.length];
    next.requestFocus();
    if (next.focusedChild == null) next.nextFocus();
  }

  void cycleSession({bool reverse = false}) {
    if (visibleSessionIds.isEmpty) return;
    final current = visibleSessionIds.indexOf(c.selectedId ?? '');
    final index = (current + (reverse ? -1 : 1)) % visibleSessionIds.length;
    unawaited(openConversation(visibleSessionIds[index]));
  }

  Future<void> syncSession(String id, Future<void> Function() operation) async {
    if (syncingSessions.contains(id)) return;
    final owner = c.client;
    setState(() {
      syncingSessions.add(id);
      syncFailures.remove(id);
      syncRetries.remove(id);
    });
    try {
      await operation();
    } catch (error) {
      if (mounted && identical(owner, c.client)) {
        setState(() {
          syncFailures[id] = error;
          syncRetries[id] = operation;
        });
      }
    } finally {
      if (mounted && identical(owner, c.client)) {
        setState(() => syncingSessions.remove(id));
      }
    }
  }

  Future<void> showSyncFailure(SessionSummary session) async {
    final failure = syncFailures[session.id], retry = syncRetries[session.id];
    if (failure == null) return;
    final owner = c.client;
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text(DshZh.syncFailure(session.displayTitle)),
        content: SizedBox(
          width: 480,
          child: DshErrorView(
            error: failure,
            onRetry: retry == null
                ? null
                : () {
                    Navigator.pop(dialogContext);
                    if (identical(owner, c.client)) {
                      unawaited(syncSession(session.id, retry));
                    }
                  },
          ),
        ),
        actions: [
          DshButton(
            onPressed: () {
              Navigator.pop(dialogContext);
              if (identical(owner, c.client)) {
                unawaited(c.run(c.refreshSessions));
              }
            },
            child: const Text(DshShellZh.refreshSessions),
          ),
          DshButton(
            onPressed: () => Navigator.pop(dialogContext),
            child: const Text(DshZh.cancel),
          ),
        ],
      ),
    );
  }

  final scaffoldKey = GlobalKey<ScaffoldState>();
  final workspaceAnchor = GlobalKey();
  final conversationViewRequest = ValueNotifier<String>('');

  /// Global page shown in place of the conversation, e.g. `schedule`.
  String? mainPanel;
  String? scheduleFocus;
  Object? openedHeaderScope;
  Object get connectionScope => (c, c.client, c.host);
  Object get headerActionScope =>
      (c, c.client, c.host, c.selectedId, c.selectionRevision);

  void openPanel(String id, {String? focus}) {
    scaffoldKey.currentState?.closeDrawer();
    setState(() {
      mainPanel = id;
      scheduleFocus = focus;
    });
  }

  void closePanel() => setState(() => mainPanel = null);

  Future<void> openConversation(String id) async {
    scaffoldKey.currentState?.closeDrawer();
    closePanel();
    await c.run(() => c.select(id));
  }

  final Map<String, bool> groupExpansion = {};
  final Map<String, bool> filteredGroupExpansion = {};
  bool showSearch = false, sideOpen = true, dockOpen = false;
  double sidebarWidth = 280, dockWidth = 470, chatWidth = 0;
  double availableWidth = 0;
  String dockTab = 'start';
  int dockRequest = 0;
  bool preparingTool = false;
  int toolIntentEpoch = 0;
  int? pendingToolIntent, pendingToolSelection;
  String? pendingToolSession;
  Object? pendingToolContext;
  Object get toolContextScope =>
      (c, c.client, c.host, c.workspaceId, c.workspaceTargetRevision);
  TerminalCreateRequest? terminalRequest;
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
    c.addListener(toolIntentScopeChanged);
    c.addListener(refreshShell);
    c.computerUseRequests.addListener(showComputerUse);
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
      oldWidget.controller.removeListener(toolIntentScopeChanged);
      oldWidget.controller.removeListener(refreshShell);
      oldWidget.controller.computerUseRequests.removeListener(showComputerUse);
      c.addListener(previewScopeChanged);
      c.addListener(toolIntentScopeChanged);
      c.addListener(refreshShell);
      c.computerUseRequests.addListener(showComputerUse);
      cancelStartToolIntent();
      previewScopeChanged();
    }
  }

  @override
  void dispose() {
    c.removeListener(previewScopeChanged);
    c.removeListener(toolIntentScopeChanged);
    c.removeListener(refreshShell);
    c.computerUseRequests.removeListener(showComputerUse);
    planPreviews.dispose();
    FocusManager.instance.removeEarlyKeyEventHandler(onShortcut);
    shellFocus.dispose();
    sidebarFocus.dispose();
    conversationFocus.dispose();
    workbenchFocus.dispose();
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
    setState(() {
      filteredGroupExpansion.clear();
      c.preferences.layout['archiveFilter'] = value;
    });
    saveLayout();
  }

  String shortcutHint(String action, String label) =>
      '$label · ${shortcutLabel(configuredShortcuts(c)[action]!)}';

  Widget shortcutBadge(String action) => ConstrainedBox(
    constraints: const BoxConstraints(maxWidth: 84),
    child: Text(
      shortcutLabel(configuredShortcuts(c)[action]!),
      key: ValueKey('sidebar-shortcut-$action'),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TextStyle(
        fontSize: DshTypography.sizeCaption,
        color: DshColors(context).muted,
      ),
    ),
  );

  void startConversation() {
    scaffoldKey.currentState?.closeDrawer();
    closePanel();
    final workspace = c.workspaceId;
    if (workspace != null) setGroupExpanded(workspace, true);
    unawaited(c.run(c.startConversation));
    c.composerFocus.value++;
  }

  bool groupExpanded(String id) => groupExpansion[id] ?? c.workspaceId == id;

  // Search and the archive-only view reveal their matches without changing
  // the user's saved folder expansion.
  bool get filteringGroups =>
      search.text.isNotEmpty || archiveFilter == 'archived';

  bool groupExpandedForView(String id) =>
      filteringGroups ? filteredGroupExpansion[id] ?? true : groupExpanded(id);

  void toggleGroupExpansion(String id) {
    if (filteringGroups) {
      setState(() => filteredGroupExpansion[id] = !groupExpandedForView(id));
    } else {
      setGroupExpanded(id, !groupExpanded(id));
    }
  }

  void toggleSidebarSearch() {
    setState(() {
      showSearch = !showSearch;
      filteredGroupExpansion.clear();
      if (!showSearch) search.clear();
    });
    if (!showSearch) {
      searchFocus.unfocus();
    }
  }

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

  void openDock([String tab = 'start']) {
    if (!dockOpen) dockReturnFocus = FocusManager.instance.primaryFocus;
    setState(() {
      dockOpen = true;
      dockTab = tab;
      dockRequest++;
    });
    if (DshBreakpoints.overlayWorkbench(availableWidth)) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) scaffoldKey.currentState?.openEndDrawer();
      });
    }
  }

  /// The model started driving a Computer Use session. A drawer would cover
  /// the conversation on narrow windows; there the binding waits until the
  /// user opens the tab.
  void showComputerUse() {
    if (!mounted || c.computerUse == null) return;
    if (dockOpen || !DshBreakpoints.overlayWorkbench(availableWidth)) {
      openDock('computer-use');
    }
  }

  void selectDockTab(String tab) {
    if (dockTab != tab) setState(() => dockTab = tab);
  }

  void fileRequestHandled(FileOpenRequest request) {
    if (identical(fileRequest, request)) fileRequest = null;
  }

  void terminalRequestHandled(TerminalCreateRequest request) {
    if (identical(terminalRequest, request)) {
      setState(() => terminalRequest = null);
    }
  }

  void cancelStartToolIntent() {
    toolIntentEpoch++;
    pendingToolIntent = null;
    pendingToolContext = null;
    terminalRequest = null;
  }

  void toolIntentScopeChanged() {
    if (pendingToolIntent == null) return;
    final sameSelection =
        c.selectionRevision == pendingToolSelection &&
        c.selectedId == pendingToolSession;
    // A controller-owned create may adopt the Hero draft once. Every other
    // navigation invalidates the pending UI intent independently of admission.
    final draftAdoption =
        pendingToolSession == null &&
        c.selectionAdoptsDraft &&
        c.selectionRevision == pendingToolSelection! + 1;
    if (pendingToolContext != toolContextScope ||
        (!sameSelection && !draftAdoption)) {
      cancelStartToolIntent();
    }
  }

  void workbenchTabClosed(String tab) {
    if (tab != 'start') return;
    cancelStartToolIntent();
    if (mounted) setState(() {});
  }

  Future<void> openStartTool(String tab) async {
    if (preparingTool || !c.connected) return;
    final intent = ++toolIntentEpoch;
    final contextScope = toolContextScope;
    final api = c.client;
    String? session = c.selectedId;
    pendingToolIntent = intent;
    pendingToolContext = contextScope;
    pendingToolSelection = c.selectionRevision;
    pendingToolSession = session;
    setState(() => preparingTool = true);
    try {
      if (session == null) {
        final path = c.currentWorkspace?['path'] as String?;
        if (path == null || path.trim().isEmpty) return;
        // Controller admission owns both the workspace and the unsent Hero
        // draft. A stale Host/selection/workspace request returns no session.
        await c.run(() async {
          session = await c.create(path);
        });
      }
      if (!mounted ||
          pendingToolIntent != intent ||
          toolIntentEpoch != intent ||
          contextScope != toolContextScope ||
          api != c.client ||
          session == null ||
          c.selectedId != session) {
        return;
      }
      if (tab == 'terminal') terminalRequest = TerminalCreateRequest();
      openDock(tab);
    } finally {
      if (pendingToolIntent == intent) {
        pendingToolIntent = null;
        pendingToolContext = null;
      }
      if (mounted) setState(() => preparingTool = false);
    }
  }

  void toggleSidebar() {
    if (DshBreakpoints.collapseSidebar(availableWidth)) {
      if (scaffoldKey.currentState?.isDrawerOpen == true) {
        scaffoldKey.currentState?.closeDrawer();
      } else {
        scaffoldKey.currentState?.openDrawer();
      }
      return;
    }
    setState(() => sideOpen = !sideOpen);
    saveLayout();
  }

  Future<void> openSearch() async {
    final previousFocus = FocusManager.instance.primaryFocus;
    final owner = c.client;
    final commands = <DshCommand>[
      for (final session in c.sessions.where(
        (s) => !s.blank && !c.archivedSessionIds.contains(s.id),
      ))
        DshCommand(
          id: 'session-${session.id}',
          group: DshZh.sessionsGroup,
          title: session.displayTitle,
          subtitle: displayPath(session.cwd),
          icon: DshIcons.messageCircle.data,
          onInvoke: () => unawaited(openConversation(session.id)),
        ),
      DshCommand(
        id: 'schedule',
        group: DshZh.pagesGroup,
        title: DshShellZh.scheduledTasks,
        icon: DshIcons.alarmClock.data,
        onInvoke: () => openPanel('schedule'),
      ),
      DshCommand(
        id: 'knowledge',
        group: DshZh.pagesGroup,
        title: DshShellZh.knowledge,
        icon: DshIcons.bookOpen.data,
        onInvoke: () => openPanel('knowledge'),
      ),
      for (final page in settingsPages)
        DshCommand(
          id: 'settings-${page.id}',
          group: DshZh.pagesGroup,
          title: page.title,
          subtitle: DshShellZh.settings,
          icon: page.icon,
          searchTerms: page.id,
          onInvoke: () => unawaited(settings(page.id)),
        ),
      DshCommand(
        id: 'new',
        group: DshZh.commandsGroup,
        title: DshZh.newSession,
        icon: DshIcons.plus.data,
        shortcut: shortcutLabel(configuredShortcuts(c)['new']!),
        enabled: c.connected,
        onInvoke: startConversation,
      ),
      DshCommand(
        id: 'stop',
        group: DshZh.commandsGroup,
        title: DshZh.stopExecution,
        icon: DshIcons.stop.data,
        shortcut: shortcutLabel(configuredShortcuts(c)['stop']!),
        enabled: c.interruptible,
        onInvoke: () {
          if (c.interruptible) unawaited(c.run(c.stop));
        },
      ),
      DshCommand(
        id: 'refresh',
        group: DshZh.commandsGroup,
        title: DshShellZh.refreshSessions,
        icon: DshIcons.refreshCw.data,
        enabled: c.connected,
        onInvoke: () => unawaited(c.run(c.refreshSessions)),
      ),
      DshCommand(
        id: 'shortcuts',
        group: DshZh.commandsGroup,
        title: DshShellZh.editShortcuts,
        icon: DshIcons.keyboard.data,
        onInvoke: () => showDialog<void>(
          context: context,
          builder: (_) => ShortcutEditor(controller: c),
        ),
      ),
    ];
    final selected = await showDialog<DshCommand>(
      context: context,
      animationStyle: AnimationStyle(
        duration: DshMotion.duration(context, DshMotion.dialog),
        curve: DshMotion.curve,
      ),
      builder: (_) => DshCommandPalette(commands: commands),
    );
    if (!mounted) return;
    if (!identical(owner, c.client)) {
      restoreFocus(previousFocus);
      return;
    }
    if (selected == null) {
      restoreFocus(previousFocus);
    } else {
      selected.onInvoke();
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
      case 'focus-next':
        cycleFocus();
      case 'focus-previous':
        cycleFocus(reverse: true);
      case 'cycle-next':
      case 'cycle-previous':
        final reverse = action == 'cycle-previous';
        if (workbenchFocus.hasFocus) {
          workbenchKey.currentState?.cycleTab(reverse: reverse);
        } else {
          cycleSession(reverse: reverse);
        }
      case 'stop':
        if (c.interruptible) unawaited(c.run(c.stop));
      case 'sidebar':
        toggleSidebar();
      case 'new':
        startConversation();
      case 'search':
        openSearch();
      case 'settings':
        settings();
      case 'composer':
        closePanel();
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted) c.composerFocus.value++;
        });
      case 'workbench':
        if (dockOpen) {
          closeDock();
        } else {
          openDock(dockTab);
        }
    }
    if (action.startsWith('session-')) {
      final index = int.tryParse(action.substring(8));
      if (index != null && index <= visibleSessionIds.length) {
        unawaited(openConversation(visibleSessionIds[index - 1]));
      }
    }
    return KeyEventResult.handled;
  }

  Widget workbenchPanel() => focusRegion(
    workbenchFocus,
    WorkbenchPanel(
      key: workbenchKey,
      controller: c,
      initialTab: dockTab,
      openRequest: dockRequest,
      onTabChanged: selectDockTab,
      onTabClosed: workbenchTabClosed,
      fileRequest: currentFileRequest,
      onFileRequestHandled: fileRequestHandled,
      planPreviews: planPreviews,
      onPlanSource: planSource,
      onOpenSettings: () => settings('environment'),
      onOpenStartTool: (tab) => unawaited(openStartTool(tab)),
      onSelectWorkspace: () => unawaited(chooseWorkspace()),
      preparingTool: preparingTool,
      terminalRequest: terminalRequest,
      onTerminalRequestHandled: terminalRequestHandled,
      onClose: closeDock,
    ),
  );

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      availableWidth = constraints.maxWidth;
      final wide = !DshBreakpoints.collapseSidebar(constraints.maxWidth);
      final canDock = !DshBreakpoints.overlayWorkbench(constraints.maxWidth);
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
            if (!open &&
                dockOpen &&
                DshBreakpoints.overlayWorkbench(availableWidth)) {
              closeDock();
            }
          },
          drawer: wide
              ? null
              : Drawer(width: 280, child: focusRegion(sidebarFocus, sidebar())),
          body: SafeArea(
            child: Row(
              children: [
                if (wide && sideOpen) ...[
                  SizedBox(
                    width: sidebarWidth.clamp(220, 340),
                    child: focusRegion(sidebarFocus, sidebar()),
                  ),
                  _divider(
                    (dx) => setState(
                      () => sidebarWidth = (sidebarWidth + dx).clamp(220, 340),
                    ),
                    onReset: () {
                      setState(() => sidebarWidth = 280);
                      saveLayout();
                    },
                  ),
                ],
                if (wide && !sideOpen)
                  SizedBox(
                    width: 56,
                    child: focusRegion(sidebarFocus, collapsedSidebar()),
                  ),
                Expanded(
                  child: focusRegion(
                    conversationFocus,
                    Column(
                      children: [
                        if (c.error != null)
                          DshErrorView(
                            error: c.error!,
                            onDismiss: c.clearError,
                          ),
                        Expanded(
                          child: Stack(
                            fit: StackFit.expand,
                            children: [
                              Offstage(
                                offstage: mainPanel != null,
                                child: TickerMode(
                                  enabled: mainPanel == null,
                                  child: ExcludeFocus(
                                    excluding: mainPanel != null,
                                    child: Conversation(
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
                                  ),
                                ),
                              ),
                              if (mainPanel == 'schedule')
                                Positioned.fill(
                                  // Clears the floating sidebar button.
                                  top: wide ? 0 : 48,
                                  child: SchedulePage(
                                    key: ValueKey((
                                      c.client,
                                      'schedule-page',
                                      scheduleFocus,
                                    )),
                                    controller: c,
                                    initialTaskId: scheduleFocus,
                                    onClose: closePanel,
                                    onOpenSession: (id) =>
                                        unawaited(openConversation(id)),
                                  ),
                                ),
                              if (mainPanel == 'knowledge')
                                Positioned.fill(
                                  // Clears the floating sidebar button.
                                  top: wide ? 0 : 48,
                                  child: KnowledgePage(
                                    key: ValueKey((c.client, 'knowledge-page')),
                                    controller: c,
                                    onClose: closePanel,
                                  ),
                                ),
                              if (mainPanel == 'plugins')
                                Positioned.fill(
                                  // Clears the floating sidebar button.
                                  top: wide ? 0 : 48,
                                  child: PluginPage(
                                    key: ValueKey((c.client, 'plugins-page')),
                                    controller: c,
                                    onOpenPlugin: openPlugin,
                                    onClose: closePanel,
                                  ),
                                ),
                              if (!wide)
                                Positioned(
                                  top: 10,
                                  left: 12,
                                  child: Builder(
                                    builder: (context) => DshIcon(
                                      DshIcons.panelLeft.data,
                                      label: DshShellZh.expandSidebar,
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
                              if (mainPanel == null)
                                Positioned(
                                  top: 8,
                                  right: 28,
                                  child: Row(
                                    mainAxisSize: MainAxisSize.min,
                                    children: [
                                      if (c.selectedId != null)
                                        headerMoreMenu(),
                                      const SizedBox(width: 8),
                                      DshIcon(
                                        DshIcons.panelRight.data,
                                        label: DshShellZh.showWorkbench,
                                        active: dockOpen,
                                        onPressed: () {
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
                                    message: DshShellZh.resizeConversationHint,
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
                ),
                if (canDock && dockOpen) ...[
                  _divider(
                    (dx) => setState(
                      () =>
                          dockWidth = (dockWidth.clamp(330, maxDockWidth) - dx)
                              .clamp(330, maxDockWidth),
                    ),
                    onReset: () {
                      setState(() => dockWidth = 470);
                      saveLayout();
                    },
                  ),
                  SizedBox(
                    width: dockWidth.clamp(330, maxDockWidth),
                    child: workbenchPanel(),
                  ),
                ],
              ],
            ),
          ),
          endDrawer: !canDock && dockOpen
              ? Drawer(
                  width: constraints.maxWidth * .9,
                  child: workbenchPanel(),
                )
              : null,
        ),
      );
    },
  );

  Future<void> accountSettings({String? provider}) => showDialog<void>(
    context: context,
    animationStyle: AnimationStyle(
      duration: DshMotion.duration(context, DshMotion.dialog),
      curve: DshMotion.curve,
    ),
    barrierColor: Colors.transparent,
    barrierDismissible: false,
    builder: (_) => SettingsShell(
      controller: c,
      initialPage: 'models',
      initialModelTab: 'accounts',
      initialAccountProvider: provider,
    ),
  );

  /// Signs in again from the sidebar without a detour through settings.
  Future<void> accountLogin(String provider) async {
    final api = c.client;
    if (api == null || !c.connected) return;
    await showDialog<void>(
      context: context,
      builder: (_) => AccountLoginDialog(
        api: api,
        provider: provider,
        onComplete: () async {
          if (!identical(api, c.client)) return;
          await c.loadAccounts();
          if (identical(api, c.client)) await c.loadCatalogs();
        },
      ),
    );
  }

  Widget headerMoreMenu() {
    final api = c.client, session = c.selectedId;
    final colors = DshColors(context);
    Widget menuLabel(IconData icon, String title, {Widget? trailing}) => Row(
      children: [
        DshGlyph(icon, size: 16, color: colors.muted),
        const SizedBox(width: 10),
        Expanded(child: Text(title)),
        ?trailing,
      ],
    );
    return PopupMenuButton<String>(
      key: const Key('more-header-menu'),
      tooltip: DshShellZh.moreActions,
      padding: EdgeInsets.zero,
      position: PopupMenuPosition.under,
      color: colors.base,
      elevation: 4,
      shadowColor: Colors.black.withValues(alpha: .1),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(12),
        side: BorderSide(color: colors.border.withValues(alpha: .6)),
      ),
      constraints: const BoxConstraints(minWidth: 220, maxWidth: 300),
      onOpened: () => openedHeaderScope = headerActionScope,
      onSelected: (value) {
        if (!mounted ||
            openedHeaderScope != headerActionScope ||
            !identical(api, c.client) ||
            session != c.selectedId) {
          return;
        }
        switch (value) {
          case 'log':
            unawaited(exportSessionLog());
          case 'feedback':
            unawaited(sessionFeedback());
          case 'team':
            openDock('team');
          case 'schedule':
            openPanel('schedule');
        }
      },
      itemBuilder: (_) => [
        PopupMenuItem(
          key: const Key('session-menu-download'),
          value: 'log',
          enabled: api != null && session != null,
          child: menuLabel(
            DshIcons.download.data,
            DshConversationZh.downloadSessionLog,
          ),
        ),
        PopupMenuItem(
          key: const Key('session-menu-feedback'),
          value: 'feedback',
          enabled: api != null && session != null,
          child: menuLabel(
            DshIcons.messageCircle.data,
            DshConversationZh.feedback,
          ),
        ),
        const PopupMenuDivider(),
        PopupMenuItem(
          key: const Key('session-menu-schedule'),
          value: 'schedule',
          child: Builder(
            builder: (menuContext) => menuLabel(
              DshIcons.alarmClock.data,
              DshShellZh.scheduledTasks,
              trailing: session == null
                  ? null
                  : ScheduleSessionBadge(
                      key: ValueKey((api, 'schedule-badge', session)),
                      controller: c,
                      sessionId: session,
                      onOpen: (task) {
                        if (!mounted ||
                            openedHeaderScope != headerActionScope ||
                            !identical(api, c.client) ||
                            session != c.selectedId) {
                          return;
                        }
                        Navigator.of(menuContext).pop();
                        openPanel('schedule', focus: task);
                      },
                    ),
            ),
          ),
        ),
        if (c.teamSettings['showButton'] != false)
          PopupMenuItem(
            key: const Key('session-menu-team'),
            value: 'team',
            child: menuLabel(DshIcons.users.data, DshShellZh.collaboration),
          ),
      ],
      child: SizedBox(
        width: 36,
        height: 36,
        child: Center(
          child: DshGlyph(
            DshIcons.ellipsis.data,
            size: 18,
            color: colors.muted,
          ),
        ),
      ),
    );
  }

  Future<void> exportSessionLog() async {
    final api = c.client, session = c.selectedId;
    if (api == null || session == null) return;
    await showDialog<void>(
      context: context,
      builder: (_) => SessionLogExportDialog(
        controller: c,
        api: api,
        sessionId: session,
        pickLocation: (filename) async => (await getSaveLocation(
          suggestedName: filename,
          acceptedTypeGroups: [
            const XTypeGroup(label: 'ZIP', extensions: ['zip']),
          ],
        ))?.path,
      ),
    );
  }

  Widget accountEntries({bool compact = false}) => AccountConnectionMenu(
    controller: c,
    compact: compact,
    onManageAccounts: (provider) =>
        unawaited(accountSettings(provider: provider)),
    onModels: () => unawaited(settings('models')),
    onLogin: (provider) => unawaited(accountLogin(provider)),
  );

  Widget settingsButton() => DshIcon(
    DshIcons.settings.data,
    key: const Key('open-settings-direct'),
    label: shortcutHint('settings', DshShellZh.settings),
    size: 36,
    color: DshColors(context).text,
    onPressed: () => settings(),
  );

  Widget sidebarNavigationContent(
    IconData icon,
    String title, {
    Widget? trailing,
  }) => Container(
    constraints: BoxConstraints(
      minHeight: DshTokens.of(context).controlHeight(context),
    ),
    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
    child: Row(
      children: [
        DshGlyph(icon, size: 16, color: DshColors(context).text),
        const SizedBox(width: 12),
        Expanded(child: Text(title, style: DshTypography.body)),
        if (trailing != null) ...[const SizedBox(width: 8), trailing],
      ],
    ),
  );

  Widget sidebarNavigation({
    required Key key,
    required IconData icon,
    required String title,
    required VoidCallback? onPressed,
    bool active = false,
    String? tooltip,
    Widget? trailing,
  }) => DshTooltip(
    message: tooltip ?? title,
    child: Semantics(
      button: true,
      selected: active,
      enabled: onPressed != null,
      child: Material(
        color: active ? DshColors(context).selected : Colors.transparent,
        borderRadius: BorderRadius.circular(8),
        child: InkWell(
          key: key,
          borderRadius: BorderRadius.circular(8),
          hoverColor: active
              ? DshColors(context).selected
              : DshColors(context).hover,
          onTap: onPressed,
          child: sidebarNavigationContent(icon, title, trailing: trailing),
        ),
      ),
    ),
  );

  Widget collapsedSidebar() {
    final colors = DshColors(context);
    return Material(
      color: colors.sidebar,
      child: Column(
        children: [
          const SizedBox(height: 12),
          Tooltip(
            message: shortcutHint('sidebar', DshShellZh.expandSidebar),
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
            DshIcons.newSession.data,
            label: shortcutHint('new', DshShellZh.newSession),
            size: 36,
            color: colors.text,
            onPressed: c.connected ? startConversation : null,
          ),
          const SizedBox(height: 12),
          DshIcon(
            DshIcons.alarmClock.data,
            key: const Key('open-schedule-direct'),
            label: DshShellZh.scheduledTasks,
            size: 36,
            active: mainPanel == 'schedule',
            onPressed: () =>
                mainPanel == 'schedule' ? closePanel() : openPanel('schedule'),
          ),
          const SizedBox(height: 12),
          DshIcon(
            DshIcons.bookOpen.data,
            key: const Key('open-knowledge'),
            label: DshShellZh.knowledge,
            size: 36,
            active: mainPanel == 'knowledge',
            onPressed: () => mainPanel == 'knowledge'
                ? closePanel()
                : openPanel('knowledge'),
          ),
          const SizedBox(height: 12),
          DshIcon(
            DshIcons.grid2x2.data,
            key: const Key('open-plugins'),
            label: DshShellZh.plugins,
            size: 36,
            color: colors.text,
            active: mainPanel == 'plugins',
            onPressed: () =>
                mainPanel == 'plugins' ? closePanel() : openPanel('plugins'),
          ),
          const SizedBox(height: 12),
          DshIcon(
            DshIcons.folderPlus.data,
            label: DshShellZh.addWorkspace,
            size: 36,
            color: colors.text,
            onPressed: c.connected ? addWorkspace : null,
          ),
          const SizedBox(height: 12),
          DshIcon(
            DshIcons.search.data,
            label: shortcutHint('search', DshZh.searchSessions),
            size: 36,
            color: colors.text,
            onPressed: openSearch,
          ),
          const Spacer(),
          accountEntries(compact: true),
          const SizedBox(height: 4),
          settingsButton(),
          const SizedBox(height: 12),
        ],
      ),
    );
  }

  Widget _divider(ValueChanged<double> move, {VoidCallback? onReset}) =>
      MouseRegion(
        cursor: SystemMouseCursors.resizeColumn,
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onDoubleTap: onReset,
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
    final owners = {
      for (final s in visibleSessions) s.id: c.workspaceOf(s)?['workspaceId'],
    };
    for (final workspace in c.workspaces) {
      final id = workspace['workspaceId'] as String;
      final entries = visibleSessions
          .where((s) => owners[s.id] == id)
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
            key: ValueKey('workspace-$id'),
            title: '${workspace['title']}',
            path: '${workspace['path']}',
            expanded: groupExpandedForView(id),
            // Highlight only where 新会话 will start.
            active: c.workspaceId == id,
            onPressed: () {
              final previous = c.workspaceId;
              // Default expansion follows the target; keep the old one as shown.
              if (previous != null && previous != id) {
                groupExpansion.putIfAbsent(previous, () => true);
              }
              c.targetWorkspace(id);
              setGroupExpanded(id, true);
            },
            toggleKey: ValueKey('workspace-toggle-$id'),
            menuKey: ValueKey('workspace-more-$id'),
            onToggle: () => toggleGroupExpansion(id),
            onMenu: (position) => workspaceMenu(workspace, position),
          ),
        ),
      );
      if (groupExpandedForView(id)) {
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
            DshShellZh.session,
            style: TextStyle(
              fontSize: DshTypography.sizeCaption,
              color: colors.muted,
            ),
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
            padding: const EdgeInsets.fromLTRB(20, 14, 12, 14),
            child: Row(
              children: [
                Expanded(
                  child: GestureDetector(
                    onTap: c.connected ? startConversation : null,
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
                  DshIcons.panelLeftClose.data,
                  label: shortcutHint('sidebar', DshShellZh.collapseSidebar),
                  onPressed: toggleSidebar,
                ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10),
            child: sidebarNavigation(
              key: const Key('new-task'),
              icon: DshIcons.newSession.data,
              title: DshShellZh.blankSession,
              tooltip: shortcutHint('new', DshShellZh.newSession),
              trailing: shortcutBadge('new'),
              onPressed: c.connected ? startConversation : null,
            ),
          ),
          const SizedBox(height: 8),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10),
            child: SidebarEntryRow(
              entries: [
                SidebarEntry(
                  key: const Key('open-schedule-direct'),
                  icon: DshIcons.alarmClock.data,
                  label: DshShellZh.scheduledShort,
                  semanticLabel: DshShellZh.scheduledTasks,
                  active: mainPanel == 'schedule',
                  onPressed: () => mainPanel == 'schedule'
                      ? closePanel()
                      : openPanel('schedule'),
                ),
                SidebarEntry(
                  key: const Key('open-knowledge'),
                  icon: DshIcons.bookOpen.data,
                  label: DshShellZh.knowledgeShort,
                  semanticLabel: DshShellZh.knowledge,
                  active: mainPanel == 'knowledge',
                  onPressed: () => mainPanel == 'knowledge'
                      ? closePanel()
                      : openPanel('knowledge'),
                ),
                SidebarEntry(
                  key: const Key('open-plugins'),
                  icon: DshIcons.grid2x2.data,
                  label: DshShellZh.plugins,
                  active: mainPanel == 'plugins',
                  onPressed: () => mainPanel == 'plugins'
                      ? closePanel()
                      : openPanel('plugins'),
                ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(22, 18, 10, 4),
            child: Row(
              children: [
                Text(
                  DshShellZh.workspace,
                  style: TextStyle(
                    fontSize: DshTypography.sizeAuxiliary,
                    color: colors.muted,
                  ),
                ),
                const Spacer(),
                DshIcon(
                  DshIcons.search.data,
                  label: DshShellZh.filterSidebar,
                  active: showSearch,
                  onPressed: toggleSidebarSearch,
                ),
                SizedBox(
                  width: 36,
                  height: 36,
                  child: PopupMenuButton<String>(
                    tooltip: DshShellZh.viewOptions,
                    padding: EdgeInsets.zero,
                    constraints: const BoxConstraints(
                      minWidth: 220,
                      maxWidth: 280,
                    ),
                    icon: DshGlyph(
                      DshIcons.slidersHorizontal.data,
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
                        'hidden': DshShellZh.hideArchived,
                        'all': DshShellZh.allSessions,
                        'archived': DshShellZh.archivedOnly,
                      }.entries)
                        CheckedPopupMenuItem(
                          value: entry.key,
                          checked: archiveFilter == entry.key,
                          child: Text(entry.value),
                        ),
                      const PopupMenuDivider(),
                      const PopupMenuItem(
                        value: 'refresh',
                        child: Text(DshShellZh.refreshSessions),
                      ),
                      const PopupMenuItem(
                        value: 'archive',
                        child: Text(DshShellZh.archiveManagement),
                      ),
                    ],
                  ),
                ),
                DshIcon(
                  DshIcons.folderPlus.data,
                  label: DshShellZh.addWorkspace,
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
                hint: DshZh.searchSessions,
                autofocus: true,
                onChanged: (_) => setState(filteredGroupExpansion.clear),
              ),
            ),
          Expanded(
            child: c.sessions.isEmpty && (c.connecting || c.loadingSessions)
                ? DshListSkeleton(
                    label: DshConversationZh.loadingList(name: DshZh.session),
                  )
                : rows.isEmpty
                ? Align(
                    alignment: Alignment.topLeft,
                    child: Padding(
                      padding: const EdgeInsets.all(24),
                      child: Text(
                        c.connecting
                            ? DshShellZh.connecting
                            : DshShellZh.noSessions,
                        style: TextStyle(
                          fontSize: DshTypography.sizeBody,
                          color: colors.muted,
                        ),
                      ),
                    ),
                  )
                : ListView.builder(
                    padding: const EdgeInsets.symmetric(horizontal: 10),
                    itemCount: rows.length,
                    itemBuilder: (_, i) => rows[i],
                  ),
          ),
          DecoratedBox(
            decoration: BoxDecoration(
              border: Border(top: BorderSide(color: colors.border)),
            ),
            child: Padding(
              padding: const EdgeInsets.fromLTRB(10, 8, 10, 10),
              child: Row(
                children: [
                  Expanded(child: accountEntries()),
                  const SizedBox(width: 4),
                  settingsButton(),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget sessionRow(SessionSummary session) => HoverRowActions(
    builder: (context, revealed) => Padding(
      padding: const EdgeInsets.only(bottom: 2),
      child: Material(
        color: mainPanel == null && c.selectedId == session.id
            ? DshColors(context).selected
            : Colors.transparent,
        borderRadius: BorderRadius.circular(7),
        child: InkWell(
          key: ValueKey('session-${session.id}'),
          borderRadius: BorderRadius.circular(7),
          hoverColor: mainPanel == null && c.selectedId == session.id
              ? DshColors(context).selected
              : DshColors(context).hover,
          onTap: () => unawaited(openConversation(session.id)),
          onSecondaryTapDown: (d) => sessionMenu(session, d.globalPosition),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(28, 5, 7, 5),
            child: Row(
              children: [
                Expanded(
                  child: Tooltip(
                    message: session.displayTitle,
                    child: Text(
                      session.displayTitle,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: DshTypography.sizeBody),
                    ),
                  ),
                ),
                if (syncingSessions.contains(session.id))
                  const Tooltip(
                    message: DshZh.syncing,
                    child: SizedBox(
                      width: 12,
                      height: 12,
                      child: CircularProgressIndicator(strokeWidth: 1.5),
                    ),
                  ),
                if (syncFailures.containsKey(session.id))
                  DshIcon(
                    DshIcons.rotateCcw.data,
                    label: DshZh.syncFailure(session.displayTitle),
                    onPressed: () => showSyncFailure(session),
                  ),
                if (session.running)
                  const SizedBox(
                    width: 10,
                    height: 10,
                    child: CircularProgressIndicator(strokeWidth: 1.4),
                  ),
                if (c.archivedSessionIds.contains(session.id))
                  Tooltip(
                    message: DshShellZh.archived,
                    child: DshGlyph(
                      DshIcons.archive.data,
                      size: 13,
                      color: DshColors(context).muted,
                    ),
                  ),
                if (c.pending.values.any((f) => f.sessionId == session.id))
                  DshGlyph(
                    DshIcons.circleHelp.data,
                    size: 13,
                    color: DshTokens.of(context).warning.foreground,
                  ),
                if (revealed)
                  RowMoreButton(
                    key: ValueKey('session-more-${session.id}'),
                    label: DshShellZh.sessionActions,
                    onMenu: (position) => sessionMenu(session, position),
                  )
                else if (!session.blank && session.updatedAt > 0)
                  Text(
                    relativeSessionAge(session.updatedAt),
                    style: TextStyle(
                      fontSize: DshTypography.sizeCaption,
                      color: DshColors(context).muted,
                    ),
                  ),
              ],
            ),
          ),
        ),
      ),
    ),
  );
  Future<void> sessionMenu(SessionSummary session, Offset pos) async {
    final ownerScope = connectionScope;
    final action = await showRowMenu<String>(context, pos, [
      PopupMenuItem(
        value: 'rename',
        child: rowMenuLabel(context, DshIcons.pencil.data, DshShellZh.rename),
      ),
      PopupMenuItem(
        value: 'fork',
        child: rowMenuLabel(context, DshIcons.gitBranch.data, DshShellZh.fork),
      ),
      PopupMenuItem(
        value: 'copy-id',
        child: rowMenuLabel(
          context,
          DshIcons.copy.data,
          DshShellZh.copySessionId,
        ),
      ),
      const PopupMenuDivider(),
      if (c.archivedSessionIds.contains(session.id))
        PopupMenuItem(
          value: 'restore',
          child: rowMenuLabel(
            context,
            DshIcons.rotateCcw.data,
            DshShellZh.restoreArchive,
          ),
        )
      else
        PopupMenuItem(
          value: 'archive',
          child: rowMenuLabel(
            context,
            DshIcons.archive.data,
            DshShellZh.archive,
          ),
        ),
    ]);
    if (!mounted || connectionScope != ownerScope) return;
    if (action == 'copy-id') {
      await Clipboard.setData(ClipboardData(text: session.id));
      return;
    }
    if (action == 'rename') {
      final owner = c.client;
      if (owner == null) {
        c.error = DshShellZh.connectFirst;
        c.emit();
        return;
      }
      var base = c.titleEditBase(session.id);
      await editTextDialog(
        context,
        DshShellZh.renameSession,
        session.title,
        onSubmit: (title) async {
          if (!mounted || connectionScope != ownerScope) {
            throw StateError(DshShellZh.titleConnectionChanged);
          }
          if (title.trim().isEmpty) {
            throw const FormatException(DshShellZh.sessionNameRequired);
          }
          if (base == null) {
            throw DshException('title-unloaded', DshShellZh.titleNotLoaded);
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
          if (!mounted || connectionScope != ownerScope) {
            throw StateError(DshShellZh.titleConnectionChanged);
          }
          await c.refreshSessions();
          base = c.titleEditBase(session.id);
          if (base == null) throw StateError(DshShellZh.titleUnavailable);
          return DshShellZh.latestTitle(
            title: base!['value'] ?? DshShellZh.blankSession,
          );
        },
      );
    }
    if (action == 'archive') {
      await syncSession(session.id, () async {
        try {
          await c.archive(session.id);
        } on DshException catch (error) {
          if (error.code != 'agent-busy' ||
              error.details['reason'] != 'active-schedules') {
            rethrow;
          }
          if (!mounted || connectionScope != ownerScope) return;
          final confirmed = await confirmAction(
            context,
            DshShellZh.archiveWithSchedulesTitle,
            DshShellZh.archiveWithSchedulesHint,
            action: DshShellZh.stopAndArchive,
          );
          if (!confirmed || !mounted || connectionScope != ownerScope) return;
          await c.archive(session.id, stopSchedules: true);
        }
      });
    }
    if (action == 'restore') {
      await syncSession(session.id, () => c.archive(session.id, restore: true));
    }
    if (action == 'fork') {
      final api = c.client;
      final selection = c.selectionRevision;
      if (api == null) return;
      await c.run(() async {
        final result = await api.call('session.fork', {
          'sessionId': session.id,
        }, true);
        if (!mounted || connectionScope != ownerScope) return;
        await c.refreshSessions();
        if (!mounted ||
            connectionScope != ownerScope ||
            c.selectionRevision != selection) {
          return;
        }
        closePanel();
        await c.select(result['sessionId'] as String);
      });
    }
  }

  Future<void> workspaceMenu(Json workspace, Offset pos) async {
    final ownerScope = connectionScope;
    final api = c.client;
    if (api == null) return;
    final action = await showRowMenu<String>(context, pos, [
      PopupMenuItem(
        value: 'rename',
        child: rowMenuLabel(
          context,
          DshIcons.pencil.data,
          DshShellZh.renameWorkspace,
        ),
      ),
      PopupMenuItem(
        value: 'open',
        child: rowMenuLabel(
          context,
          DshIcons.folderOpen.data,
          DshShellZh.openInFileManager,
        ),
      ),
      const PopupMenuDivider(),
      PopupMenuItem(
        value: 'delete',
        child: rowMenuLabel(
          context,
          DshIcons.trash2.data,
          DshShellZh.deleteWorkspace,
          destructive: true,
        ),
      ),
    ]);
    if (!mounted || connectionScope != ownerScope) return;
    if (action == 'delete') {
      final confirmed = await confirmAction(
        context,
        DshShellZh.deleteWorkspace,
        DshShellZh.deleteWorkspaceHint(title: workspace['title']),
        action: DshShellZh.deleteWorkspace,
      );
      if (!confirmed || !mounted || connectionScope != ownerScope) return;
      await c.run(() async {
        await api.call('workspace.delete', {
          'workspaceId': workspace['workspaceId'],
        }, true);
        if (!mounted || connectionScope != ownerScope) return;
        if (c.workspaceId == workspace['workspaceId']) c.targetWorkspace(null);
        groupExpansion.remove('${workspace['workspaceId']}');
        await c.refreshSessions();
        if (mounted && connectionScope == ownerScope) saveLayout();
      });
      return;
    }
    if (action == 'rename') {
      final title = await editTextDialog(
        context,
        DshShellZh.renameWorkspace,
        '${workspace['title']}',
      );
      if (title != null && mounted && connectionScope == ownerScope) {
        await c.run(() async {
          await api.call('workspace.rename', {
            'workspaceId': workspace['workspaceId'],
            'title': title,
          }, true);
          if (mounted && connectionScope == ownerScope) {
            await c.refreshSessions();
          }
        });
      }
    }
    if (action == 'open') {
      await c.run(() async {
        await api.call('host.openPath', {'path': workspace['path']}, true);
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
          if (c.client != api) {
            throw StateError(DshShellZh.workspaceConnectionChanged);
          }
          await api.request(
            '/__dsh-artifacts/workspace-settings',
            body: {'path': path, 'location': scratch},
            mutation: true,
          );
          if (c.client != api) {
            throw StateError(DshShellZh.workspaceConnectionChanged);
          }
          await c.addWorkspace(path);
        },
      ),
    );
  }

  Future<void> chooseWorkspace() async {
    final ownerScope = connectionScope;
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
                  DshGlyph(DshIcons.folder.data, size: 16),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      '${w['title']}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: DshTypography.sizeBody),
                    ),
                  ),
                  if (w['workspaceId'] == c.workspaceId)
                    DshGlyph(DshIcons.check.data, size: 14),
                ],
              ),
            ),
          ),
        const PopupMenuDivider(),
        PopupMenuItem(
          value: '__add',
          height: 38,
          child: Row(
            children: [
              DshGlyph(DshIcons.plus.data, size: 16),
              SizedBox(width: 10),
              Text(DshShellZh.addWorkingDirectory),
            ],
          ),
        ),
        const PopupMenuItem(
          value: '__git',
          height: 38,
          child: Text(DshShellZh.cloneGitDirectory),
        ),
        const PopupMenuItem(
          value: '__cloud',
          height: 38,
          child: Text(DshShellZh.cloudRepository),
        ),
        const PopupMenuItem(
          value: '__ssh',
          height: 38,
          child: Text(DshShellZh.sshDirectory),
        ),
      ],
    );
    if (!mounted || connectionScope != ownerScope) return;
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
        c.targetWorkspace(result['workspaceId'] as String);
        setGroupExpanded(c.workspaceId!, true);
        await c.run(c.startConversation);
      }
    } else if (id != null) {
      c.targetWorkspace(id);
      setGroupExpanded(id, true);
      await c.run(c.startConversation);
    }
  }

  Future<void> settings([String page = 'general']) => showDialog<void>(
    context: context,
    animationStyle: AnimationStyle(
      duration: DshMotion.duration(context, DshMotion.dialog),
      curve: DshMotion.curve,
    ),
    barrierColor: Colors.transparent,
    barrierDismissible: false,
    builder: (_) => SettingsShell(
      controller: c,
      initialPage: page,
      onOpenPlugin: (row) {
        Navigator.of(context).pop();
        openPlugin(row);
      },
    ),
  );

  /// Host entry ids are assigned per installation ("artifacts", "clock",
  /// …); the module name is what identifies the plugin's own view.
  void openPlugin(Json row) {
    final module = DshPluginSettingsZh.canonical(
      '${row['moduleName'] ?? row['id'] ?? row['entryId'] ?? ''}',
    );
    if (module == 'dsh-schedule') {
      openPanel('schedule');
      return;
    }
    closePanel();
    switch (module) {
      case 'dsh-better-sidebar' || 'dsh-sidebar-workbench-suite':
        openDock('start');
      case 'dsh-context-jump':
        conversationViewRequest.value = 'user-message-rail';
      case 'dsh-artifacts':
        conversationViewRequest.value = 'artifacts';
      default:
        conversationViewRequest.value = 'conversation';
        c.composerFocus.value++;
    }
  }

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
    'task-result': DshShellZh.feedbackTaskResult,
    'instruction-following': DshShellZh.feedbackInstructions,
    'product-interaction': DshShellZh.feedbackInteraction,
    'service-stability': DshShellZh.feedbackStability,
    'resource-cost': DshShellZh.feedbackCost,
    'security-privacy-permission': DshShellZh.feedbackSecurity,
    'other': DshShellZh.feedbackOther,
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
        throw DshException('protocol', DshShellZh.feedbackUnconfirmed);
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
    title: const Text(
      DshShellZh.sessionFeedback,
      style: TextStyle(fontSize: DshTypography.sizeSectionTitle),
    ),
    content: SizedBox(
      width: 500,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            DshShellZh.feedbackHint,
            style: TextStyle(fontSize: DshTypography.sizeAuxiliary),
          ),
          const SizedBox(height: 14),
          DropdownButtonFormField<String>(
            key: ValueKey(category),
            initialValue: category.isEmpty ? null : category,
            decoration: const InputDecoration(
              labelText: DshShellZh.feedbackCategory,
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
              labelText: DshShellZh.feedbackNote,
              border: OutlineInputBorder(),
            ),
          ),
          if (error != null)
            Padding(
              padding: const EdgeInsets.only(top: 10),
              child: DshErrorView(error: error!),
            ),
        ],
      ),
    ),
    actions: [
      DshButton(
        onPressed: busy ? null : () => Navigator.pop(context),
        child: const Text(DshZh.cancel),
      ),
      DshButton(
        primary: true,
        onPressed: busy ? null : save,
        child: Text(busy ? DshZh.saving : DshShellZh.saveFeedback),
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
      title: const Text(
        DshShellZh.addWorkspace,
        style: TextStyle(fontSize: DshTypography.sizeSectionTitle),
      ),
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
                    style: const TextStyle(fontSize: DshTypography.sizeBody),
                  ),
                )
              else
                DshField(
                  key: const Key('working-directory'),
                  controller: input,
                  hint: DshShellZh.workingDirectory,
                  enabled: !busy,
                ),
              const SizedBox(height: 16),
              DshButton(
                icon: advanced
                    ? DshIcons.chevronDown.data
                    : DshIcons.chevronRight.data,
                onPressed: busy
                    ? null
                    : () => setState(() => advanced = !advanced),
                child: const Text(DshShellZh.advancedSettings),
              ),
              if (advanced) ...[
                const SizedBox(height: 12),
                const Text(
                  DshShellZh.trashLocation,
                  style: TextStyle(fontSize: DshTypography.sizeBody),
                ),
                const SizedBox(height: 8),
                Row(
                  children: [
                    Expanded(
                      child: DshField(
                        controller: scratch,
                        hint: DshShellZh.globalLocation,
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
                      child: const Text(DshShellZh.chooseDirectory),
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                const Text(
                  DshShellZh.trashLocationHint,
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    height: 1.5,
                  ),
                ),
              ],
              if (error != null)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: DshErrorView(error: error!),
                ),
            ],
          ),
        ),
      ),
      actions: [
        DshButton(
          onPressed: busy ? null : () => Navigator.pop(context),
          child: const Text(DshZh.cancel),
        ),
        DshButton(
          key: const Key('create-task'),
          primary: true,
          onPressed: busy ? null : create,
          child: Text(busy ? DshShellZh.adding : DshShellZh.add),
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
    title: const Text(
      DshShellZh.localService,
      style: TextStyle(fontSize: DshTypography.sizeSectionTitle),
    ),
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
              child: const Text(DshShellZh.installedConfiguration),
            ),
          ),
          if (widget.controller.host != null)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: SelectableText(
                DshShellZh.configDirectory(
                  path: displayPath(widget.controller.host!.home),
                ),
                style: const TextStyle(fontSize: DshTypography.sizeCaption),
              ),
            ),
          const SizedBox(height: 16),
          Row(
            children: [
              Expanded(
                child: DshField(
                  controller: exe,
                  hint: DshShellZh.hostExecutable,
                ),
              ),
              DshIcon(
                DshIcons.folderOpen.data,
                label: DshShellZh.chooseExecutable,
                onPressed: () async {
                  final file = await openFile(
                    acceptedTypeGroups: [
                      const XTypeGroup(
                        label: DshShellZh.executable,
                        extensions: ['exe'],
                      ),
                    ],
                  );
                  if (mounted && file != null) exe.text = file.path;
                },
              ),
            ],
          ),
          if (error != null) DshErrorView(error: error!),
          if (busy) const LinearProgressIndicator(),
        ],
      ),
    ),
    actions: [
      DshButton(
        onPressed: busy ? null : () => Navigator.pop(context),
        child: const Text(DshZh.cancel),
      ),
      DshButton(
        onPressed: busy ? null : () => submit(true),
        outline: true,
        child: const Text(DshShellZh.startAndConnect),
      ),
      DshButton(
        onPressed: busy ? null : () => submit(false),
        primary: true,
        child: const Text(DshShellZh.connect),
      ),
    ],
  );
}
