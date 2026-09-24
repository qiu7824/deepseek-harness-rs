import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../design/primitives.dart';

/// Native Windows dictation bridge. It uses the platform recognizer through a
/// MethodChannel, so the desktop client never embeds a browser runtime.
class VoiceInputController extends ChangeNotifier {
  static const _channel = MethodChannel('dsh/voice');
  static int _next = 0;
  Timer? _timer, _limit;
  String? _generation;
  bool _disposed = false;
  String phase = 'idle';
  String latestText = '', _committed = '';
  int textRevision = 0;
  String? error;
  bool get supported => defaultTargetPlatform == TargetPlatform.windows;
  bool get listening => phase == 'listening';
  bool get active => phase != 'idle';
  void _emit() {
    if (!_disposed) notifyListeners();
  }

  Future<void> start(String initialText) async {
    if (!supported || active || _disposed) return;
    final token = _generation =
        '${DateTime.now().microsecondsSinceEpoch}-${++_next}';
    error = null;
    _committed = latestText = initialText;
    phase = 'starting';
    _emit();
    try {
      await _channel.invokeMethod<void>('start', token);
      if (_disposed || _generation != token) return;
      _limit = Timer(const Duration(minutes: 2), stop);
      _schedule(token);
    } catch (e) {
      if (_disposed || _generation != token) return;
      error = e is MissingPluginException
          ? '此客户端未加载语音组件，请重新启动最新版本。'
          : e is PlatformException && e.code == 'voice-busy'
          ? '语音识别正在释放资源，请稍后重试。'
          : '无法启动语音识别：$e';
      phase = 'idle';
      _generation = null;
      _emit();
    }
  }

  void _schedule(String token) {
    _timer?.cancel();
    if (!_disposed && _generation == token) {
      _timer = Timer(const Duration(milliseconds: 50), () => _poll(token));
    }
  }

  Future<void> _poll(String token) async {
    try {
      final events = await _channel.invokeListMethod<dynamic>('poll') ?? [];
      if (_disposed || _generation != token) return;
      var changed = false;
      for (final value in events) {
        if (value is! Map || value['generation'] != token) continue;
        switch (value['kind']) {
          case 'ready':
            if (phase == 'starting') {
              phase = 'listening';
              changed = true;
            }
          case 'result':
          case 'partial':
            if (phase != 'listening' &&
                !(phase == 'stopping' && value['kind'] == 'result')) {
              continue;
            }
            final text = '${value['text'] ?? ''}';
            if (text.isEmpty) continue;
            final next =
                '${_committed.trimRight()}${_committed.trim().isEmpty ? '' : ' '}$text';
            if (next.length > 65536) {
              error = '语音文本已达到长度上限，请结束后分段输入。';
              unawaited(stop());
              continue;
            }
            latestText = next;
            textRevision++;
            if (value['kind'] == 'result') _committed = next;
            changed = true;
          case 'error':
            error = 'Windows 语音识别不可用，请检查麦克风和语音语言包（${value['text']}）。';
            phase = 'stopping';
            changed = true;
          case 'stopped':
            phase = 'idle';
            _generation = null;
            _limit?.cancel();
            changed = true;
        }
      }
      if (changed) _emit();
      _schedule(token);
    } catch (e) {
      if (_disposed || _generation != token) return;
      error = '读取语音识别状态失败：$e';
      cancel();
    }
  }

  Future<void> stop() async {
    if (_disposed || !active || phase == 'stopping') return;
    final token = _generation;
    phase = 'stopping';
    _limit?.cancel();
    _emit();
    try {
      await _channel.invokeMethod<void>('stop');
    } catch (e) {
      if (!_disposed && _generation == token) {
        error = '停止语音识别失败：$e';
        cancel();
      }
    }
  }

  void cancel({bool notify = true}) {
    final wasActive = active;
    _generation = null;
    phase = 'idle';
    _timer?.cancel();
    _limit?.cancel();
    if (wasActive) {
      unawaited(_channel.invokeMethod<void>('stop').catchError((Object _) {}));
    }
    if (notify && wasActive) _emit();
  }

  @override
  void dispose() {
    _disposed = true;
    cancel(notify: false);
    super.dispose();
  }
}

class VoiceInputButton extends StatefulWidget {
  const VoiceInputButton({
    super.key,
    required this.listening,
    this.starting = false,
    this.stopping = false,
    required this.supported,
    required this.onStart,
    required this.onStop,
    this.error,
  });
  final bool listening, supported, starting, stopping;
  final String? error;
  final VoidCallback? onStart;
  final VoidCallback onStop;

  @override
  State<VoiceInputButton> createState() => _VoiceInputButtonState();
}

class _VoiceInputButtonState extends State<VoiceInputButton> {
  final focus = FocusNode();
  int? pointer;
  bool held = false, focused = false;
  bool get enabled =>
      widget.supported && widget.onStart != null && !widget.stopping;
  void begin() {
    if (!enabled || held || widget.listening || widget.starting) return;
    held = true;
    widget.onStart!();
  }

  void end() {
    pointer = null;
    if (!held && !widget.listening && !widget.starting) return;
    held = false;
    widget.onStop();
  }

  @override
  void dispose() {
    focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final label = !widget.supported
        ? '当前系统不支持语音识别'
        : widget.error ??
              (widget.starting
                  ? '正在启动语音识别'
                  : widget.stopping
                  ? '正在停止语音识别'
                  : widget.listening
                  ? '松开结束，识别文字实时写入'
                  : '按住说话；空格键也可按住输入');
    return Tooltip(
      message: label,
      child: Focus(
        focusNode: focus,
        canRequestFocus: enabled,
        onFocusChange: (value) {
          if (!value) end();
          if (mounted) setState(() => focused = value);
        },
        onKeyEvent: (_, event) {
          final key = event.logicalKey;
          if (key == LogicalKeyboardKey.escape) {
            end();
            return KeyEventResult.handled;
          }
          if (key != LogicalKeyboardKey.space &&
              key != LogicalKeyboardKey.enter) {
            return KeyEventResult.ignored;
          }
          if (event is KeyDownEvent) begin();
          if (event is KeyUpEvent) end();
          return KeyEventResult.handled;
        },
        child: Semantics(
          label: label,
          button: true,
          enabled: enabled,
          onTap: enabled
              ? () {
                  if (widget.listening || widget.starting) {
                    end();
                  } else {
                    begin();
                  }
                }
              : null,
          child: MouseRegion(
            cursor: enabled
                ? SystemMouseCursors.click
                : SystemMouseCursors.basic,
            child: Listener(
              behavior: HitTestBehavior.opaque,
              onPointerDown: (event) {
                if (!enabled ||
                    event.buttons != kPrimaryButton ||
                    pointer != null) {
                  return;
                }
                pointer = event.pointer;
                focus.requestFocus();
                begin();
              },
              onPointerUp: (event) {
                if (event.pointer == pointer) end();
              },
              onPointerCancel: (event) {
                if (event.pointer == pointer) end();
              },
              child: Container(
                width: 28,
                height: 28,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(8),
                  color: widget.listening
                      ? const Color(0x1fd92d20)
                      : DshColors(context).layer,
                  border: focused
                      ? Border.all(color: DshColors(context).blue)
                      : null,
                ),
                child: DshGlyph(
                  LucideIcons.mic,
                  asset: 'assets/icons/voice-mic.svg',
                  size: 16,
                  color: widget.listening
                      ? const Color(0xffd92d20)
                      : DshColors(context).text
                            .withValues(alpha: enabled ? 1 : .45),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
