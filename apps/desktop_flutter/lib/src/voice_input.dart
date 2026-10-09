import '../l10n/runtime_zh.dart';

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../design/primitives.dart';

/// Native Windows dictation bridge. It uses the platform recognizer through a
/// MethodChannel, so the desktop client never embeds a browser runtime.
class VoiceInputController extends ChangeNotifier {
  static const _channel = MethodChannel('dsh/voice');
  static const stopTimeout = Duration(seconds: 5);
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
  int get retainedTextUnits => latestText.length + _committed.length;
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
          ? DshRuntimeZh.voicePluginUnavailable
          : e is PlatformException && e.code == 'voice-busy'
          ? DshRuntimeZh.voiceReleasing
          : DshRuntimeZh.voiceStartFailed(error: e);
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
    if (_disposed || _generation != token) return;
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
              error = DshRuntimeZh.voiceTextLimit;
              unawaited(stop());
              continue;
            }
            latestText = next;
            textRevision++;
            if (value['kind'] == 'result') _committed = next;
            changed = true;
          case 'error':
            error = DshRuntimeZh.voiceRecognizerUnavailable(
              detail: value['text'],
            );
            unawaited(stop());
            changed = true;
          case 'stopped':
            phase = 'idle';
            _generation = null;
            _limit?.cancel();
            _limit = null;
            _timer?.cancel();
            _timer = null;
            _committed = '';
            changed = true;
        }
      }
      if (changed) _emit();
      _schedule(token);
    } catch (e) {
      if (_disposed || _generation != token) return;
      error = DshRuntimeZh.voicePollFailed(error: e);
      cancel();
    }
  }

  Future<void> stop() async {
    if (_disposed || !active || phase == 'stopping') return;
    final token = _generation;
    phase = 'stopping';
    _limit?.cancel();
    // Native shutdown may lose its final event during device or window
    // teardown. Keep final-result polling bounded even when that event is lost.
    _limit = Timer(stopTimeout, () {
      if (!_disposed && _generation == token) cancel();
    });
    _emit();
    try {
      await _channel.invokeMethod<void>('stop');
    } catch (e) {
      if (!_disposed && _generation == token) {
        error = DshRuntimeZh.voiceStopFailed(error: e);
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
    _timer = null;
    _limit = null;
    latestText = _committed = '';
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
        ? DshRuntimeZh.voiceUnsupported
        : widget.error ??
              (widget.starting
                  ? DshRuntimeZh.voiceStarting
                  : widget.stopping
                  ? DshRuntimeZh.voiceStopping
                  : widget.listening
                  ? DshRuntimeZh.voiceListeningHint
                  : DshRuntimeZh.voiceIdleHint);
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
                  DshIcons.mic.data,
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
