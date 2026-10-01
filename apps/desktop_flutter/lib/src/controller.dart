import '../l10n/runtime_zh.dart';

import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:dsh_client/dsh_client.dart';

import 'preferences.dart';
import 'reading_position.dart';

String permissionName(String value) =>
    const {
      'workspace-write': DshRuntimeZh.workspaceWrite,
      'danger-full-access': DshRuntimeZh.fullAccess,
      'full-access': DshRuntimeZh.fullAccess,
      'read-only': DshRuntimeZh.readOnly,
    }[value] ??
    value;

/// A Host left running by an earlier installation keeps the port, and the
/// desktop client would silently use its older API surface.
String hostVersionMismatch(String address, String running, String expected) =>
    DshRuntimeZh.hostVersionMismatch(
      address: address,
      running: running,
      expected: expected,
    );

/// A Computer Use control session the model drives in the selected
/// conversation; the workbench attaches to it instead of starting another.
class ComputerUseBinding {
  const ComputerUseBinding({
    required this.browserSessionId,
    required this.target,
  });
  final String browserSessionId, target;
  @override
  bool operator ==(Object other) =>
      other is ComputerUseBinding &&
      other.browserSessionId == browserSessionId &&
      other.target == target;
  @override
  int get hashCode => Object.hash(browserSessionId, target);
}

class DesktopController extends ChangeNotifier {
  DesktopController(
    this.preferences, {
    DshClient Function(String)? clientFactory,
  }) : _clientFactory = clientFactory ?? ((address) => DshClient(address)) {
    messageChanges.addListener(() {
      _lastConversationState = _conversationState();
    });
  }
  final DesktopPreferences preferences;
  late final themeChanges = ValueNotifier<bool>(preferences.dark);
  final conversationChanges = ValueNotifier<int>(0);
  final connectionChanges = ValueNotifier<int>(0);
  final interactionChanges = ValueNotifier<int>(0);
  List<Object?>? _lastConversationState, _lastInteractionState;
  Object? _lastConnectionState;
  int unreadHistoryEvents = 0;
  final readingPositions = <String, ConversationReadingPosition>{};

  void rememberReadingPosition(
    String id, {
    required int? seq,
    required String? itemId,
    required double viewportOffset,
    required bool follow,
  }) {
    readingPositions.remove(id);
    if (!follow && seq != null && itemId != null && viewportOffset.isFinite) {
      readingPositions[id] = ConversationReadingPosition(
        seq: seq,
        itemId: itemId,
        viewportOffset: viewportOffset,
      );
    }
    while (readingPositions.length > 64) {
      readingPositions.remove(readingPositions.keys.first);
    }
  }

  double get bodyFontSize {
    final value = preferences.layout['bodyFontSize'];
    return value is num && value.isFinite ? value.toDouble().clamp(14, 18) : 15;
  }

  Future<void> setBodyFontSize(double value) async {
    await preferences.saveLayoutValue('bodyFontSize', value.clamp(14, 18));
    emit();
  }

  final DshClient Function(String) _clientFactory;
  DshClient? _client;
  DshClient? get client => _client;
  HostInfo? host;
  List<SessionSummary> sessions = [];
  Set<String> archivedSessionIds = {};
  List<Json> workspaces = [],
      archivedSessions = [],
      presets = [],
      commands = [],
      queued = [],
      jobs = [];
  String? workspaceId;
  String preset = 'blank';
  String get presetName =>
      presets.where((p) => p['id'] == preset).firstOrNull?['name'] as String? ??
      const {
        'standard': DshRuntimeZh.standardPreset,
        'blank': DshRuntimeZh.blankPreset,
        'code': DshRuntimeZh.codePreset,
      }[preset] ??
      preset;
  final projectionWindow = ProjectionWindow();
  Json get projections => projectionWindow.values;
  Map<String, String> get permissionChoices => {
    for (final row in objects(object(projections['permissions'])['options']))
      if (row['value'] is String)
        row['value']
            as String: permissionName(row['value'] as String) == row['value']
            ? '${row['name'] ?? row['value']}'
            : permissionName(row['value'] as String),
  };
  final projectionChanges = ValueNotifier<int>(0);
  Timer? _projectionPaint;
  Json conversationSettings = {};
  Json menuSettings = {};
  Json teamSettings = {};

  /// `context-compaction` settings: `thresholds` keyed by `provider/model`.
  Json contextCompaction = {};
  Set<String> disabledPlugins = {};
  int scheduleRevision = 0;
  bool? scheduleEnabled;
  bool pluginEnabled(String name) => !disabledPlugins.contains(name);
  final messageChanges = ValueNotifier<int>(0);
  final composerFocus = ValueNotifier<int>(0);

  /// Latest Computer Use session the model drove in the selected
  /// conversation; [computerUseRequests] asks the shell to show it.
  ComputerUseBinding? computerUse;
  final computerUseRequests = ValueNotifier<int>(0);
  final _computerUseShown = <String>{};
  int _computerUseSeq = -1;
  void _forgetComputerUse() {
    computerUse = null;
    _computerUseShown.clear();
    _computerUseSeq = -1;
  }

  List<Json> subscriptionAccounts = [];
  RequestScope? _historyScope;
  RequestScope? _commandScope;
  int _commandActivityRequest = 0;
  final Set<String> _liveCommands = {};
  bool commandRunning = false;
  RequestScope? _planScope;
  Object? _planChange;
  int? _planSelection;
  PlanModeProjection? get planMode =>
      PlanModeProjection.from(projections['plan']);
  bool get changingPlanMode =>
      _planChange != null && _planSelection == _selection;
  int _historyRequest = 0;
  int? historyTargetSeq;
  bool _holdingLiveHistory = false;
  bool _heldActivity = false;
  bool get holdingLiveHistory => _holdingLiveHistory;
  bool get holdingHistoryActivity => _holdingLiveHistory && _heldActivity;
  bool get readingHistory => historyTargetSeq != null || _holdingLiveHistory;
  Map<String, Object> get resourceDiagnostics => {
    'historyBytes': window.retainedBytes,
    'historyEvents': window.eventCount,
    'liveBufferBytes': _bufferBytes,
    'liveBufferEvents': _buffer.length,
    'projectionBytes': projectionWindow.retainedBytes,
    'controllerSubscriptions': _subscriptions.length,
    'pendingInteractions': pending.length,
    'connected': connected,
    'readingHistory': readingHistory,
    'loadingHistory': loading,
    ...DshClient.resourceCounts,
  };
  Json? get currentWorkspace =>
      workspaces.where((w) => w['workspaceId'] == workspaceId).firstOrNull;

  /// The Workspace a session belongs to: its registered membership, else the
  /// folder it runs in. The sidebar groups sessions by the same rule.
  Json? workspaceOf(SessionSummary session) {
    final registered = workspaces
        .where((w) => (w['sessionIds'] as List? ?? []).contains(session.id))
        .firstOrNull;
    if (registered != null) return registered;
    final folder = _pathKey(session.cwd);
    return workspaces.where((w) => _pathKey(w['path']) == folder).firstOrNull;
  }

  final _pathKeys = <String, String>{};
  String _pathKey(Object? path) {
    final raw = '${path ?? ''}';
    if (_pathKeys.length > 4096) _pathKeys.clear();
    return _pathKeys[raw] ??= workspacePathKey(raw);
  }

  int _workspaceTargetRevision = 0;

  /// New sessions start in this Workspace until another one is chosen.
  void targetWorkspace(String? id) {
    // Even reselecting the provisional default is an explicit user choice.
    _workspaceTargetRevision++;
    final previousDraftScope = draftScopeKey;
    _explicitDraftWorkspace = true;
    _provisionalDraftScope = null;
    if (workspaceId == id && previousDraftScope == draftScopeKey) return;
    workspaceId = id;
    emit();
  }

  String? get _recentWorkspaceId {
    final recent =
        sessions.where((s) => !archivedSessionIds.contains(s.id)).toList()
          ..sort((a, b) => b.updatedAt.compareTo(a.updatedAt));
    for (final session in recent) {
      final owner = workspaceOf(session)?['workspaceId'];
      if (owner is String) return owner;
    }
    return null;
  }

  String? selectedId;
  String? _draftHostAddress;
  String? _provisionalDraftScope;
  bool _explicitDraftWorkspace = false;
  static const unnamedDraftPrefix = '__dsh_unnamed_draft_v1__:';

  /// A reserved preferences entry; it is never sent as a Host session id.
  String get unnamedDraftKey => _unnamedDraftKey(workspaceId);

  String _unnamedDraftKey(String? workspace) {
    final address = _draftHostAddress ?? preferences.address;
    final uri = Uri.tryParse(address);
    final hostKey =
        uri != null &&
            (uri.scheme == 'http' || uri.scheme == 'https') &&
            uri.host.isNotEmpty
        ? uri.origin
        : address;
    return '$unnamedDraftPrefix${jsonEncode([hostKey, workspace])}';
  }

  String get draftScopeKey =>
      selectedId ?? _provisionalDraftScope ?? unnamedDraftKey;

  void _resumeProvisionalDraft() {
    final provisional = _unnamedDraftKey(null);
    _provisionalDraftScope =
        !_explicitDraftWorkspace &&
            (preferences.drafts[provisional]?.isNotEmpty ?? false)
        ? provisional
        : null;
  }

  ModelCatalog? catalog;
  Object? _modelChange;
  int _modelRevision = 0;
  final _modelDefaultSaves = <DshClient, Future<void>>{};
  int _modelChangeSelection = -1;
  bool get changingModel =>
      _modelChange != null && _modelChangeSelection == _selection;
  ConversationWindow window = ConversationWindow();
  List<TranscriptItem> transcript = [];
  final Map<String, HostFrame> pending = {};
  final Set<String> answering = {};
  bool connecting = false, loading = false, sending = false;
  bool _muxReady = false, _hostReady = false, _disposed = false;
  bool get connected => host != null && _muxReady && _hostReady;
  String? error;
  int _epoch = 0, _selection = 0;
  int get selectionRevision => _selection;
  int get workspaceTargetRevision => _workspaceTargetRevision;
  int? _draftAdoptionRevision;
  bool get selectionAdoptsDraft => _draftAdoptionRevision == _selection;
  final List<StreamSubscription<dynamic>> _subscriptions = [];
  final List<HistoryEvent> _buffer = [];
  int _bufferBytes = 0;
  bool _overflow = false;
  Timer? _paint, _refresh, _draftSave, _listRefresh;
  Future<void>? _listRequest;
  bool get loadingSessions => _listRequest != null;
  Future<String?>? _startingConversation;
  SessionSummary? get selected =>
      sessions.where((e) => e.id == selectedId).firstOrNull;
  List<HostFrame> get interactions =>
      pending.values.where((f) => f.sessionId == selectedId).toList();
  String get draft => preferences.drafts[draftScopeKey] ?? '';
  bool get running => selected?.running ?? false;
  bool get interruptible => running || commandRunning || sending;

  void _clearCommandActivity() {
    _commandScope?.cancel();
    _commandScope = null;
    _commandActivityRequest++;
    _liveCommands.clear();
    commandRunning = false;
  }

  Future<void> refreshCommandActivity() async {
    final api = _client,
        id = selectedId,
        epoch = _epoch,
        selection = _selection;
    if (api == null || id == null || _disposed) return;
    _commandScope?.cancel();
    final scope = _commandScope = RequestScope();
    final request = ++_commandActivityRequest;
    try {
      final result = await api.rpc(
        'commands.activity',
        payload: {'sessionId': id},
        scope: scope,
      );
      if (_disposed ||
          scope.cancelled ||
          request != _commandActivityRequest ||
          epoch != _epoch ||
          selection != _selection ||
          id != selectedId) {
        return;
      }
      if (result['active'] is bool) {
        commandRunning = result['active'] == true;
        if (!commandRunning) _liveCommands.clear();
        emit();
      }
    } catch (_) {
      // Preserve observed live activity through reconnects or older Hosts.
    } finally {
      if (identical(_commandScope, scope)) _commandScope = null;
      scope.cancel();
    }
  }

  bool get compacting {
    final active = <String>{};
    for (final event in window.events) {
      final id = event.data['compactionId'];
      if (id is! String) continue;
      if (event.type == 'compaction/start') active.add(id);
      if (event.type == 'compaction/end') active.remove(id);
    }
    return active.isNotEmpty && !readingHistory;
  }

  bool get blankConversation =>
      selectedId != null &&
      selected?.blank == true &&
      transcript.isEmpty &&
      !running;
  final _sessionRevisions = <String, int>{}, _titleSequences = <String, int>{};
  void _sessionChanged(String id) =>
      _sessionRevisions[id] = (_sessionRevisions[id] ?? 0) + 1;
  void _title(String id, Object? value, int seq) {
    final row = sessions.where((s) => s.id == id).firstOrNull;
    if (row == null ||
        value is! String ||
        value.trim().isEmpty ||
        seq < (_titleSequences[id] ?? -1)) {
      return;
    }
    _titleSequences[id] = seq;
    row.title = value;
    row.titleEditBase = {'value': value, 'throughSeq': seq};
    row.blank = false;
    _sessionChanged(id);
  }

  bool get canEditTodos => host?.supportsIdleTodoEdits == true;

  void emit() {
    if (_disposed) return;
    themeChanges.value = preferences.dark;
    final connection = (connected, connecting, _client, host, error);
    if (_lastConnectionState != connection) {
      _lastConnectionState = connection;
      connectionChanges.value++;
    }
    final interactionState = <Object?>[
      selectedId,
      draftScopeKey,
      connected,
      ...interactions,
      ...answering,
    ];
    if (!listEquals(_lastInteractionState, interactionState)) {
      _lastInteractionState = interactionState;
      interactionChanges.value++;
    }
    final conversationState = _conversationState();
    if (!listEquals(_lastConversationState, conversationState)) {
      _lastConversationState = conversationState;
      conversationChanges.value++;
    }
    notifyListeners();
  }

  List<Object?> _conversationState() {
    final row = selected;
    final selectedWorkspace = row == null ? null : workspaceOf(row);
    return <Object?>[
      _client,
      host,
      connected,
      connecting,
      error,
      selectedId,
      _selection,
      row?.title,
      row?.cwd,
      row?.blank,
      row?.running,
      row?.agentPreset,
      preset,
      workspaceId,
      currentWorkspace?['title'],
      currentWorkspace?['path'],
      selectedWorkspace?['workspaceId'],
      selectedWorkspace?['title'],
      selectedWorkspace?['path'],
      catalog,
      loading,
      sending,
      commandRunning,
      changingPlanMode,
      changingModel,
      transcript,
      window,
      window.hasBefore,
      window.hasAfter,
      historyTargetSeq,
      _holdingLiveHistory,
      unreadHistoryEvents,
      bodyFontSize,
      jsonEncode(conversationSettings),
      jsonEncode(menuSettings),
      jsonEncode(teamSettings),
      jsonEncode(contextCompaction),
      jsonEncode(presets),
      jsonEncode(commands),
      jsonEncode(queued),
      jsonEncode(jobs),
      ...disabledPlugins,
      ...interactions,
      ...answering,
    ];
  }

  void clearError() {
    error = null;
    emit();
  }

  Future<void> run(Future<void> Function() action) async {
    try {
      await action();
    } catch (e) {
      if (e is DshException && e.code == 'cancelled') return;
      if (!_disposed) error = e.toString();
    }
    emit();
  }

  Future<void> initialize() async {
    final included = HostLauncher.bundled();
    if (included != null) {
      preferences.executable = included;
    } else if (preferences.executable.isEmpty) {
      preferences.executable = HostLauncher.discover();
    }
    await run(() async {
      if (included == null) {
        await connect(preferences.address);
        return;
      }
      // A short probe: a refused loopback connect costs about two seconds on
      // Windows, which the bundled Host would otherwise pay before starting.
      final running = await HostLauncher.live(preferences.address);
      if (running == null) {
        await HostLauncher.start(included, preferences.address);
      }
      await connect(preferences.address);
      final bundled = HostLauncher.packagedVersion(included);
      if (running != null && bundled != null && running.version != bundled) {
        error = hostVersionMismatch(
          preferences.address,
          running.version,
          bundled,
        );
      }
    });
  }

  void setDraft(String text) {
    final id = draftScopeKey;
    if (selectedId == null && workspaceId == null) {
      // A late default Workspace is a creation target, not permission to
      // replace text already entered before the inventory became available.
      _provisionalDraftScope = id;
    }
    if (text.isEmpty) {
      preferences.drafts.remove(id);
    } else {
      preferences.drafts[id] = text;
    }
    _draftSave?.cancel();
    _draftSave = Timer(const Duration(milliseconds: 500), () {
      unawaited(run(preferences.save));
    });
  }

  Future<void> connect(String address) async {
    if (_disposed) return;
    readingPositions.clear();
    _clearCommandActivity();
    final next = _clientFactory(address);
    _draftHostAddress = address;
    final epoch = ++_epoch;
    final retiredSubscriptions = List<StreamSubscription<dynamic>>.of(
      _subscriptions,
    );
    _subscriptions.clear();
    final old = _client;
    _client = null;
    var published = false;
    bool current() => !_disposed && epoch == _epoch;
    _selection++;
    _historyScope?.cancel();
    _planScope?.cancel();
    _planChange = null;
    _listRequest = null;
    connecting = true;
    error = null;
    host = null;
    _muxReady = false;
    _hostReady = false;
    loading = false;
    sending = false;
    pending.clear();
    answering.clear();
    catalog = null;
    selectedId = null;
    historyTargetSeq = null;
    _holdingLiveHistory = false;
    sessions = [];
    archivedSessionIds = {};
    archivedSessions = [];
    _sessionRevisions.clear();
    _titleSequences.clear();
    workspaces = [];
    presets = [];
    subscriptionAccounts = [];
    workspaceId = null;
    _explicitDraftWorkspace = false;
    _resumeProvisionalDraft();
    conversationSettings = {};
    menuSettings = {};
    teamSettings = {};
    disabledPlugins = {};
    scheduleEnabled = null;
    queued = [];
    jobs = [];
    clearProjections();
    window = ConversationWindow();
    _forgetComputerUse();
    transcript = [];
    _buffer.clear();
    _bufferBytes = 0;
    _overflow = false;
    _refresh?.cancel();
    _listRefresh?.cancel();
    // Reset the visible draft scope before asynchronous subscription cleanup.
    // Keystrokes during a slow disconnect must not enter the next Host's slot.
    emit();
    try {
      try {
        await Future.wait(retiredSubscriptions.map((sub) => sub.cancel()));
      } finally {
        if (old != null && !identical(old, next)) {
          unawaited(old.close().catchError((Object _) {}));
        }
      }
      if (!current()) return;
      _client = next;
      published = true;
      emit();
      final description = await next.describe();
      if (!current()) return;
      host = description;
      preferences.address = address;
      await preferences.save();
      if (!current()) return;
      for (final name in ['mux', 'host']) {
        if (!current()) break;
        final channel = next.events(name);
        _subscriptions.add(
          channel.frames.listen(
            (frame) {
              if (epoch == _epoch) _onFrame(frame);
            },
            onError: (Object e) {
              if (epoch == _epoch) {
                error = DshRuntimeZh.eventConnectionInterrupted(error: e);
                emit();
              }
            },
          ),
        );
        _subscriptions.add(
          channel.states.listen((ready) {
            if (epoch != _epoch) return;
            final wasConnected = connected;
            if (name == 'mux') {
              _muxReady = ready;
              if (ready) {
                pending.clear();
                _commandScope?.cancel();
                _commandActivityRequest++;
              }
            } else {
              _hostReady = ready;
            }
            if (connected && !wasConnected) {
              error = null;
              unawaited(run(() => _restoreConnection(epoch)));
              unawaited(loadCatalogs());
            }
            emit();
          }),
        );
        channel.start();
      }
    } catch (_) {
      if (current()) rethrow;
    } finally {
      // Once published, ownership transfers to the next connect/dispose call.
      // A stale attempt closes only a client that it never published.
      if (!published && !identical(_client, next)) {
        await next.close().catchError((Object _) {});
      }
      if (current()) {
        connecting = false;
        emit();
      }
    }
  }

  Future<void> _restoreConnection(int epoch) async {
    final restoring = selectedId == null;
    final id = selectedId ?? preferences.sessionId;
    Object? historyError;
    StackTrace? historyStack;
    // Read the saved conversation immediately. Neither the complete sidebar
    // inventory nor provider/plugin discovery is needed to render its history.
    final history = id == null
        ? Future<void>.value()
        : restoring
        ? select(id)
        : Future.wait([
            refreshCommandActivity(),
            loadHistory(after: historyTargetSeq, targetSeq: historyTargetSeq),
          ]).then((_) {});
    final selection = _selection, workspace = workspaceId;
    final workspaceTargetRevision = _workspaceTargetRevision;
    bool current() => !_disposed && epoch == _epoch && selection == _selection;
    await Future.wait([
      history.catchError((Object e, StackTrace stack) {
        historyError = e;
        historyStack = stack;
      }),
      refreshSessions().then((_) {
        if (!current() || !restoring || id == null) return;
        if (!sessions.any((s) => s.id == id)) {
          // A deleted saved session must not remain selected or overwrite a
          // session the user chose while the sidebar was still loading.
          newConversation();
          return;
        }
        preset = selected?.agentPreset ?? preset;
        if (_workspaceTargetRevision == workspaceTargetRevision &&
            (workspaceId == workspace || workspace == null)) {
          final session = selected;
          workspaceId =
              (session == null ? null : workspaceOf(session))?['workspaceId']
                  as String? ??
              workspaceId;
        }
        emit();
      }),
    ]);
    if (current() && historyError != null) {
      Error.throwWithStackTrace(historyError!, historyStack!);
    }
  }

  Future<void> startHost() async {
    final running = await HostLauncher.live(preferences.address);
    if (running != null) {
      await connect(preferences.address);
      final selected = HostLauncher.packagedVersion(preferences.executable);
      if (selected != null && running.version != selected) {
        error = hostVersionMismatch(
          preferences.address,
          running.version,
          selected,
        );
        emit();
      }
      return;
    }
    connecting = true;
    emit();
    try {
      await HostLauncher.start(preferences.executable, preferences.address);
      await connect(preferences.address);
    } finally {
      connecting = false;
      emit();
    }
  }

  Future<void> refreshSessions() async {
    final api = _client;
    if (api == null) return;
    final epoch = _epoch;
    // Coalesce host status bursts without concurrent full-list requests.
    if (_listRequest != null) {
      await _listRequest;
      if (epoch != _epoch) return;
    }
    final task = () async {
      final revisions = Map<String, int>.of(_sessionRevisions);
      final responses = await Future.wait<Object>([
        api.sessions(),
        api.call('workspace.list'),
      ]);
      if (epoch != _epoch || _disposed) return;
      final result = responses[0] as List<SessionSummary>;
      final workspaces = responses[1] as Json;
      final archived = (workspaces['archivedSessionIds'] as List? ?? [])
          .whereType<String>()
          .toSet();
      for (final row in result) {
        final previous = sessions.where((s) => s.id == row.id).firstOrNull;
        if (previous != null &&
            revisions[row.id] != _sessionRevisions[row.id]) {
          row.running = previous.running;
          row.title = previous.title;
          row.titleEditBase = previous.titleEditBase;
          row.blank = previous.blank;
        }
        final titleSeq = row.titleEditBase?['throughSeq'];
        if (titleSeq is int && titleSeq >= (_titleSequences[row.id] ?? -1)) {
          _titleSequences[row.id] = titleSeq;
        } else if (previous != null && titleSeq is int) {
          row.title = previous.title;
          row.titleEditBase = previous.titleEditBase;
        }
      }
      sessions = result;
      archivedSessionIds = archived;
      archivedSessions = result
          .where((s) => archived.contains(s.id))
          .map(
            (s) => {
              'sessionId': s.id,
              'title': s.title,
              'cwd': s.cwd,
              'updatedAt': s.updatedAt,
            },
          )
          .toList();
      this.workspaces = objects(workspaces['items']);
      // Without a chosen Workspace, continue where the user last worked
      // instead of whichever Workspace happens to be listed first.
      workspaceId ??=
          _recentWorkspaceId ??
          this.workspaces.firstOrNull?['workspaceId'] as String?;
      emit();
    }();
    _listRequest = task;
    if (sessions.isEmpty) emit();
    try {
      await task;
    } finally {
      if (identical(_listRequest, task)) {
        _listRequest = null;
        if (sessions.isEmpty) emit();
      }
    }
  }

  Future<void> select(String id, {bool adoptDraft = false}) async {
    _clearCommandActivity();
    _historyScope?.cancel();
    _planScope?.cancel();
    _planChange = null;
    _refresh?.cancel();
    _selection++;
    _draftAdoptionRevision = adoptDraft ? _selection : null;
    selectedId = id;
    unreadHistoryEvents = 0;
    historyTargetSeq = null;
    _holdingLiveHistory = false;
    preset = selected?.agentPreset ?? preset;
    catalog = null;
    clearProjections();
    queued = [];
    jobs = [];
    final session = selected;
    workspaceId =
        (session == null ? null : workspaceOf(session))?['workspaceId']
            as String? ??
        workspaceId;
    loading = false;
    window = ConversationWindow();
    _forgetComputerUse();
    transcript = [];
    preferences.sessionId = id;
    final generation = _selection, epoch = _epoch;
    final readingPosition = readingPositions[id];
    emit();
    await Future.wait([
      () async {
        await loadHistory(
          after: readingPosition?.seq,
          holdForReading: readingPosition != null,
        );
        if (readingPosition != null &&
            generation == _selection &&
            epoch == _epoch &&
            !_disposed) {
          if (!transcript.any(
            (item) =>
                item.id == readingPosition.itemId ||
                item.seq == readingPosition.seq,
          )) {
            readingPositions.remove(id);
            await loadHistory(force: true);
          }
        }
      }(),
      refreshCommandActivity(),
      () async {
        final revision = _modelRevision;
        final models = await _client!.models(id);
        if (generation == _selection &&
            epoch == _epoch &&
            !_disposed &&
            revision == _modelRevision) {
          catalog = models;
          emit();
        }
      }(),
      preferences.save(),
    ]);
  }

  Future<void> loadHistory({
    int? before,
    int? after,
    bool merge = false,
    bool force = false,
    bool holdForReading = false,
    int? targetSeq,
  }) async {
    final id = selectedId, api = _client;
    if (id == null || api == null || (loading && !force)) return;
    if (_holdingLiveHistory &&
        !force &&
        !merge &&
        before == null &&
        after == null &&
        targetSeq == null) {
      window.needsRefresh = true;
      return;
    }
    final generation = _selection, epoch = _epoch;
    final request = ++_historyRequest;
    final oldTarget = historyTargetSeq;
    _paint?.cancel();
    if (force) _refresh?.cancel();
    final projectionVersion = projectionWindow.version;
    loading = true;
    _historyScope?.cancel();
    final scope = _historyScope = RequestScope();
    _buffer.clear();
    _bufferBytes = 0;
    _overflow = false;
    emit();
    try {
      final page = await api.history(
        id,
        before: before,
        after: after,
        scope: scope,
      );
      if (generation != _selection ||
          epoch != _epoch ||
          _disposed ||
          request != _historyRequest) {
        return;
      }
      if (merge) {
        window.mergePage(page, older: before != null);
      } else {
        final next = ConversationWindow()..replace(page);
        if (targetSeq != null &&
            !next.project().any(
              (m) => m.seq == targetSeq && m.kind == 'user',
            )) {
          throw StateError(DshRuntimeZh.messageNotFound);
        }
        window = next;
        unreadHistoryEvents = 0;
        historyTargetSeq = targetSeq;
        _heldActivity = interruptible || compacting;
        _holdingLiveHistory = holdForReading;
      }
      if (page.projections.isNotEmpty) {
        _title(
          id,
          object(page.projections['values'])['title'],
          (page.projections['asOfSeq'] as num?)?.toInt() ?? -1,
        );
        projectionWindow.snapshot(
          page.projections,
          requestVersion: projectionVersion,
        );
        projectionChanges.value++;
      }
      if (readingHistory) {
        if (_buffer.isNotEmpty) {
          window.needsRefresh = true;
          unreadHistoryEvents += _buffer.length;
        }
      } else {
        for (final event in _buffer) {
          window.append(event);
        }
      }
      if (_overflow) window.needsRefresh = true;
      transcript = window.project();
      if (window.needsRefresh && !window.hasAfter) _scheduleRefresh();
    } catch (_) {
      if (generation != _selection ||
          epoch != _epoch ||
          request != _historyRequest ||
          _disposed) {
        return;
      }
      if (generation == _selection &&
          epoch == _epoch &&
          request == _historyRequest) {
        historyTargetSeq = oldTarget;
      }
      rethrow;
    } finally {
      if (generation == _selection &&
          epoch == _epoch &&
          request == _historyRequest) {
        _buffer.clear();
        _bufferBytes = 0;
        loading = false;
        emit();
      }
    }
  }

  /// Keeps the reading window stable even when new live events arrive.
  void holdHistory(int seq) {
    if (!transcript.any((m) => m.kind == 'user' && m.seq == seq)) return;
    _historyScope?.cancel();
    _historyRequest++;
    _refresh?.cancel();
    _paint?.cancel();
    loading = false;
    historyTargetSeq = seq;
    _holdingLiveHistory = false;
    if (_buffer.isNotEmpty) window.needsRefresh = true;
    _buffer.clear();
    _bufferBytes = 0;
    emit();
  }

  /// Wheel/trackpad readers keep their current rows until they return to latest.
  /// Live events are represented by a refresh marker, as with indexed history.
  void holdLiveHistory() {
    if (_client == null || selectedId == null || readingHistory) return;
    _heldActivity = interruptible || compacting;
    _holdingLiveHistory = true;
    _historyScope?.cancel();
    _historyRequest++;
    _refresh?.cancel();
    if (_paint?.isActive == true || _buffer.isNotEmpty) {
      window.needsRefresh = true;
    }
    _paint?.cancel();
    loading = false;
    _buffer.clear();
    _bufferBytes = 0;
    emit();
  }

  Future<void> returnLatest() => loadHistory(force: true);

  void _scheduleRefresh() {
    if (readingHistory) return;
    if (_refresh?.isActive ?? false) return;
    _refresh = Timer(const Duration(milliseconds: 350), () {
      if (connected && !readingHistory) unawaited(run(() => loadHistory()));
    });
  }

  void _scheduleList() {
    _listRefresh?.cancel();
    _listRefresh = Timer(const Duration(milliseconds: 150), () {
      unawaited(run(refreshSessions));
    });
  }

  void _onFrame(HostFrame frame) {
    if (_disposed) return;
    final payload = frame.payload, type = frame.type;
    if (type == 'host/schedule-changed') {
      scheduleRevision++;
      if (payload['enabled'] is bool) {
        scheduleEnabled = payload['enabled'] as bool;
      }
      emit();
      return;
    }
    if (type == 'session/subscribed' && frame.sessionId != null) {
      final id = frame.sessionId!, lastSeq = payload['lastSeq'];
      if (lastSeq is int && lastSeq >= -1) {
        if ((_titleSequences[id] ?? -1) > lastSeq) {
          _titleSequences.remove(id);
          sessions
                  .where((session) => session.id == id)
                  .firstOrNull
                  ?.titleEditBase =
              null;
          _sessionChanged(id);
          _scheduleList();
        }
        if (id == selectedId && projectionWindow.truncate(lastSeq)) {
          _historyScope?.cancel();
          _historyRequest++;
          _refresh?.cancel();
          if (historyTargetSeq != null && historyTargetSeq! > lastSeq) {
            historyTargetSeq = lastSeq >= 0 ? lastSeq : null;
          }
          projectionChanges.value++;
          unawaited(
            run(
              () => loadHistory(
                after: historyTargetSeq,
                targetSeq: historyTargetSeq,
              ),
            ),
          );
        }
      }
      if (id == selectedId) unawaited(refreshCommandActivity());
    }
    if (type == 'session/projection' && payload['key'] != 'title') {
      if (frame.sessionId == selectedId &&
          payload['key'] is String &&
          payload['seq'] is num &&
          projectionWindow.apply(
            payload['key'] as String,
            payload['value'],
            (payload['seq'] as num).toInt(),
          )) {
        _projectionPaint ??= Timer(const Duration(milliseconds: 50), () {
          _projectionPaint = null;
          if (!_disposed) projectionChanges.value++;
        });
      }
      return;
    }
    if (type == 'session/queue' || type == 'session/jobs') {
      if (frame.sessionId == selectedId) {
        if (type == 'session/queue') {
          queued = objects(payload['items']);
        } else {
          jobs = objects(payload['jobs']);
        }
        emit();
      }
      return;
    }
    if (type == 'approval/requested' || type == 'question/requested') {
      pending[frame.rpcId] = frame;
    }
    if (type == 'approval/resolved') {
      pending.removeWhere(
        (_, f) =>
            f.sessionId == frame.sessionId &&
            f.payload['approvalId'] == payload['approvalId'],
      );
    }
    if (type == 'question/resolved') pending.remove(payload['questionRpcId']);
    if (type == 'host/session-status') {
      final row = sessions.where((s) => s.id == frame.sessionId).firstOrNull;
      if (row == null) {
        _scheduleList();
        return;
      }
      if (payload['running'] is! bool) return;
      final nextRunning = payload['running'] == true;
      if (row.running == nextRunning) return;
      final wasRunning = row.running;
      row.running = nextRunning;
      _sessionChanged(row.id);
      if (frame.sessionId == selectedId &&
          wasRunning &&
          !nextRunning &&
          !window.hasAfter) {
        _scheduleRefresh();
      }
    }
    if (type == 'host/session-added' ||
        type == 'host/session-removed' ||
        type == 'host/workspace-changed') {
      _scheduleList();
    }
    if (type == 'host/agent-error' && frame.sessionId == selectedId) {
      error = payload['message'] as String? ?? DshRuntimeZh.executionFailed;
    }
    if (type == 'stream/error') {
      error =
          object(payload['error'])['message'] as String? ??
          DshRuntimeZh.eventStreamError;
      if (!window.hasAfter) _scheduleRefresh();
    }
    if (type == 'session/projection' && payload['key'] == 'title') {
      if (frame.sessionId != null) {
        _title(
          frame.sessionId!,
          payload['value'],
          (payload['seq'] as num?)?.toInt() ?? 0,
        );
      }
    }
    if (type == 'session/event' && frame.sessionId != null) {
      final event = object(payload['event']),
          data = object(object(payload['event'])['data']);
      if (frame.sessionId == selectedId &&
          data['commandId'] is String &&
          (event['type'] == 'command/run' || event['type'] == 'command/done')) {
        final id = data['commandId'] as String;
        if (event['type'] == 'command/run' && data['name'] != 'goal') {
          _liveCommands.add(id);
        } else if (event['type'] == 'command/done') {
          _liveCommands.remove(id);
        }
        commandRunning = _liveCommands.isNotEmpty;
        emit();
        unawaited(refreshCommandActivity());
      }
      final row = sessions.where((s) => s.id == frame.sessionId).firstOrNull;
      if (event['type'] == 'session/title') {
        _title(
          frame.sessionId!,
          data['title'],
          (event['seq'] as num?)?.toInt() ?? 0,
        );
        emit();
      }
      if (row != null && event['type'] == 'user/message' && row.blank) {
        row.blank = false;
        _sessionChanged(row.id);
        emit();
      }
      if (row != null && event['type'] == 'turn/start' && !row.running) {
        row.running = true;
        _sessionChanged(row.id);
        emit();
      }
      if (row != null && event['type'] == 'turn/end') {
        _scheduleList();
      }
      if (frame.sessionId == selectedId &&
          event['type'] == 'computer-use/activity') {
        _computerUseActivity(event, data);
      }
    }
    if (frame.sessionId == selectedId && type == 'session/event') {
      final event = HistoryEvent.fromJson(
        object(payload['event']),
        view: payload['view'] == null ? null : object(payload['view']),
      );
      if (loading) {
        final size = event.retainedBytes;
        if (_buffer.length < 2000 && _bufferBytes + size < 2 * 1024 * 1024) {
          _buffer.add(event);
          _bufferBytes += size;
        } else {
          _overflow = true;
        }
        return;
      }
      if (readingHistory) {
        window.needsRefresh = true;
        unreadHistoryEvents++;
        if (_paint?.isActive != true) {
          _paint = Timer(const Duration(milliseconds: 72), emit);
        }
        return;
      }
      window.append(event);
      if (window.needsRefresh && !window.hasAfter) _scheduleRefresh();
      if (_paint?.isActive != true) {
        _paint = Timer(const Duration(milliseconds: 72), () {
          transcript = window.project();
          if (!_disposed) messageChanges.value++;
        });
      }
      return;
    }
    emit();
  }

  /// The Host records each successful `computer_use` call of a connected
  /// session. Show the session once per control identity, and again when the
  /// model starts it anew.
  void _computerUseActivity(Json event, Json data) {
    final seq = (event['seq'] as num?)?.toInt() ?? -1;
    final session = data['browserSessionId'], target = data['target'];
    if (seq <= _computerUseSeq ||
        data['ownerSessionId'] != selectedId ||
        session is! String ||
        session.isEmpty ||
        session.length > 256 ||
        target is! String ||
        !const {'local', 'remote', 'browser'}.contains(target)) {
      return;
    }
    _computerUseSeq = seq;
    final key = jsonEncode([target, session, data['controlId']]);
    if (!_computerUseShown.add(key) && data['action'] != 'start') return;
    if (_computerUseShown.length > 256) {
      _computerUseShown.remove(_computerUseShown.first);
    }
    computerUse = ComputerUseBinding(browserSessionId: session, target: target);
    computerUseRequests.value++;
  }

  Future<String?> create(String cwd) async {
    if (!connected) throw StateError(DshRuntimeZh.connectLocalService);
    if (cwd.trim().isEmpty) {
      throw const FormatException(DshRuntimeZh.workingDirectoryRequired);
    }
    final api = _client!, epoch = _epoch, selection = _selection;
    final ownerWorkspace = workspaceId;
    final fromDraft = selectedId == null;
    final draftScope = fromDraft ? draftScopeKey : null;
    final workspaceTarget = _workspaceTargetRevision;
    final id =
        (await api.call('session.create', {
              'cwd': cwd.trim(),
              'agentPreset': preset,
              'workspaceId': ?ownerWorkspace,
            }, true))['sessionId']
            as String;
    bool current() =>
        !_disposed &&
        epoch == _epoch &&
        selection == _selection &&
        ownerWorkspace == workspaceId &&
        workspaceTarget == _workspaceTargetRevision &&
        (!fromDraft || draftScope == draftScopeKey);
    if (!current()) return null;
    await refreshSessions();
    if (!current()) return null;
    // The Host acknowledged creation and this scope still owns the request.
    // Move the latest text synchronously with adoption; subsequent history or
    // model loading failures leave a recoverable draft on the created session.
    if (draftScope != null) {
      final text = preferences.drafts[draftScope];
      if (text != null) preferences.drafts[id] = text;
      preferences.drafts.remove(draftScope);
    }
    await select(id, adoptDraft: fromDraft);
    return !_disposed &&
            epoch == _epoch &&
            _selection == selection + 1 &&
            workspaceTarget == _workspaceTargetRevision &&
            selectedId == id
        ? id
        : null;
  }

  Future<void> send(String text) async {
    final id = selectedId, api = _client;
    if (id == null ||
        api == null ||
        !connected ||
        sending ||
        changingPlanMode ||
        changingModel ||
        text.trim().isEmpty) {
      return;
    }
    final epoch = _epoch;
    sending = true;
    emit();
    try {
      final result = await api.prompt(id, text, requestId: newRequestId());
      if (result['accepted'] != true) {
        throw StateError(DshRuntimeZh.messageRejected);
      }
      if (preferences.drafts[id] == text) {
        preferences.drafts.remove(id);
        await preferences.save();
      }
      if (epoch != _epoch) return;
      final row = sessions.where((s) => s.id == id).firstOrNull;
      if (result['running'] is bool) row?.running = result['running'] as bool;
      if (selectedId == id) await loadHistory();
      _scheduleList();
    } finally {
      if (epoch == _epoch) {
        sending = false;
        emit();
      }
    }
  }

  Future<String?> sendParts(
    String text,
    List<Json> attachments, {
    String mode = 'queue',
  }) async {
    if (!connected ||
        sending ||
        changingPlanMode ||
        changingModel ||
        _disposed) {
      return null;
    }
    error = null;
    final api = _client!, epoch = _epoch, selection = _selection;
    final starting = _startingConversation;
    var id = selectedId;
    final fromDraft = id == null;
    if (fromDraft && text.isNotEmpty) setDraft(text);
    sending = true;
    emit();
    try {
      if (id == null && starting != null) {
        id = await starting;
        if (id == null || _selection != selection + 1) return null;
      } else if (id == null) {
        final path = currentWorkspace?['path'] as String?;
        if (path == null) throw StateError(DshRuntimeZh.selectWorkspace);
        id = await create(path);
        if (id == null || _selection != selection + 1) return null;
      } else if (_selection != selection) {
        return null;
      }
      if (_disposed || epoch != _epoch || selectedId != id) return null;
      final requestSelection = _selection;
      // Hero creation may have adopted newer typing while the request waited.
      // Preserve that text rather than replacing it with the submitted snapshot.
      if (text.isNotEmpty && !fromDraft) preferences.drafts[id] = text;
      final slash = RegExp(r'^/([^\s]+)(?:\s|$)').firstMatch(text.trimLeft());
      var command = false;
      if (slash != null) {
        final name = slash.group(1);
        final available = await api.availableCommands(id);
        if (_disposed ||
            epoch != _epoch ||
            _selection != requestSelection ||
            selectedId != id) {
          return null;
        }
        command = available.any((candidate) => candidate['name'] == name);
      }
      if (command && attachments.isNotEmpty) {
        throw StateError(DshRuntimeZh.runCommandBeforeAttachments);
      }
      final result = command
          ? await api.call('commands.execute', {
              'args': {'agentId': id, 'line': text.trim()},
            }, true)
          : await api.call('session.prompt', {
              'sessionId': id,
              'mode': mode,
              'requestId': newRequestId(),
              'content': [
                if (text.trim().isNotEmpty) {'type': 'text', 'text': text},
                ...attachments,
              ],
            }, true);
      if (_disposed || epoch != _epoch) return null;
      if (command) {
        final outcome = object(result['result']);
        if (outcome['kind'] != 'success') {
          throw StateError(
            '${outcome['text'] ?? DshRuntimeZh.planCommandRejected}',
          );
        }
      } else if (result['accepted'] != true) {
        throw StateError(DshRuntimeZh.messageRejected);
      }
      try {
        if (preferences.drafts[id] == text) {
          preferences.drafts.remove(id);
          await preferences.save();
        }
        if (epoch != _epoch || _disposed) return null;
        await refreshSessions();
        if (epoch == _epoch && selectedId == id) await loadHistory();
      } catch (e) {
        if (epoch == _epoch && !_disposed) {
          error = DshRuntimeZh.sentButRefreshFailed(error: e);
        }
      }
      return id;
    } catch (_) {
      if (epoch != _epoch || _disposed) return null;
      rethrow;
    } finally {
      if (epoch == _epoch) {
        sending = false;
        emit();
      }
    }
  }

  Future<void> loadCatalogs() async {
    final api = _client, epoch = _epoch;
    if (api == null) return;
    Future<void> optional(Future<void> Function() action) async {
      try {
        await action();
      } catch (_) {}
    }

    await Future.wait([
      optional(() async {
        final result = await api.call('agentPreset.list');
        if (_disposed || epoch != _epoch) return;
        presets = objects(result['presets']);
        if (selectedId == null) {
          preset =
              presets.where((p) => p['isDefault'] == true).firstOrNull?['id']
                  as String? ??
              preset;
        }
        emit();
      }),
      optional(() async {
        final description = await api.call('settings.describe');
        if (_disposed || epoch != _epoch) return;
        final namespaces = objects(description['namespaces']);
        Json settings(String name) => object(
          namespaces.where((n) => n['ns'] == name).firstOrNull?['value'],
        );
        conversationSettings = settings('ui-conversation');
        menuSettings = settings('mini-menu');
        teamSettings = settings('agent-teams');
        contextCompaction = settings('context-compaction');
        emit();
      }),
      optional(loadAccounts),
      optional(loadPlugins),
    ]);
  }

  Future<void> loadPlugins() async {
    final api = _client, epoch = _epoch;
    if (api == null) return;
    final result = await api.call('pluginInventory.list');
    if (_disposed || epoch != _epoch) return;
    disabledPlugins = {
      for (final entry in objects(result['entries']))
        if (entry['enabled'] == false)
          for (final key in ['moduleName', 'entryId', 'id', 'name'])
            if (entry[key] is String) entry[key] as String,
    };
    emit();
  }

  Future<void> loadAccounts() async {
    final api = _client, epoch = _epoch;
    if (api == null) return;
    final result = await api.request('/provider-auth/providers', body: {});
    if (!_disposed && epoch == _epoch) {
      subscriptionAccounts = objects(result['providers']);
      emit();
    }
  }

  void newConversation() {
    _clearCommandActivity();
    _startingConversation = null;
    _historyScope?.cancel();
    _planScope?.cancel();
    _planChange = null;
    _selection++;
    _refresh?.cancel();
    selectedId = null;
    _resumeProvisionalDraft();
    historyTargetSeq = null;
    _holdingLiveHistory = false;
    catalog = null;
    loading = false;
    clearProjections();
    queued = [];
    jobs = [];
    window = ConversationWindow();
    _forgetComputerUse();
    transcript = [];
    preferences.sessionId = null;
    emit();
  }

  Future<void> startConversation() async {
    if (_startingConversation != null || !connected || _disposed) return;
    newConversation();
    final path = currentWorkspace?['path'] as String?;
    if (path == null || path.trim().isEmpty) return;
    final created = create(path);
    _startingConversation = created;
    emit();
    try {
      await created;
    } finally {
      if (identical(_startingConversation, created)) {
        _startingConversation = null;
        emit();
      }
    }
  }

  void clearProjections() {
    _projectionPaint?.cancel();
    _projectionPaint = null;
    projectionWindow.clear();
    if (!_disposed) projectionChanges.value++;
  }

  Future<void> updateTodos(List<Json> expected, Json action) async {
    if (!canEditTodos) {
      throw StateError(DshRuntimeZh.taskEditsUnsupported);
    }
    final id = selectedId, api = _client;
    if (id == null || api == null) throw StateError(DshRuntimeZh.selectSession);
    final accepted = await api.call('session.updateTodos', {
      'sessionId': id,
      'expected': expected,
      'action': action,
    }, true);
    if (accepted['accepted'] != true) {
      throw StateError(DshRuntimeZh.taskUpdateRejected);
    }
    if (selectedId == id) await loadHistory();
  }

  Future<void> changeGoal(
    String operation,
    Json goal, {
    String? objective,
  }) async {
    if (!{'pause', 'resume', 'edit', 'clear'}.contains(operation)) {
      throw ArgumentError.value(operation);
    }
    final id = selectedId, api = _client;
    if (id == null || api == null) throw StateError(DshRuntimeZh.selectSession);
    await api.call('goal.$operation', {
      'sessionId': id,
      'ref': {'id': goal['id'], 'revision': goal['revision']},
      'objective': ?objective,
    }, true);
    if (selectedId == id) await loadHistory();
  }

  Future<void> addWorkspace(String path) async {
    final api = _client, epoch = _epoch, selection = _selection;
    final workspace = workspaceId;
    if (api == null || !connected) {
      throw StateError(DshRuntimeZh.connectService);
    }
    bool current() =>
        !_disposed &&
        epoch == _epoch &&
        selection == _selection &&
        workspace == workspaceId;
    final value = await api.call('workspace.create', {'path': path}, true);
    if (!current()) return;
    final created = object(value['workspace'])['workspaceId'];
    if (created is! String) {
      throw StateError(DshRuntimeZh.invalidWorkspaceResponse);
    }
    await refreshSessions();
    if (!current()) return;
    targetWorkspace(created);
    await startConversation();
  }

  Json? titleEditBase(String id) {
    final base = sessions
        .where((session) => session.id == id)
        .firstOrNull
        ?.titleEditBase;
    return base == null ? null : Map<String, dynamic>.of(base);
  }

  Future<void> renameSession(
    String id,
    String title, {
    Json? expectedTitle,
    DshClient? expectedClient,
  }) async {
    if (expectedClient != null && !identical(expectedClient, _client)) {
      throw StateError(DshRuntimeZh.titleConnectionChanged);
    }
    final api = _client, epoch = _epoch;
    if (api == null || !connected) {
      throw StateError(DshRuntimeZh.connectService);
    }
    final base = expectedTitle ?? titleEditBase(id);
    if (base == null) throw StateError(DshRuntimeZh.titleNotLoaded);
    final result = await api.call('session.rename', {
      'sessionId': id,
      'title': title,
      'expectedTitle': base,
    }, true);
    if (epoch != _epoch || _disposed) return;
    if (result['seq'] is int) _title(id, result['title'], result['seq'] as int);
    await refreshSessions();
  }

  Future<void> archive(
    String id, {
    bool restore = false,
    bool stopSchedules = false,
  }) async {
    final api = _client!, epoch = _epoch;
    if (restore) {
      await api.call('workspace.unarchiveSession', {'sessionId': id}, true);
    } else {
      await api.archiveSession(id, stopSchedules: stopSchedules);
    }
    if (_disposed || epoch != _epoch) return;
    if (!restore && selectedId == id) newConversation();
    await refreshSessions();
  }

  /// Automatic compaction threshold for one `provider/model`; null restores
  /// the default.
  Future<void> setCompactionThreshold(String key, double? ratio) async {
    final api = _client;
    if (api == null) throw StateError(DshRuntimeZh.connectLocalService);
    final thresholds = {...object(contextCompaction['thresholds'])};
    if (ratio == null) {
      thresholds.remove(key);
    } else {
      thresholds[key] = ratio;
    }
    await api.call('settings.replace', {
      'ns': 'context-compaction',
      'section': {'thresholds': thresholds},
    }, true);
    if (_disposed || api != _client) return;
    contextCompaction = {...contextCompaction, 'thresholds': thresholds};
    emit();
  }

  /// Run the `/compact` command in the selected session now.
  Future<void> compactNow() async {
    final api = _client, id = selectedId;
    if (api == null || id == null) throw StateError(DshRuntimeZh.openSession);
    final result = await api.call('commands.execute', {
      'args': {'agentId': id, 'line': '/compact'},
    }, true);
    final outcome = object(result['result']);
    if (outcome.isNotEmpty && outcome['kind'] != 'success') {
      throw StateError(
        '${outcome['text'] ?? DshRuntimeZh.compactionNotStarted}',
      );
    }
  }

  Future<void> setReasoning(String effort) async {
    final current = catalog;
    if (selectedId == null || current == null) return;
    final choice = current.choices
        .where((model) => model.key == current.currentKey)
        .firstOrNull;
    if (choice == null ||
        !choice.reasoning.any((level) => level['id'] == effort)) {
      throw StateError('当前模型不支持该推理等级');
    }
    if (current.current['reasoningEffort'] == effort) return;
    await _changeModelSelection({
      ...current.current,
      'reasoningEffort': effort,
    });
  }

  Future<void> rememberModel(
    DshClient api,
    Json current, {
    bool Function()? isCurrent,
  }) async {
    final previous = _modelDefaultSaves[api] ?? Future<void>.value();
    final saving = () async {
      await previous;
      // A write already accepted by the Host cannot be recalled. Serialize
      // newer defaults after it, and discard queued writes whose scope retired.
      if (_disposed || api != _client || isCurrent?.call() == false) return;
      try {
        await api.call('settings.replace', {
          'ns': 'agent-default-model',
          'section': {
            'provider': current['provider'],
            'model': current['model'],
            'executionMode': current['executionMode'] ?? 'standard',
            if (current['reasoningEffort'] != null)
              'reasoningEffort': current['reasoningEffort'],
          },
        }, true);
      } catch (failure) {
        if (!_disposed && api == _client && isCurrent?.call() != false) {
          error = DshRuntimeZh.defaultModelSaveFailed(error: failure);
        }
      }
    }();
    _modelDefaultSaves[api] = saving;
    try {
      await saving;
    } finally {
      if (identical(_modelDefaultSaves[api], saving)) {
        _modelDefaultSaves.remove(api);
      }
    }
  }

  Future<void> stop() async {
    final id = selectedId, api = _client;
    if (id == null || api == null) return;
    await api.cancel(id);
    await refreshCommandActivity();
    await refreshSessions();
  }

  Future<void> chooseModel(ModelChoice model) async {
    if (catalog?.currentKey == model.key) return;
    await _changeModelSelection({
      'provider': model.provider,
      'model': model.id,
    });
  }

  Future<void> refreshModels() async {
    final api = _client,
        id = selectedId,
        generation = _selection,
        epoch = _epoch,
        revision = _modelRevision;
    if (api == null || id == null || changingModel) return;
    bool current() =>
        !_disposed &&
        api == _client &&
        epoch == _epoch &&
        generation == _selection &&
        revision == _modelRevision;
    try {
      final next = await api.models(id);
      if (!current()) return;
      catalog = next;
      emit();
    } catch (_) {
      if (current()) rethrow;
    }
  }

  Future<void> _changeModelSelection(Json selection) async {
    final id = selectedId,
        api = _client,
        generation = _selection,
        epoch = _epoch;
    if (_disposed || id == null || api == null) return;
    if (sending) throw StateError('正在提交消息，请稍候再调整模型或推理等级');
    if (changingModel) throw StateError('正在切换模型或推理等级，请稍候');
    final token = Object();
    _modelRevision++;
    _modelChange = token;
    _modelChangeSelection = generation;
    error = null;
    bool current() =>
        !_disposed &&
        epoch == _epoch &&
        identical(api, _client) &&
        generation == _selection &&
        selectedId == id &&
        identical(_modelChange, token);
    emit();
    try {
      final result = await api.call('session.selectModel', {
        ...selection,
        'sessionId': id,
      }, true);
      if (!current()) return;
      final confirmed = object(result['selected']);
      if (confirmed['provider'] is String && confirmed['model'] is String) {
        final previous = catalog;
        catalog = ModelCatalog.fromJson({
          'current': confirmed,
          'routable': true,
          'failures': previous?.failures ?? const <Json>[],
          'groups': [
            for (final provider
                in previous?.providerNames.entries ??
                    const <MapEntry<String, String>>[])
              {
                'id': provider.key,
                'name': provider.value,
                'models': [
                  for (final model in previous!.choices.where(
                    (model) => model.provider == provider.key,
                  ))
                    {
                      'id': model.id,
                      'name': model.name,
                      'reasoning': {'efforts': model.reasoning},
                    },
                ],
              },
          ],
        });
        emit();
      }
      ModelCatalog next;
      try {
        next = await api.models(id);
      } catch (failure) {
        if (!current()) return;
        if (confirmed.isNotEmpty) {
          await rememberModel(api, confirmed, isCurrent: current);
          if (!current()) return;
          throw StateError('模型或推理等级已更新，但刷新列表失败：$failure');
        }
        rethrow;
      }
      if (!current()) return;
      catalog = next;
      await rememberModel(api, next.current, isCurrent: current);
      if (current()) emit();
    } catch (_) {
      if (current()) rethrow;
    } finally {
      if (identical(_modelChange, token)) {
        _modelChange = null;
        if (!_disposed) emit();
      }
    }
  }

  Future<void> answer(HostFrame frame, Json value) async {
    await _settleInteraction(frame, (api) => api.respond(frame, value));
  }

  Future<void> setPlanMode(bool enabled) async {
    final api = _client,
        id = selectedId,
        generation = _selection,
        epoch = _epoch;
    if (api == null ||
        id == null ||
        !connected ||
        sending ||
        changingPlanMode) {
      return;
    }
    final token = Object(), scope = RequestScope();
    _planScope?.cancel();
    _planScope = scope;
    _planChange = token;
    _planSelection = generation;
    error = null;
    emit();
    bool current() =>
        !_disposed &&
        epoch == _epoch &&
        generation == _selection &&
        identical(_planChange, token);
    var accepted = false;
    try {
      final result = await api.rpc(
        'commands.execute',
        payload: {
          'args': {'agentId': id, 'line': enabled ? '/plan' : '/plan off'},
        },
        mutation: true,
        scope: scope,
      );
      if (!current()) return;
      final outcome = object(result['result']);
      if (outcome['kind'] != 'success') {
        throw StateError(
          '${outcome['text'] ?? DshRuntimeZh.planCommandRejected}',
        );
      }
      accepted = true;
      final version = projectionWindow.version;
      final page = await api.rpc(
        'session.history',
        payload: {'sessionId': id, 'maxMessages': 1},
        scope: scope,
      );
      if (!current()) return;
      projectionWindow.snapshot(
        object(page['projections']),
        requestVersion: version,
      );
      projectionChanges.value++;
    } catch (e) {
      if (!current() || scope.cancelled) return;
      if (accepted) throw StateError(DshRuntimeZh.planRefreshFailed(error: e));
      rethrow;
    } finally {
      scope.cancel();
      if (identical(_planChange, token)) {
        _planChange = null;
        emit();
      }
    }
  }

  Future<void> cancelQuestion(HostFrame frame) async {
    await _settleInteraction(frame, (api) => api.cancelQuestion(frame));
  }

  Future<void> _settleInteraction(
    HostFrame frame,
    Future<bool> Function(DshClient) respond,
  ) async {
    if (answering.contains(frame.rpcId) || !connected) return;
    final api = _client!, epoch = _epoch;
    answering.add(frame.rpcId);
    emit();
    try {
      final accepted = await respond(api);
      if (epoch != _epoch || _disposed) return;
      pending.remove(frame.rpcId);
      if (!accepted) error = DshRuntimeZh.interactionAlreadyHandled;
    } finally {
      if (epoch == _epoch && !_disposed) {
        answering.remove(frame.rpcId);
        emit();
      }
    }
  }

  @override
  void dispose() {
    _disposed = true;
    _clearCommandActivity();
    _historyScope?.cancel();
    _planScope?.cancel();
    messageChanges.dispose();
    themeChanges.dispose();
    conversationChanges.dispose();
    connectionChanges.dispose();
    interactionChanges.dispose();
    _projectionPaint?.cancel();
    projectionChanges.dispose();
    composerFocus.dispose();
    computerUseRequests.dispose();
    _epoch++;
    _selection++;
    _paint?.cancel();
    _refresh?.cancel();
    _draftSave?.cancel();
    _listRefresh?.cancel();
    final retiredSubscriptions = List<StreamSubscription<dynamic>>.of(
      _subscriptions,
    );
    _subscriptions.clear();
    final retiredClient = _client;
    _client = null;
    for (final sub in retiredSubscriptions) {
      unawaited(sub.cancel().catchError((Object _) {}));
    }
    unawaited(retiredClient?.close().catchError((Object _) {}));
    unawaited(preferences.save().catchError((Object _) {}));
    super.dispose();
  }
}
