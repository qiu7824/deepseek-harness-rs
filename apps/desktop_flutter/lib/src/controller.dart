import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:dsh_client/dsh_client.dart';

import 'preferences.dart';

String permissionName(String value) =>
    const {
      'workspace-write': '工作区内修改',
      'danger-full-access': '完全访问',
      'full-access': '完全访问',
      'read-only': '只读',
    }[value] ??
    value;

class DesktopController extends ChangeNotifier {
  DesktopController(
    this.preferences, {
    DshClient Function(String)? clientFactory,
  }) : _clientFactory = clientFactory ?? ((address) => DshClient(address));
  final DesktopPreferences preferences;
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
      const {'standard': '标准模式', 'blank': '空白模式', 'code': '代码模式'}[preset] ??
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
  Set<String> disabledPlugins = {};
  bool pluginEnabled(String name) => !disabledPlugins.contains(name);
  final messageChanges = ValueNotifier<int>(0);
  final composerFocus = ValueNotifier<int>(0);
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
  bool get readingHistory => historyTargetSeq != null;
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
  String? selectedId;
  ModelCatalog? catalog;
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
  int? _draftAdoptionRevision;
  bool get selectionAdoptsDraft => _draftAdoptionRevision == _selection;
  final List<StreamSubscription<dynamic>> _subscriptions = [];
  final List<HistoryEvent> _buffer = [];
  int _bufferBytes = 0;
  bool _overflow = false;
  Timer? _paint, _refresh, _draftSave, _listRefresh;
  Future<void>? _listRequest;
  Future<String?>? _startingConversation;
  SessionSummary? get selected =>
      sessions.where((e) => e.id == selectedId).firstOrNull;
  List<HostFrame> get interactions =>
      pending.values.where((f) => f.sessionId == selectedId).toList();
  String get draft => preferences.drafts[selectedId] ?? '';
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
    if (!_disposed) notifyListeners();
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
      try {
        await connect(preferences.address);
      } catch (_) {
        if (included == null) rethrow;
        await HostLauncher.start(included, preferences.address);
        await connect(preferences.address);
      }
    });
  }

  void setDraft(String text) {
    final id = selectedId;
    if (id == null) return;
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
    _clearCommandActivity();
    final next = _clientFactory(address);
    final epoch = ++_epoch;
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
    sessions = [];
    archivedSessionIds = {};
    archivedSessions = [];
    _sessionRevisions.clear();
    _titleSequences.clear();
    workspaces = [];
    presets = [];
    subscriptionAccounts = [];
    workspaceId = null;
    conversationSettings = {};
    menuSettings = {};
    teamSettings = {};
    disabledPlugins = {};
    queued = [];
    jobs = [];
    clearProjections();
    window = ConversationWindow();
    transcript = [];
    _buffer.clear();
    _bufferBytes = 0;
    _overflow = false;
    _refresh?.cancel();
    _listRefresh?.cancel();
    for (final sub in _subscriptions) {
      await sub.cancel();
    }
    _subscriptions.clear();
    final old = _client;
    _client = next;
    if (old != null) unawaited(old.close());
    emit();
    try {
      final description = await next.describe();
      if (_disposed || epoch != _epoch) return;
      host = description;
      preferences.address = address;
      await preferences.save();
      for (final name in ['mux', 'host']) {
        final channel = next.events(name);
        _subscriptions.add(
          channel.frames.listen(
            (frame) {
              if (epoch == _epoch) _onFrame(frame);
            },
            onError: (Object e) {
              if (epoch == _epoch) {
                error = '事件连接中断，正在恢复：$e';
                emit();
              }
            },
          ),
        );
        _subscriptions.add(
          channel.states.listen((ready) {
            if (epoch != _epoch) return;
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
            if (connected) {
              error = null;
              unawaited(
                run(() async {
                  await refreshSessions();
                  if (epoch != _epoch || _disposed) return;
                  await loadCatalogs();
                  if (selectedId != null) {
                    await refreshCommandActivity();
                    await loadHistory(
                      after: historyTargetSeq,
                      targetSeq: historyTargetSeq,
                    );
                  } else {
                    final saved = preferences.sessionId;
                    final id = sessions.any((s) => s.id == saved)
                        ? saved
                        : null;
                    if (id != null) await select(id);
                  }
                }),
              );
            }
            emit();
          }),
        );
        channel.start();
      }
    } finally {
      if (epoch == _epoch) {
        connecting = false;
        emit();
      }
    }
  }

  Future<void> startHost() async {
    try {
      final probe = DshClient(
        preferences.address,
        timeout: const Duration(seconds: 2),
      );
      try {
        await probe.describe();
        await connect(preferences.address);
        return;
      } finally {
        await probe.close();
      }
    } catch (_) {
      /* No live Host: launch the selected installation. */
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
      final result = await api.sessions();
      if (epoch != _epoch || _disposed) return;
      final workspaces = await api.call('workspace.list');
      if (epoch != _epoch || _disposed) return;
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
      workspaceId ??= this.workspaces.firstOrNull?['workspaceId'] as String?;
      emit();
    }();
    _listRequest = task;
    try {
      await task;
    } finally {
      if (identical(_listRequest, task)) _listRequest = null;
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
    historyTargetSeq = null;
    preset = selected?.agentPreset ?? preset;
    catalog = null;
    clearProjections();
    queued = [];
    jobs = [];
    workspaceId =
        workspaces
                .where((w) => (w['sessionIds'] as List? ?? []).contains(id))
                .firstOrNull?['workspaceId']
            as String? ??
        workspaceId;
    loading = false;
    window = ConversationWindow();
    transcript = [];
    preferences.sessionId = id;
    final generation = _selection, epoch = _epoch;
    emit();
    await Future.wait([
      loadHistory(),
      refreshCommandActivity(),
      () async {
        final models = await _client!.models(id);
        if (generation == _selection && epoch == _epoch && !_disposed) {
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
    int? targetSeq,
  }) async {
    final id = selectedId, api = _client;
    if (id == null || api == null || (loading && !force)) return;
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
          throw StateError('无法定位该消息，请刷新索引后重试。');
        }
        window = next;
        historyTargetSeq = targetSeq;
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
        if (_buffer.isNotEmpty) window.needsRefresh = true;
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
    if (_buffer.isNotEmpty) window.needsRefresh = true;
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
      error = payload['message'] as String? ?? '任务执行失败';
    }
    if (type == 'stream/error') {
      error = object(payload['error'])['message'] as String? ?? '事件流错误';
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

  Future<String?> create(String cwd) async {
    if (!connected) throw StateError('请先连接本机服务');
    if (cwd.trim().isEmpty) throw const FormatException('请输入工作目录');
    final api = _client!, epoch = _epoch, selection = _selection;
    final ownerWorkspace = workspaceId;
    final fromDraft = selectedId == null;
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
        ownerWorkspace == workspaceId;
    if (!current()) return null;
    await refreshSessions();
    if (!current()) return null;
    await select(id, adoptDraft: fromDraft);
    return !_disposed &&
            epoch == _epoch &&
            _selection == selection + 1 &&
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
        text.trim().isEmpty) {
      return;
    }
    final epoch = _epoch;
    sending = true;
    emit();
    try {
      final result = await api.prompt(id, text, requestId: newRequestId());
      if (result['accepted'] != true) throw StateError('消息未被接受');
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
    if (!connected || sending || changingPlanMode || _disposed) return null;
    error = null;
    final api = _client!, epoch = _epoch, selection = _selection;
    final starting = _startingConversation;
    var id = selectedId;
    sending = true;
    emit();
    try {
      if (id == null && starting != null) {
        id = await starting;
        if (id == null || _selection != selection + 1) return null;
      } else if (id == null) {
        final path = currentWorkspace?['path'] as String?;
        if (path == null) throw StateError('请先选择工作区');
        id = await create(path);
        if (id == null || _selection != selection + 1) return null;
      } else if (_selection != selection) {
        return null;
      }
      if (_disposed || epoch != _epoch || selectedId != id) return null;
      final requestSelection = _selection;
      if (text.isNotEmpty) preferences.drafts[id] = text;
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
        throw StateError('请先执行命令，再发送附件。');
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
          throw StateError('${outcome['text'] ?? '计划命令未被接受'}');
        }
      } else if (result['accepted'] != true) {
        throw StateError('消息未被接受');
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
        if (epoch == _epoch && !_disposed) error = '消息已发送，但刷新失败：$e';
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
    try {
      final result = await api.call('agentPreset.list');
      if (epoch == _epoch) {
        presets = objects(result['presets']);
        if (selectedId == null) {
          preset =
              presets.where((p) => p['isDefault'] == true).firstOrNull?['id']
                  as String? ??
              preset;
        }
      }
      final description = await api.call('settings.describe');
      if (epoch == _epoch && !_disposed) {
        conversationSettings = object(
          objects(description['namespaces'])
              .where((n) => n['ns'] == 'ui-conversation')
              .firstOrNull?['value'],
        );
        menuSettings = object(
          objects(description['namespaces'])
              .where((n) => n['ns'] == 'mini-menu')
              .firstOrNull?['value'],
        );
        teamSettings = object(
          objects(description['namespaces'])
              .where((n) => n['ns'] == 'agent-teams')
              .firstOrNull?['value'],
        );
      }
      await loadAccounts();
    } catch (_) {}
    try {
      await loadPlugins();
    } catch (_) {}
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
    historyTargetSeq = null;
    catalog = null;
    loading = false;
    clearProjections();
    queued = [];
    jobs = [];
    window = ConversationWindow();
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
      throw StateError('当前 Host 不支持安全保存任务编辑，请更新 Host 后重试。');
    }
    final id = selectedId, api = _client;
    if (id == null || api == null) throw StateError('请先选择会话');
    final accepted = await api.call('session.updateTodos', {
      'sessionId': id,
      'expected': expected,
      'action': action,
    }, true);
    if (accepted['accepted'] != true) throw StateError('任务更新未被接受');
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
    if (id == null || api == null) throw StateError('请先选择会话');
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
    if (api == null || !connected) throw StateError('请先连接服务');
    bool current() =>
        !_disposed &&
        epoch == _epoch &&
        selection == _selection &&
        workspace == workspaceId;
    final value = await api.call('workspace.create', {'path': path}, true);
    if (!current()) return;
    final created = object(value['workspace'])['workspaceId'];
    if (created is! String) throw StateError('服务未返回有效的工作区。');
    await refreshSessions();
    if (!current()) return;
    workspaceId = created;
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
      throw StateError('服务连接已改变，请重新打开标题编辑。');
    }
    final api = _client, epoch = _epoch;
    if (api == null || !connected) throw StateError('请先连接服务');
    final base = expectedTitle ?? titleEditBase(id);
    if (base == null) throw StateError('标题状态尚未加载，请读取最新状态。');
    final result = await api.call('session.rename', {
      'sessionId': id,
      'title': title,
      'expectedTitle': base,
    }, true);
    if (epoch != _epoch || _disposed) return;
    if (result['seq'] is int) _title(id, result['title'], result['seq'] as int);
    await refreshSessions();
  }

  Future<void> archive(String id, {bool restore = false}) async {
    await _client!.call(
      restore ? 'workspace.unarchiveSession' : 'workspace.archiveSession',
      {'sessionId': id},
      true,
    );
    if (!restore && selectedId == id) newConversation();
    await refreshSessions();
  }

  Future<void> setReasoning(String effort) async {
    if (selectedId == null || catalog == null) return;
    final api = _client!,
        id = selectedId!,
        epoch = _epoch,
        selection = _selection;
    await api.call('session.selectModel', {
      ...catalog!.current,
      'sessionId': id,
      'reasoningEffort': effort,
    }, true);
    final next = await api.models(id);
    if (_disposed || epoch != _epoch || selection != _selection) return;
    catalog = next;
    await rememberModel(api, next.current);
    emit();
  }

  Future<void> rememberModel(DshClient api, Json current) async {
    if (_disposed || api != _client) return;
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
      if (!_disposed && api == _client) {
        error = '当前会话模型已切换，但默认模型保存失败：$failure';
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
    final id = selectedId, api = _client, generation = _selection;
    if (id == null || api == null) return;
    await api.selectModel(id, model);
    final next = await api.models(id);
    if (!_disposed && api == _client && generation == _selection) {
      catalog = next;
      await rememberModel(api, next.current);
      emit();
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
        throw StateError('${outcome['text'] ?? '计划命令未被接受'}');
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
      if (accepted) throw StateError('计划命令已被接受，但状态刷新失败：$e');
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
      if (!accepted) error = '此请求已经结束或已在其他客户端处理。';
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
    _projectionPaint?.cancel();
    projectionChanges.dispose();
    composerFocus.dispose();
    _epoch++;
    _selection++;
    _paint?.cancel();
    _refresh?.cancel();
    _draftSave?.cancel();
    _listRefresh?.cancel();
    for (final sub in _subscriptions) {
      unawaited(sub.cancel());
    }
    unawaited(_client?.close());
    unawaited(preferences.save().catchError((Object _) {}));
    super.dispose();
  }
}
