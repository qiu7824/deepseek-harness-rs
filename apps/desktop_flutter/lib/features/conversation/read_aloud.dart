import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// One native Windows voice for the visible conversation. Chunks bound the
/// platform message size while preserving the full final answer.
class ReadAloudController extends ChangeNotifier {
  static const channel = MethodChannel('dsh/read-aloud');
  static const chunkCodeUnits = 4096;
  Timer? _timer;
  String? _text, _activeId;
  int _offset = 0, _serial = 0;
  bool _polling = false, _disposed = false;

  String? get activeId => _activeId;
  int get retainedTextUnits => _text?.length ?? 0;
  bool isSpeaking(String id) => _activeId == id;
  String get generation => 'speech-$_serial';

  Future<void> toggle(String id, String text) async {
    if (_activeId == id) {
      await stop();
      return;
    }
    await stop();
    if (_disposed || text.trim().isEmpty) return;
    ++_serial;
    _activeId = id;
    _text = text;
    _offset = 0;
    notifyListeners();
    final owner = generation;
    try {
      await _next(owner);
    } catch (_) {
      if (_activeId == id && generation == owner) await stop();
      rethrow;
    }
  }

  Future<void> _next(String owner) async {
    if (_disposed || owner != generation || _activeId == null) return;
    final value = _text;
    if (value == null || _offset >= value.length) {
      await stop();
      return;
    }
    var end = _offset + chunkCodeUnits;
    if (end > value.length) end = value.length;
    if (end < value.length) {
      final last = value.codeUnitAt(end - 1);
      if (last >= 0xd800 && last <= 0xdbff) end--;
    }
    final chunk = value.substring(_offset, end);
    _offset = end;
    await channel.invokeMethod<void>('start', {
      'generation': owner,
      'text': chunk,
    });
    if (_disposed || owner != generation || _activeId == null) {
      await channel.invokeMethod<void>('stop', {'generation': owner});
      return;
    }
    _timer ??= Timer.periodic(
      const Duration(milliseconds: 350),
      (_) => unawaited(_poll(owner)),
    );
  }

  Future<void> _poll(String owner) async {
    if (_polling || _disposed || owner != generation || _activeId == null) {
      return;
    }
    _polling = true;
    try {
      final done = await channel.invokeMethod<bool>('status', {
        'generation': owner,
      });
      if (done == true && owner == generation && _activeId != null) {
        await _next(owner);
      }
    } catch (_) {
      if (owner == generation) await stop();
    } finally {
      _polling = false;
    }
  }

  Future<void> stop() async {
    final old = _activeId == null ? null : generation;
    _timer?.cancel();
    _timer = null;
    _activeId = null;
    _text = null;
    _offset = 0;
    ++_serial;
    if (!_disposed && old != null) notifyListeners();
    if (old != null) {
      try {
        await channel.invokeMethod<void>('stop', {'generation': old});
      } catch (_) {
        // Shutdown may race the runner's channel removal; local resources are
        // already released before this best-effort native stop is sent.
      }
    }
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    unawaited(stop());
    super.dispose();
  }
}
