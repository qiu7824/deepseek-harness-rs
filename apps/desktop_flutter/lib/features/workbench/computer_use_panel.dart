import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../design/select.dart';
import '../../src/controller.dart' show ComputerUseBinding;
import '../../src/resource_diagnostics.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

const _route = '/__dsh-computer-use';
const _desktopAdapters = {'native-desktop', 'uu-desktop'};
const _readOnlyActions = {'capture', 'list_sessions', 'list_windows'};
const _targetLabels = {
  'local': DshConversationZh.localDesktop,
  'remote': DshConversationZh.boundRemoteDevice,
  'browser': DshConversationZh.isolatedBrowser,
};

/// Why the agent may not act right now, as the Host reports it.
String computerUsePauseText(Json? control, Json? state) {
  final reason =
      control?['pauseReason'] ??
      object(state?['controlDiagnostics'])['pauseReason'];
  return const {
        'local-physical-input': DshConversationZh.takeoverLocalInput,
        'external-injected-input': DshConversationZh.takeoverInjectedInput,
        'escape-hotkey': DshConversationZh.takeoverEmergencyShortcut,
        'gui-input': DshConversationZh.takeoverControlInput,
        'gui-takeover': DshConversationZh.takeoverPanel,
        'start-human': DshConversationZh.takeoverInitialMode,
        'resume-pending': DshConversationZh.takeoverNotReleased,
        'release-failed': DshConversationZh.takeoverInputReleasePending,
        'emergency-stop-unavailable':
            DshConversationZh.takeoverShortcutUnavailable,
        'reader-shutdown': DshConversationZh.controlProcessDisconnected,
        'connection-closing': DshConversationZh.closingControlConnection,
      }['$reason'] ??
      DshConversationZh.takeoverReasonUnknown;
}

/// Human view of the shared Computer Use runtime. It drives the same control
/// session as the model's `computer_use` tool, keyed by conversation and
/// browser session, so a person can watch, take over and hand control back.
class ComputerUsePanel extends StatefulWidget {
  const ComputerUsePanel({
    super.key,
    required this.api,
    required this.session,
    this.binding,
    this.onOpenSettings,
  });
  final DshClient api;
  final String session;

  /// The session the model is driving; the panel attaches instead of
  /// starting another one.
  final ComputerUseBinding? binding;
  final VoidCallback? onOpenSettings;
  @override
  State<ComputerUsePanel> createState() => ComputerUsePanelState();
}

class ComputerUsePanelState extends State<ComputerUsePanel>
    with WidgetsBindingObserver, ResourceDiagnosticScope {
  @override
  String get resourceScopeKind => 'computer-use-view';
  @override
  Map<String, int> get resourceDiagnostics => {
    'computerUsePanels': mounted ? 1 : 0,
    'computerUseFrameBytes': frame?.bytes.length ?? 0,
    'computerUseDisplayedImageSlots': frame != null && frameDecoded ? 1 : 0,
    'computerUseRetiringFrameBytes': retiringFrames.fold(
      0,
      (n, f) => n + f.bytes.length,
    ),
    'computerUsePendingEvictions': retiringFrames.length,
    'computerUseRequests': pendingRequests,
    'computerUsePollRequests': polling ? 1 : 0,
    'computerUsePollTimers': poll?.isActive == true ? 1 : 0,
    'computerUseRetentionTimers': frameRelease?.isActive == true ? 1 : 0,
  };
  RequestScope scope = RequestScope();
  RequestScope pollScope = RequestScope();
  final url = TextEditingController(), typing = TextEditingController();
  final urlFocus = FocusNode(), typingFocus = FocusNode();
  final frameFocus = FocusNode(), windowAnchor = GlobalKey();
  String browserSessionId = 'default', target = 'local';
  bool attachOnly = false, closed = false, disabled = false;
  List<String> sessions = [];
  Json? meta, state, control, selectedWindow;
  MemoryImage? frame;
  bool frameDecoded = false;
  final retiringFrames = <MemoryImage>{};
  int pendingRequests = 0;
  Object? error;
  int foreground = 0, requestSequence = 0, appliedSequence = 0, epoch = 0;
  bool privateInput = false, liveDesktop = true, autoRefresh = false;
  bool panelVisible = true, appVisible = true, polling = false;
  Timer? poll, frameRelease;
  Offset? pressed;
  int? pressedButtons;
  ({Offset at, DateTime time})? lastClick;
  double scrollX = 0, scrollY = 0;
  Offset scrollAt = Offset.zero;
  bool scrolling = false;

  bool get visible => panelVisible && appVisible;
  bool get busy => foreground > 0;
  String? get adapter => meta?['adapter'] as String?;
  bool get desktop => _desktopAdapters.contains(adapter);
  bool get manual => control?['mode'] == 'manual';
  bool get interactive =>
      state != null &&
      state!['interactive'] != false &&
      state!['connected'] != false;
  List<String> get actions =>
      (meta?['actions'] as List? ?? const []).whereType<String>().toList();
  Size? get viewport {
    final value = object(state?['viewport']);
    final width = value['width'], height = value['height'];
    return width is num && height is num && width > 0 && height > 0
        ? Size(width.toDouble(), height.toDouble())
        : null;
  }

  @override
  void initState() {
    super.initState();
    final lifecycle = WidgetsBinding.instance.lifecycleState;
    appVisible = lifecycle == null || lifecycle == AppLifecycleState.resumed;
    WidgetsBinding.instance.addObserver(this);
    typingFocus.addListener(() {
      if (typingFocus.hasFocus) takeover();
    });
    unawaited(bind(widget.binding));
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

  @override
  void didUpdateWidget(covariant ComputerUsePanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    final binding = widget.binding;
    if (widget.api != oldWidget.api || widget.session != oldWidget.session) {
      unawaited(bind(binding));
    } else if (binding != null &&
        binding != oldWidget.binding &&
        (binding.browserSessionId != browserSessionId ||
            binding.target != target)) {
      unawaited(bind(binding));
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    poll?.cancel();
    frameRelease?.cancel();
    scope.cancel();
    pollScope.cancel();
    unawaited(frame?.evict());
    frame = null;
    frameDecoded = false;
    for (final image in retiringFrames) {
      unawaited(image.evict());
    }
    retiringFrames.clear();
    for (final node in [urlFocus, typingFocus, frameFocus]) {
      node.dispose();
    }
    url.dispose();
    typing.dispose();
    super.dispose();
  }

  void updateVisibility({bool? app, bool? panel}) {
    final was = visible;
    appVisible = app ?? appVisible;
    panelVisible = panel ?? panelVisible;
    if (visible == was) return;
    if (visible) {
      frameRelease?.cancel();
      frameRelease = null;
      if (frame == null && state != null && !closed && !busy && !polling) {
        final generation = epoch;
        polling = true;
        unawaited(
          _action('capture', const {}, quiet: true).whenComplete(() {
            if (mounted && generation == epoch) {
              polling = false;
              schedulePoll();
            }
          }),
        );
      }
      schedulePoll();
    } else {
      poll?.cancel();
      pollScope.cancel();
      pollScope = RequestScope();
      frameRelease?.cancel();
      frameRelease = Timer(const Duration(minutes: 2), releaseHiddenFrame);
    }
  }

  void releaseHiddenFrame() {
    frameRelease?.cancel();
    frameRelease = null;
    if (mounted && !visible && frame != null) {
      setState(() => showFrame(null));
      if (!WidgetsBinding.instance.framesEnabled) {
        WidgetsBinding.instance.scheduleWarmUpFrame();
      }
    }
  }

  @override
  void didHaveMemoryPressure() {
    if (!visible) releaseHiddenFrame();
  }

  int resetRequests() {
    epoch++;
    poll?.cancel();
    scope.cancel();
    pollScope.cancel();
    scope = RequestScope();
    pollScope = RequestScope();
    foreground = 0;
    polling = scrolling = false;
    scrollX = scrollY = 0;
    pressed = null;
    pressedButtons = null;
    lastClick = null;
    typing.clear();
    url.clear();
    sessions = [];
    return epoch;
  }

  Future<Json> request(
    String operation,
    Json body, {
    bool quiet = false,
  }) async {
    final generation = epoch, api = widget.api, owner = widget.session;
    final requestScope = quiet ? pollScope : scope;
    pendingRequests++;
    late final Json value;
    try {
      value = await api.request(
        '$_route/$operation',
        body: {'ownerSessionId': owner, ...body},
        scope: requestScope,
        mutation:
            operation == 'action' && !_readOnlyActions.contains(body['action']),
        maxBytes: 24 * 1024 * 1024,
      );
    } finally {
      pendingRequests--;
    }
    if (!mounted ||
        generation != epoch ||
        requestScope.cancelled ||
        api != widget.api ||
        owner != widget.session) {
      throw DshException(
        'cancelled',
        DshConversationZh.controlConnectionChanged,
      );
    }
    return value;
  }

  /// Attach to the model's session when one is known; otherwise reuse an
  /// existing browser session before starting the default control target.
  Future<void> bind(ComputerUseBinding? binding) async {
    // Runs from initState and didUpdateWidget, which build right after.
    final epoch = resetRequests();
    showFrame(null);
    state = control = meta = selectedWindow = null;
    closed = disabled = false;
    error = null;
    if (binding != null) {
      browserSessionId = binding.browserSessionId;
      target = binding.target;
      attachOnly = true;
    } else {
      List<String> names = [];
      try {
        names = await listSessions();
      } catch (_) {
        // Discovery is optional; meta reports a disabled or missing runtime.
      }
      if (!mounted || epoch != this.epoch) return;
      final selected =
          names.where((name) => name != 'default').firstOrNull ??
          names.firstOrNull ??
          'default';
      setState(() {
        sessions = names;
        browserSessionId = selected;
        target = names.isEmpty ? 'local' : 'browser';
        attachOnly = names.contains(selected);
      });
    }
    await connect(epoch);
  }

  Future<List<String>> listSessions() async {
    final value = await request('action', {
      'target': 'browser',
      'action': 'list_sessions',
      'includeScreenshot': false,
    });
    return (value['sessions'] as List? ?? const [])
        .whereType<String>()
        .where((name) => name.isNotEmpty && name.length <= 256)
        .take(64)
        .toList();
  }

  Future<void> refreshSessions() async {
    final generation = epoch;
    try {
      final names = await listSessions();
      if (mounted && generation == epoch) setState(() => sessions = names);
    } catch (failure) {
      if (mounted &&
          generation == epoch &&
          !(failure is DshException && failure.code == 'cancelled')) {
        setState(() => error = describe(failure));
      }
    }
  }

  Future<void> connect(int epoch) async {
    try {
      var value = await request('meta', {'target': target});
      if (!mounted || epoch != this.epoch) return;
      // Single-adapter runtimes ignore the target; routed ones declare the
      // default, which may differ from the local desktop.
      final targets = objects(value['targets']).map((t) => '${t['id']}');
      if (targets.isNotEmpty && !targets.contains(target)) {
        target =
            objects(value['targets'])
                .where((t) => t['default'] == true)
                .map((t) => '${t['id']}')
                .firstOrNull ??
            targets.first;
        value = await request('meta', {'target': target});
        if (!mounted || epoch != this.epoch) return;
      }
      setState(() => meta = value);
      if (value['enabled'] != true) {
        setState(() {
          disabled = true;
          error = DshConversationZh.computerUseDisabled;
        });
        return;
      }
      if (value['available'] == false) {
        setState(
          () => error =
              '${object(value['error'])['message'] ?? DshConversationZh.computerUseAdapterUnavailable}',
        );
        return;
      }
      if (closed) return;
      await action(attachOnly ? 'capture' : 'start');
      if (mounted && epoch == this.epoch) schedulePoll();
    } catch (failure) {
      if (mounted && epoch == this.epoch) {
        setState(() => error = describe(failure));
      }
    }
  }

  DshError describe(Object failure) {
    final value = DshError.describe(failure);
    return DshError(
      title: value.title,
      message: value.message.split('; controlDiagnostics=').first,
      details: value.details,
      code: value.code,
      cancelled: value.cancelled,
      outcomeUnknown: value.outcomeUnknown,
    );
  }

  void showFrame(Uint8List? bytes) {
    final previous = frame;
    frame = bytes == null ? null : MemoryImage(bytes);
    if (bytes == null) frameDecoded = false;
    // Every capture is a new image; keep them out of the shared cache.
    if (previous != null) {
      retiringFrames.add(previous);
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (retiringFrames.remove(previous)) unawaited(previous.evict());
      });
    }
  }

  Future<Json?> action(String name, [Json extra = const {}]) =>
      _action(name, extra, quiet: false);

  Future<Json?> _action(String name, Json extra, {required bool quiet}) async {
    final generation = epoch;
    final requestScope = quiet ? pollScope : scope;
    final sequence = ++requestSequence;
    if (name == 'close') {
      closed = true;
    } else if (name == 'start') {
      closed = false;
      liveDesktop = true;
    }
    if (!quiet) {
      poll?.cancel();
      setState(() {
        foreground++;
        error = null;
      });
    }
    try {
      final value = await request('action', {
        'browserSessionId': browserSessionId,
        'target': target,
        'action': name,
        'includeScreenshot': true,
        ...extra,
      }, quiet: quiet);
      if (!mounted || generation != epoch || requestScope.cancelled) {
        return null;
      }
      if (mounted &&
          generation == epoch &&
          !requestScope.cancelled &&
          sequence >= appliedSequence) {
        appliedSequence = sequence;
        setState(() => apply(name, value));
      }
      return value;
    } catch (failure) {
      final code = failure is DshException
          ? '${failure.details['error'] ?? failure.code}'
          : '';
      if (!mounted ||
          generation != epoch ||
          requestScope.cancelled ||
          code == 'cancelled') {
        return null;
      }
      if (name == 'capture' && code == 'COMPUTER_USE_CAPTURE_INTERRUPTED') {
        return {};
      }
      if (quiet && code == 'COMPUTER_USE_MANUAL_CONTROL') return {};
      if (mounted &&
          generation == epoch &&
          !requestScope.cancelled &&
          sequence >= appliedSequence) {
        appliedSequence = sequence;
        setState(() => fail(name, code, describe(failure)));
      }
      return null;
    } finally {
      if (!quiet && mounted && generation == epoch) {
        setState(() => foreground = math.max(0, foreground - 1));
        schedulePoll();
      }
    }
  }

  void apply(String name, Json value) {
    final next = value['control'];
    if (next is Map) {
      int generation(Json? row) => (row?['generation'] as num?)?.toInt() ?? 0;
      if (control == null || generation(object(next)) >= generation(control)) {
        control = object(next);
      }
      if (name == 'resume_agent') typing.clear();
    }
    if (value['state'] is Map) {
      state = object(value['state']);
      final address = state!['url'];
      if (address is String && !urlFocus.hasFocus) url.text = address;
    }
    final shot = object(value['screenshot'])['base64'];
    // Accepted foreground operations may finish while hidden. Keep their
    // control state, without recreating a frame that was already reclaimed.
    if (shot is String && visible) {
      try {
        showFrame(base64Decode(shot));
      } on FormatException {
        showFrame(null);
      }
    }
    if (name == 'close') {
      state = control = null;
      showFrame(null);
      typing.clear();
    }
  }

  void fail(String name, String code, Object failure) {
    error = failure;
    if (code == 'COMPUTER_USE_EMERGENCY_STOP_UNAVAILABLE') {
      control = {
        ...?control,
        'mode': 'manual',
        'pauseReason': 'emergency-stop-unavailable',
      };
    }
    if (adapter == 'native-desktop' &&
        code.startsWith('COMPUTER_USE_WINDOW_NOT_FOCUSED') &&
        state != null) {
      state = {...state!, 'foreground': false};
    }
    if (desktop && (name == 'start' || name == 'capture')) {
      showFrame(null);
      liveDesktop = false;
      if (state != null) state = {...state!, 'interactive': false};
    }
  }

  /// One capture at a time while visible: native desktops every 500 ms,
  /// UU devices every second, browsers every 3 s when auto refresh is on.
  Duration? get pollInterval {
    if (!visible || closed || busy || state == null) return null;
    // Desktop drivers report their connection; browser state carries none.
    if (desktop) {
      if (!liveDesktop || state!['connected'] != true) return null;
      return Duration(milliseconds: adapter == 'native-desktop' ? 500 : 1000);
    }
    return autoRefresh ? const Duration(seconds: 3) : null;
  }

  void schedulePoll() {
    poll?.cancel();
    final interval = pollInterval;
    if (interval == null || polling) return;
    final generation = epoch;
    poll = Timer(interval, () async {
      if (!mounted || generation != epoch || pollInterval == null) return;
      polling = true;
      try {
        await _action('capture', const {}, quiet: true);
      } finally {
        if (generation == epoch) polling = false;
      }
      if (mounted && generation == epoch) schedulePoll();
    });
  }

  Future<void> reconnect() async {
    final generation = epoch;
    if (meta?['enabled'] != true || meta?['available'] == false) {
      await connect(resetRequests());
      return;
    }
    final window = selectedWindow == null
        ? (state?['windowId'])
        : selectedWindow!['windowId'];
    if (state != null &&
        await action('close', {'includeScreenshot': false}) == null) {
      return;
    }
    if (!mounted || generation != epoch) return;
    await action('start', {
      if (adapter == 'native-desktop' && window is int && window > 0)
        'windowId': window,
    });
  }

  Future<void> switchTarget(String next) async {
    if (next == target || busy) return;
    final generation = epoch;
    if (state != null &&
        await action('close', {'includeScreenshot': false}) == null) {
      return;
    }
    if (!mounted || generation != epoch) return;
    setState(() {
      target = next;
      attachOnly = false;
      closed = false;
      selectedWindow = null;
    });
    await bindExisting(browserSessionId, next, attach: false);
  }

  Future<void> bindExisting(
    String session,
    String nextTarget, {
    required bool attach,
  }) async {
    final epoch = resetRequests();
    setState(() {
      browserSessionId = session;
      target = nextTarget;
      attachOnly = attach;
      closed = false;
      state = control = meta = null;
      showFrame(null);
    });
    await connect(epoch);
  }

  void takeover() {
    if (state != null && !manual) {
      unawaited(action('takeover', {'includeScreenshot': false}));
    }
  }

  void navigate() {
    var address = url.text.trim();
    if (address.isEmpty) return;
    if (!RegExp(r'^https?://', caseSensitive: false).hasMatch(address)) {
      final local = RegExp(
        r'^(?:localhost|127\.\d+\.\d+\.\d+|\[::1\])(?::\d+)?(?:[/?#]|$)',
        caseSensitive: false,
      ).hasMatch(address);
      address = '${local ? 'http' : 'https'}://$address';
    }
    unawaited(action('navigate', {'url': address}));
  }

  Future<void> sendText() async {
    final generation = epoch;
    final text = typing.text;
    if (text.isEmpty || busy || !interactive) return;
    if (text.length > 8192) {
      setState(() => error = DshConversationZh.controlInputLimit);
      return;
    }
    final value = await action('type', {'text': text});
    if (mounted &&
        generation == epoch &&
        value != null &&
        typing.text == text) {
      typing.clear();
    }
  }

  Offset? toViewport(Offset local, Size box) {
    final size = viewport;
    if (size == null || box.isEmpty) return null;
    return Offset(
      (local.dx * size.width / box.width).clamp(0, size.width - 1),
      (local.dy * size.height / box.height).clamp(0, size.height - 1),
    );
  }

  void pointerUp(PointerUpEvent event, Size box) {
    final start = pressed, buttons = pressedButtons;
    pressed = pressedButtons = null;
    if (start == null || busy || !interactive) return;
    final from = toViewport(start, box),
        to = toViewport(event.localPosition, box);
    if (from == null || to == null) return;
    if (buttons == kSecondaryMouseButton) {
      unawaited(
        action('click', {'x': from.dx, 'y': from.dy, 'button': 'right'}),
      );
      return;
    }
    if ((event.localPosition - start).distance > 6) {
      lastClick = null;
      unawaited(
        action('drag', {
          'x': from.dx,
          'y': from.dy,
          'endX': to.dx,
          'endY': to.dy,
        }),
      );
      return;
    }
    final now = DateTime.now(), previous = lastClick;
    final repeated =
        previous != null &&
        now.difference(previous.time) < const Duration(milliseconds: 400) &&
        (previous.at - start).distance <= 6;
    lastClick = repeated ? null : (at: start, time: now);
    unawaited(
      action(repeated ? 'double_click' : 'click', {'x': from.dx, 'y': from.dy}),
    );
  }

  /// Wheel ticks arrive faster than actions complete; send their sum.
  Future<void> wheel(PointerScrollEvent event, Size box) async {
    final at = toViewport(event.localPosition, box);
    if (at == null || !interactive) return;
    scrollX += event.scrollDelta.dx;
    scrollY += event.scrollDelta.dy;
    scrollAt = at;
    if (scrolling) return;
    scrolling = true;
    final generation = epoch;
    try {
      while (mounted && generation == epoch && (scrollX != 0 || scrollY != 0)) {
        final dx = scrollX.clamp(-5000.0, 5000.0).roundToDouble();
        final dy = scrollY.clamp(-5000.0, 5000.0).roundToDouble();
        scrollX = scrollY = 0;
        if (dx == 0 && dy == 0) break;
        final done = await action('scroll', {
          'x': scrollAt.dx,
          'y': scrollAt.dy,
          if (dx != 0) 'deltaX': dx,
          if (dy != 0) 'deltaY': dy,
        });
        if (!mounted || generation != epoch) return;
        if (done == null) {
          scrollX = scrollY = 0;
          break;
        }
      }
    } finally {
      if (generation == epoch) scrolling = false;
    }
  }

  Future<void> chooseWindow() async {
    final generation = epoch;
    final value = await action('list_windows', {'includeScreenshot': false});
    if (!mounted || value == null) return;
    final rows = objects(value['windows'])
        .where((row) => row['windowId'] is int && (row['windowId'] as int) > 0)
        .where((row) => row['title'] is String)
        .take(256)
        .toList();
    final anchor = windowAnchor.currentContext?.findRenderObject();
    final overlay =
        Overlay.of(context).context.findRenderObject()! as RenderBox;
    final origin = anchor is RenderBox
        ? anchor.localToGlobal(Offset(0, anchor.size.height), ancestor: overlay)
        : Offset.zero;
    final current = state?['windowId'];
    final picked = await showMenu<String>(
      context: context,
      position: RelativeRect.fromRect(
        Rect.fromLTWH(origin.dx, origin.dy, 0, 0),
        Offset.zero & overlay.size,
      ),
      constraints: const BoxConstraints(minWidth: 240, maxWidth: 420),
      items: [
        const PopupMenuItem(
          value: 'desktop',
          height: 34,
          child: Text(DshConversationZh.primaryMonitor),
        ),
        for (final row in rows)
          PopupMenuItem(
            value: '${row['windowId']}',
            height: 34,
            child: Text(
              '${(row['title'] as String).characters.take(512)}'
              '${row['windowId'] == current ? DshConversationZh.currentSuffix : ''}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
        if (rows.isEmpty)
          const PopupMenuItem(
            enabled: false,
            height: 34,
            child: Text(DshConversationZh.noAvailableWindows),
          ),
      ],
    );
    if (!mounted || generation != epoch || picked == null) return;
    setState(
      () => selectedWindow = picked == 'desktop'
          ? {'windowId': null, 'title': DshConversationZh.primaryMonitor}
          : rows.firstWhere((row) => '${row['windowId']}' == picked),
    );
  }

  /// Explicitly closes a control session started by this view. Hiding or closing
  /// the view never calls this action; attached sessions stay with their owner.
  void endSession() {
    if (attachOnly || closed || meta?['enabled'] != true) return;
    unawaited(
      widget.api
          .request(
            '$_route/action',
            body: {
              'ownerSessionId': widget.session,
              'browserSessionId': browserSessionId,
              'target': target,
              'action': 'close',
              'includeScreenshot': false,
            },
            mutation: true,
          )
          .then<void>((_) {}, onError: (_) {}),
    );
  }

  Json get diagnostic => {
    'adapter': adapter,
    'phase': state?['phase'] ?? (state == null ? 'idle' : 'unknown'),
    'connected': state?['connected'] == true,
    'interactive': state?['interactive'] != false,
    'control': control,
    'viewport': state?['viewport'],
    'windowId': state?['windowId'],
    'foreground': state?['foreground'],
    'frame': {'available': frame != null, 'polling': pollInterval != null},
    'error': error == null ? null : DshError.describe(error!).details,
    'capabilities': actions,
  };

  String get statusText => !interactive
      ? (state?['connected']) == false
            ? DshConversationZh.disconnected
            : state != null
            ? DshConversationZh.awaitingFrame
            : DshConversationZh.notConnected
      : manual
      ? DshConversationZh.humanControlStatus
      : DshConversationZh.agentControlReady;

  Widget button(
    String label,
    VoidCallback? onPressed, {
    Key? key,
    bool active = false,
  }) => DshButton(
    key: key,
    height: 28,
    outline: true,
    active: active,
    fontSize: DshTypography.sizeCaption,
    padding: const EdgeInsets.symmetric(horizontal: 10),
    onPressed: onPressed,
    child: Text(label),
  );

  Widget banner(String text) {
    final colors = DshColors(context);
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: colors.layer,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              text,
              style: TextStyle(
                fontSize: DshTypography.sizeBody,
                color: colors.muted,
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// The frame fills the panel width at the page's aspect ratio, up to
  /// [maxHeight]; the rest of the panel scrolls.
  Widget framePane(double maxHeight) {
    final colors = DshColors(context);
    final image = frame, size = viewport;
    if (image == null || size == null) {
      return SizedBox(
        height: math.min(200, maxHeight),
        child: Center(
          child: Text(
            closed
                ? DshConversationZh.controlConnectionClosedHint
                : error != null
                ? DshConversationZh.frameUnavailable
                : DshConversationZh.connectingAndFetchingFrame,
            style: TextStyle(
              fontSize: DshTypography.sizeCaption,
              color: colors.muted,
            ),
          ),
        ),
      );
    }
    return LayoutBuilder(
      builder: (context, constraints) {
        final scale = math.min(
          constraints.maxWidth / size.width,
          maxHeight / size.height,
        );
        final box = Size(size.width * scale, size.height * scale);
        return Center(
          child: Focus(
            focusNode: frameFocus,
            onKeyEvent: (_, event) {
              if (event is KeyDownEvent &&
                  event.logicalKey == LogicalKeyboardKey.escape) {
                takeover();
                return KeyEventResult.handled;
              }
              return KeyEventResult.ignored;
            },
            child: Semantics(
              label: DshConversationZh.controlFrameHint,
              child: MouseRegion(
                cursor: interactive && !busy
                    ? SystemMouseCursors.precise
                    : SystemMouseCursors.basic,
                child: Listener(
                  onPointerDown: (event) {
                    frameFocus.requestFocus();
                    pressed = event.localPosition;
                    pressedButtons = event.buttons;
                  },
                  onPointerUp: (event) => pointerUp(event, box),
                  onPointerCancel: (_) => pressed = pressedButtons = null,
                  // Claim the wheel before the panel's scroll view does.
                  onPointerSignal: (event) {
                    if (event is PointerScrollEvent && interactive) {
                      GestureBinding.instance.pointerSignalResolver.register(
                        event,
                        (event) =>
                            unawaited(wheel(event as PointerScrollEvent, box)),
                      );
                    }
                  },
                  child: SizedBox.fromSize(
                    key: const ValueKey('computer-use-frame'),
                    size: box,
                    child: Image(
                      image: image,
                      fit: BoxFit.fill,
                      gaplessPlayback: true,
                      filterQuality: FilterQuality.medium,
                      frameBuilder: (_, child, number, _) {
                        if (number != null) frameDecoded = true;
                        return child;
                      },
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  Widget diagnostics() {
    final colors = DshColors(context);
    final entries = {
      DshConversationZh.connect: state?['connected'] == true
          ? DshConversationZh.connected
          : state != null
          ? DshConversationZh.connecting
          : DshConversationZh.notConnected,
      DshConversationZh.adapter: adapter ?? DshConversationZh.unselected,
      DshConversationZh.controlOwnership: manual
          ? DshConversationZh.humanTakeover
          : control?['mode'] == 'agent'
          ? DshConversationZh.agent
          : DshConversationZh.notEstablished,
      DshConversationZh.frame: viewport == null
          ? DshConversationZh.notReceived
          : '${viewport!.width.round()}×${viewport!.height.round()}',
      DshConversationZh.connectionPhase:
          '${state?['phase'] ?? (state == null ? 'idle' : 'unknown')}',
      DshConversationZh.interactionState: state?['interactive'] == false
          ? DshConversationZh.paused
          : state?['connected'] == true
          ? DshConversationZh.controllable
          : DshConversationZh.notControllable,
      DshConversationZh.windowFocus: state?['foreground'] == true
          ? DshConversationZh.foreground
          : state?['foreground'] == false
          ? DshConversationZh.background
          : DshConversationZh.notApplicable,
      DshConversationZh.pauseReason: computerUsePauseText(control, state),
      DshConversationZh.controlGeneration: control?['generation'] is int
          ? '${control!['generation']}'
          : DshConversationZh.unavailable,
      DshConversationZh.emergencyShortcut:
          '${state?['emergencyStopShortcut'] ?? DshConversationZh.unavailable}',
      DshConversationZh.capabilities: actions.isEmpty
          ? DshConversationZh.undeclared
          : actions.join('、'),
    };
    // The workbench paints an opaque ColoredBox; the tile needs its own
    // Material to show its ink.
    return Material(
      type: MaterialType.transparency,
      child: Theme(
        data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
        child: ExpansionTile(
          key: const ValueKey('computer-use-diagnostics'),
          dense: true,
          tilePadding: EdgeInsets.zero,
          childrenPadding: const EdgeInsets.only(bottom: 8),
          title: Text(
            DshConversationZh.runtimeDiagnostics(
              state: error != null
                  ? DshConversationZh.error
                  : interactive
                  ? DshConversationZh.normal
                  : state != null
                  ? DshConversationZh.waiting
                  : DshConversationZh.notConnected,
            ),
            style: const TextStyle(fontSize: DshTypography.sizeCaption),
          ),
          children: [
            for (final entry in entries.entries)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 2),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    SizedBox(
                      width: 72,
                      child: Text(
                        entry.key,
                        style: TextStyle(
                          fontSize: DshTypography.sizeCaption,
                          color: colors.muted,
                        ),
                      ),
                    ),
                    Expanded(
                      child: SelectableText(
                        entry.value,
                        style: const TextStyle(
                          fontSize: DshTypography.sizeCaption,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            Align(
              alignment: Alignment.centerRight,
              child: button(
                DshConversationZh.copyDiagnostics,
                () => Clipboard.setData(
                  ClipboardData(
                    text: DshError.redact(
                      const JsonEncoder.withIndent('  ').convert(diagnostic),
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final targets = objects(meta?['targets'])
        .map((t) => '${t['id']}')
        .where(_targetLabels.containsKey)
        .toList();
    final nativeDesktop = adapter == 'native-desktop';
    final window = selectedWindow;
    return LayoutBuilder(
      builder: (context, panel) => ListView(
        padding: const EdgeInsets.all(10),
        children: [
          if (targets.length > 1 || sessions.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Wrap(
                spacing: 6,
                runSpacing: 6,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  if (targets.length > 1)
                    DshSelect<String>(
                      key: const ValueKey('computer-use-target'),
                      maxWidth: 220,
                      outline: true,
                      value: target,
                      options: {
                        for (final id in targets) id: _targetLabels[id]!,
                      },
                      onChanged: busy ? null : (value) => switchTarget(value),
                    ),
                  if (sessions.isNotEmpty && target == 'browser')
                    DshSelect<String>(
                      key: const ValueKey('computer-use-browser-session'),
                      maxWidth: 200,
                      outline: true,
                      value: browserSessionId,
                      options: {
                        for (final name in {browserSessionId, ...sessions})
                          name: DshConversationZh.browserSession(name: name),
                      },
                      onChanged: busy
                          ? null
                          : (value) =>
                                bindExisting(value, 'browser', attach: true),
                    ),
                  if (sessions.isNotEmpty || target == 'browser')
                    button(
                      DshConversationZh.refreshBrowserSessions,
                      busy ? null : refreshSessions,
                    ),
                ],
              ),
            ),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              button(
                state == null
                    ? DshConversationZh.connect
                    : DshConversationZh.reconnect,
                busy ? null : reconnect,
                key: const ValueKey('computer-use-connect'),
              ),
              button(
                manual
                    ? DshConversationZh.returnControlToAgent
                    : DshConversationZh.humanTakeover,
                busy || state == null
                    ? null
                    : () => action(manual ? 'resume_agent' : 'takeover', {
                        'includeScreenshot': false,
                      }),
                key: const ValueKey('computer-use-control'),
                active: manual,
              ),
              if (nativeDesktop && actions.contains('list_windows'))
                KeyedSubtree(
                  key: windowAnchor,
                  child: button(
                    DshConversationZh.selectWindow,
                    busy || state?['connected'] != true ? null : chooseWindow,
                  ),
                ),
              if (nativeDesktop &&
                  state?['windowId'] != null &&
                  actions.contains('focus_window'))
                button(
                  DshConversationZh.activateWindow,
                  busy || state?['connected'] != true
                      ? null
                      : () => action('focus_window'),
                ),
              if (desktop)
                button(
                  DshConversationZh.autoRefresh,
                  state?['connected'] != true
                      ? null
                      : () {
                          setState(() => liveDesktop = !liveDesktop);
                          schedulePoll();
                        },
                  active: liveDesktop,
                )
              else
                button(DshConversationZh.autoRefresh, () {
                  setState(() => autoRefresh = !autoRefresh);
                  schedulePoll();
                }, active: autoRefresh),
              button(
                DshConversationZh.refreshFrame,
                busy ? null : () => action('capture'),
              ),
              button(
                DshConversationZh.closeControlSession,
                busy || state == null
                    ? null
                    : () => action('close', {'includeScreenshot': false}),
                key: const ValueKey('computer-use-close'),
              ),
              Text(
                statusText,
                key: const ValueKey('computer-use-status'),
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: colors.muted,
                ),
              ),
            ],
          ),
          if (nativeDesktop &&
              window != null &&
              window['windowId'] != state?['windowId'])
            banner(DshConversationZh.windowSelected(title: window['title'])),
          if (!desktop)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Row(
                children: [
                  button(
                    DshConversationZh.back,
                    busy || state == null
                        ? null
                        : () => action('click', {
                            'x': 0,
                            'y': 0,
                            'button': 'back',
                          }),
                  ),
                  const SizedBox(width: 6),
                  Expanded(
                    child: DshField(
                      key: const ValueKey('computer-use-url'),
                      controller: url,
                      focusNode: urlFocus,
                      hint: DshConversationZh.controlledBrowserAddress,
                      onSubmitted: (_) => navigate(),
                    ),
                  ),
                  const SizedBox(width: 6),
                  button(DshConversationZh.navigate, busy ? null : navigate),
                ],
              ),
            ),
          if (desktop)
            banner(
              adapter == 'uu-desktop'
                  ? DshConversationZh.remoteEmergencyHint(
                      shortcut:
                          state?['emergencyStopShortcut'] ??
                          DshConversationZh.unconfirmed,
                    )
                  : DshConversationZh.localDesktopTakeoverHint,
            ),
          if (manual)
            banner(
              DshConversationZh.controlPaused(
                reason: computerUsePauseText(control, state),
              ),
            ),
          if (error != null) DshErrorView(error: error!),
          if (error != null && disabled && widget.onOpenSettings != null)
            button(DshConversationZh.openSettings, widget.onOpenSettings),
          const SizedBox(height: 8),
          DecoratedBox(
            decoration: BoxDecoration(
              color: colors.layer,
              borderRadius: BorderRadius.circular(8),
            ),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(8),
              child: framePane(math.max(200, panel.maxHeight * .7)),
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: DshField(
                  key: const ValueKey('computer-use-typing'),
                  controller: typing,
                  focusNode: typingFocus,
                  secret: privateInput,
                  hint: DshConversationZh.typeIntoFocusedTarget,
                  onSubmitted: (_) => sendText(),
                ),
              ),
              const SizedBox(width: 6),
              button(
                DshConversationZh.input,
                busy || !interactive ? null : sendText,
              ),
              const SizedBox(width: 6),
              button(DshConversationZh.privateInput, () {
                setState(() => privateInput = !privateInput);
                takeover();
              }, active: privateInput),
            ],
          ),
          const SizedBox(height: 6),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              for (final key in ['Enter', 'Tab', 'Backspace', 'Escape'])
                button(
                  key,
                  busy || !interactive
                      ? null
                      : () => action('key', {
                          'keys': [key],
                        }),
                ),
              button(
                DshConversationZh.scrollUp,
                busy || !interactive
                    ? null
                    : () => action('scroll', {'deltaY': -540}),
              ),
              button(
                DshConversationZh.scrollDown,
                busy || !interactive
                    ? null
                    : () => action('scroll', {'deltaY': 540}),
              ),
              Text(
                DshConversationZh.controlTargetSummary(
                  adapter: adapter ?? DshConversationZh.notConnected,
                  target: state == null
                      ? DshConversationZh.namedSession(id: browserSessionId)
                      : '${state!['targetTitle'] ?? state!['title'] ?? state!['url'] ?? ''}${viewport == null ? '' : ' · ${viewport!.width.round()}×${viewport!.height.round()}'}',
                ),
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: colors.muted,
                ),
              ),
            ],
          ),
          diagnostics(),
        ],
      ),
    );
  }
}
