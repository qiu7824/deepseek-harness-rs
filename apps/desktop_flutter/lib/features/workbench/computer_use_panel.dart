import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../src/controller.dart' show ComputerUseBinding;

const _route = '/__dsh-computer-use';
const _desktopAdapters = {'native-desktop', 'uu-desktop'};
const _readOnlyActions = {'capture', 'list_sessions', 'list_windows'};
const _targetLabels = {
  'local': '本机桌面',
  'remote': '已绑定的 UU 远程设备',
  'browser': '隔离浏览器',
};

/// Why the agent may not act right now, as the Host reports it.
String computerUsePauseText(Json? control, Json? state) {
  final reason =
      control?['pauseReason'] ??
      object(state?['controlDiagnostics'])['pauseReason'];
  return const {
        'local-physical-input': '检测到本机键鼠输入（包括其他窗口）',
        'external-injected-input': '检测到其他程序注入键鼠输入',
        'escape-hotkey': '已触发全局急停快捷键',
        'gui-input': '控制画面收到人工输入',
        'gui-takeover': '已在控制面板选择人工接管',
        'start-human': '连接以人工控制模式启动',
        'resume-pending': '控制权尚未交还',
        'release-failed': '键鼠释放未完成',
        'emergency-stop-unavailable': '全局急停快捷键不可用',
        'reader-shutdown': '控制进程已断开',
        'connection-closing': '控制连接正在关闭',
      }['$reason'] ??
      '暂停来源尚未确认';
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
    with WidgetsBindingObserver {
  final scope = RequestScope();
  RequestScope pollScope = RequestScope();
  final url = TextEditingController(), typing = TextEditingController();
  final urlFocus = FocusNode(), typingFocus = FocusNode();
  final frameFocus = FocusNode(), windowAnchor = GlobalKey();
  String browserSessionId = 'default', target = 'local';
  bool attachOnly = false, closed = false, disabled = false;
  List<String> sessions = [];
  Json? meta, state, control, selectedWindow;
  MemoryImage? frame;
  String? error;
  int foreground = 0, requestSequence = 0, appliedSequence = 0, epoch = 0;
  bool privateInput = false, liveDesktop = true, autoRefresh = false;
  bool panelVisible = true, appVisible = true, polling = false;
  Timer? poll;
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
    if (binding != null &&
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
    scope.cancel();
    pollScope.cancel();
    frame?.evict();
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
      schedulePoll();
    } else {
      poll?.cancel();
      pollScope.cancel();
      pollScope = RequestScope();
    }
  }

  Future<Json> request(String operation, Json body, {bool quiet = false}) =>
      widget.api.request(
        '$_route/$operation',
        body: {'ownerSessionId': widget.session, ...body},
        scope: quiet ? pollScope : scope,
        mutation:
            operation == 'action' && !_readOnlyActions.contains(body['action']),
        maxBytes: 24 * 1024 * 1024,
      );

  /// Attach to the model's session when one is known; otherwise reuse an
  /// existing browser session before starting the default control target.
  Future<void> bind(ComputerUseBinding? binding) async {
    // Runs from initState and didUpdateWidget, which build right after.
    final epoch = ++this.epoch;
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
    try {
      final names = await listSessions();
      if (mounted) setState(() => sessions = names);
    } catch (failure) {
      if (mounted) setState(() => error = describe(failure));
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
          error = 'Computer Use 未启用：在“设置 → 目录与运行环境”中开启后重启本机服务。';
        });
        return;
      }
      if (value['available'] == false) {
        setState(
          () => error =
              '${object(value['error'])['message'] ?? 'Computer Use 执行器当前不可用，请检查浏览器或外部命令设置'}',
        );
        return;
      }
      if (closed) return;
      await action(attachOnly ? 'capture' : 'start');
      schedulePoll();
    } catch (failure) {
      if (mounted && epoch == this.epoch) {
        setState(() => error = describe(failure));
      }
    }
  }

  String describe(Object failure) => failure is DshException
      ? failure.message.split('; controlDiagnostics=').first
      : '$failure';

  void showFrame(Uint8List? bytes) {
    final previous = frame;
    frame = bytes == null ? null : MemoryImage(bytes);
    // Every capture is a new image; keep them out of the shared cache.
    if (previous != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) => previous.evict());
    }
  }

  Future<Json?> action(String name, [Json extra = const {}]) =>
      _action(name, extra, quiet: false);

  Future<Json?> _action(String name, Json extra, {required bool quiet}) async {
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
      if (mounted && sequence >= appliedSequence) {
        appliedSequence = sequence;
        setState(() => apply(name, value));
      }
      return value;
    } catch (failure) {
      final code = failure is DshException
          ? '${failure.details['error'] ?? failure.code}'
          : '';
      if (code == 'cancelled') return null;
      if (name == 'capture' && code == 'COMPUTER_USE_CAPTURE_INTERRUPTED') {
        return {};
      }
      if (quiet && code == 'COMPUTER_USE_MANUAL_CONTROL') return {};
      if (mounted && sequence >= appliedSequence) {
        appliedSequence = sequence;
        setState(() => fail(name, code, describe(failure)));
      }
      return null;
    } finally {
      if (!quiet && mounted) {
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
    if (shot is String) {
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

  void fail(String name, String code, String message) {
    error = message;
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
    poll = Timer(interval, () async {
      if (!mounted || pollInterval == null) return;
      polling = true;
      try {
        await _action('capture', const {}, quiet: true);
      } finally {
        polling = false;
      }
      if (mounted) schedulePoll();
    });
  }

  Future<void> reconnect() async {
    if (meta?['enabled'] != true || meta?['available'] == false) {
      await connect(++epoch);
      return;
    }
    final window = selectedWindow == null
        ? (state?['windowId'])
        : selectedWindow!['windowId'];
    if (state != null &&
        await action('close', {'includeScreenshot': false}) == null) {
      return;
    }
    await action('start', {
      if (adapter == 'native-desktop' && window is int && window > 0)
        'windowId': window,
    });
  }

  Future<void> switchTarget(String next) async {
    if (next == target || busy) return;
    if (state != null &&
        await action('close', {'includeScreenshot': false}) == null) {
      return;
    }
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
    final epoch = ++this.epoch;
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
    final text = typing.text;
    if (text.isEmpty || busy || !interactive) return;
    if (text.length > 8192) {
      setState(() => error = '单次输入最多 8192 个字符');
      return;
    }
    final value = await action('type', {'text': text});
    if (value != null && typing.text == text) typing.clear();
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
    try {
      while (mounted && (scrollX != 0 || scrollY != 0)) {
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
        if (done == null) {
          scrollX = scrollY = 0;
          break;
        }
      }
    } finally {
      scrolling = false;
    }
  }

  Future<void> chooseWindow() async {
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
        const PopupMenuItem(value: 'desktop', height: 34, child: Text('主显示器')),
        for (final row in rows)
          PopupMenuItem(
            value: '${row['windowId']}',
            height: 34,
            child: Text(
              '${(row['title'] as String).characters.take(512)}'
              '${row['windowId'] == current ? ' · 当前' : ''}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
            ),
          ),
        if (rows.isEmpty)
          const PopupMenuItem(
            enabled: false,
            height: 34,
            child: Text('没有可用窗口'),
          ),
      ],
    );
    if (!mounted || picked == null) return;
    setState(
      () => selectedWindow = picked == 'desktop'
          ? {'windowId': null, 'title': '主显示器'}
          : rows.firstWhere((row) => '${row['windowId']}' == picked),
    );
  }

  /// Closing the tab ends a session this view started. A session attached
  /// from model activity or an existing browser stays with its owner.
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
    'error': error,
    'capabilities': actions,
  };

  String get statusText => !interactive
      ? (state?['connected']) == false
            ? '连接已断开'
            : state != null
            ? '等待画面'
            : '未连接'
      : manual
      ? '人工接管中 · 智能体控制暂停'
      : '智能体可操作';

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
    fontSize: 12,
    padding: const EdgeInsets.symmetric(horizontal: 10),
    onPressed: onPressed,
    child: Text(label),
  );

  Widget banner(String text, {bool danger = false, Widget? action}) {
    final colors = DshColors(context);
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: danger ? Colors.red.withValues(alpha: .08) : colors.layer,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              text,
              style: TextStyle(
                fontSize: 12,
                color: danger ? Colors.red.shade700 : colors.muted,
              ),
            ),
          ),
          ?action,
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
                ? '连接已关闭。点击“连接”重新打开。'
                : error != null
                ? '暂时无法显示画面'
                : '正在连接并获取画面…',
            style: TextStyle(fontSize: 12, color: colors.muted),
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
              label: '控制画面，操作即接管；画面聚焦时 Esc 暂停智能体',
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
      '连接': state?['connected'] == true
          ? '已连接'
          : state != null
          ? '连接中'
          : '未连接',
      '适配器': adapter ?? '未选择',
      '控制权': manual
          ? '人工接管'
          : control?['mode'] == 'agent'
          ? '智能体'
          : '未建立',
      '画面': viewport == null
          ? '未收到'
          : '${viewport!.width.round()}×${viewport!.height.round()}',
      '连接阶段': '${state?['phase'] ?? (state == null ? 'idle' : 'unknown')}',
      '交互状态': state?['interactive'] == false
          ? '已暂停'
          : state?['connected'] == true
          ? '可操作'
          : '不可操作',
      '窗口焦点': state?['foreground'] == true
          ? '前台'
          : state?['foreground'] == false
          ? '后台'
          : '不适用',
      '暂停原因': computerUsePauseText(control, state),
      '控制代次': control?['generation'] is int
          ? '${control!['generation']}'
          : '未提供',
      '急停快捷键': '${state?['emergencyStopShortcut'] ?? '未提供'}',
      '能力': actions.isEmpty ? '未声明' : actions.join('、'),
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
            '运行诊断 · ${error != null
                ? '错误'
                : interactive
                ? '正常'
                : state != null
                ? '等待'
                : '未连接'}',
            style: const TextStyle(fontSize: 12),
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
                        style: TextStyle(fontSize: 11, color: colors.muted),
                      ),
                    ),
                    Expanded(
                      child: SelectableText(
                        entry.value,
                        style: const TextStyle(fontSize: 11),
                      ),
                    ),
                  ],
                ),
              ),
            Align(
              alignment: Alignment.centerRight,
              child: button(
                '复制诊断',
                () => Clipboard.setData(
                  ClipboardData(
                    text: const JsonEncoder.withIndent('  ')
                        .convert(diagnostic),
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
                          name: '浏览器会话 $name',
                      },
                      onChanged: busy
                          ? null
                          : (value) =>
                                bindExisting(value, 'browser', attach: true),
                    ),
                  if (sessions.isNotEmpty || target == 'browser')
                    button('刷新浏览器会话', busy ? null : refreshSessions),
                ],
              ),
            ),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              button(
                state == null ? '连接' : '重新连接',
                busy ? null : reconnect,
                key: const ValueKey('computer-use-connect'),
              ),
              button(
                manual ? '交还智能体' : '人工接管',
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
                    '选择窗口',
                    busy || state?['connected'] != true ? null : chooseWindow,
                  ),
                ),
              if (nativeDesktop &&
                  state?['windowId'] != null &&
                  actions.contains('focus_window'))
                button(
                  '激活窗口',
                  busy || state?['connected'] != true
                      ? null
                      : () => action('focus_window'),
                ),
              if (desktop)
                button(
                  '自动刷新',
                  state?['connected'] != true
                      ? null
                      : () {
                          setState(() => liveDesktop = !liveDesktop);
                          schedulePoll();
                        },
                  active: liveDesktop,
                )
              else
                button('自动刷新', () {
                  setState(() => autoRefresh = !autoRefresh);
                  schedulePoll();
                }, active: autoRefresh),
              button('刷新画面', busy ? null : () => action('capture')),
              button(
                '关闭会话',
                busy || state == null
                    ? null
                    : () => action('close', {'includeScreenshot': false}),
                key: const ValueKey('computer-use-close'),
              ),
              Text(
                statusText,
                key: const ValueKey('computer-use-status'),
                style: TextStyle(fontSize: 12, color: colors.muted),
              ),
            ],
          ),
          if (nativeDesktop &&
              window != null &&
              window['windowId'] != state?['windowId'])
            banner('已选择：${window['title']} · 重新连接后切换'),
          if (!desktop)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Row(
                children: [
                  button(
                    '后退',
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
                      hint: '受控浏览器地址',
                      onSubmitted: (_) => navigate(),
                    ),
                  ),
                  const SizedBox(width: 6),
                  button('转到', busy ? null : navigate),
                ],
              ),
            ),
          if (desktop)
            banner(
              adapter == 'uu-desktop'
                  ? 'UU 远程 · 全局急停 ${state?['emergencyStopShortcut'] ?? '尚未确认'}；画面聚焦时 Esc 暂停智能体。'
                  : '本机桌面 · 与本机共用键鼠；操作其他窗口也会暂停智能体，避免争抢鼠标或将内容输入错误窗口。',
            ),
          if (manual)
            banner('智能体控制暂停 · ${computerUsePauseText(control, state)}'),
          if (error != null)
            banner(
              error!,
              danger: true,
              action: disabled && widget.onOpenSettings != null
                  ? button('打开设置', widget.onOpenSettings)
                  : null,
            ),
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
                  hint: '输入到当前焦点',
                  onSubmitted: (_) => sendText(),
                ),
              ),
              const SizedBox(width: 6),
              button('输入', busy || !interactive ? null : sendText),
              const SizedBox(width: 6),
              button('私密输入', () {
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
                '向上',
                busy || !interactive
                    ? null
                    : () => action('scroll', {'deltaY': -540}),
              ),
              button(
                '向下',
                busy || !interactive
                    ? null
                    : () => action('scroll', {'deltaY': 540}),
              ),
              Text(
                '${adapter ?? '未连接'} · ${state == null ? '会话 $browserSessionId' : '${state!['targetTitle'] ?? state!['title'] ?? state!['url'] ?? ''}${viewport == null ? '' : ' · ${viewport!.width.round()}×${viewport!.height.round()}'}'}',
                style: TextStyle(fontSize: 11, color: colors.muted),
              ),
            ],
          ),
          diagnostics(),
        ],
      ),
    );
  }
}
