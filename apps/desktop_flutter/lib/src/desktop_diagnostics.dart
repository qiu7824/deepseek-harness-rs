import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'dart:ui' show FrameTiming;

import 'controller.dart';
import 'resource_diagnostics.dart';

/// Opt-in local JSONL counters. No control commands or conversation data.
class DesktopDiagnostics with WidgetsBindingObserver {
  DesktopDiagnostics(this.controller, this.file);
  final DesktopController controller;
  final File file;
  Timer? _timer;
  bool _writing = false, _closed = false, _observing = false;
  final _buildMicros = <int>[], _rasterMicros = <int>[];
  void _onTimings(List<FrameTiming> frames) {
    for (final frame in frames) {
      if (_buildMicros.length >= 600) break;
      _buildMicros.add(frame.buildDuration.inMicroseconds);
      _rasterMicros.add(frame.rasterDuration.inMicroseconds);
    }
  }

  static Future<DesktopDiagnostics?> start(
    DesktopController controller,
    String? path,
  ) async {
    if (path == null || path.trim().isEmpty) return null;
    final monitor = DesktopDiagnostics(controller, File(path));
    try {
      await monitor.file.parent.create(recursive: true);
      await monitor.sample();
      if (monitor._closed) return null;
      WidgetsBinding.instance.addObserver(monitor);
      WidgetsBinding.instance.addTimingsCallback(monitor._onTimings);
      monitor._observing = true;
      monitor._timer = Timer.periodic(
        const Duration(seconds: 5),
        (_) => unawaited(monitor.sample()),
      );
      return monitor;
    } catch (_) {
      return null;
    }
  }

  Map<String, Object> snapshot() {
    final owners = <String, int>{};
    void visit(Element element) {
      if (element is StatefulElement && element.state is ResourceDiagnostics) {
        for (final value
            in (element.state as ResourceDiagnostics)
                .resourceDiagnostics
                .entries) {
          owners.update(
            value.key,
            (n) => n + value.value,
            ifAbsent: () => value.value,
          );
        }
      }
      element.visitChildElements(visit);
    }

    WidgetsBinding.instance.rootElement?.visitChildElements(visit);
    final cache = PaintingBinding.instance.imageCache;
    final view = WidgetsBinding.instance.platformDispatcher.views.firstOrNull;
    int percentile(List<int> values) {
      if (values.isEmpty) return 0;
      values.sort();
      return values[((values.length - 1) * .95).round()];
    }

    final frames = _buildMicros.length,
        buildP95 = percentile(_buildMicros),
        rasterP95 = percentile(_rasterMicros);
    final buildOver = _buildMicros.where((v) => v > 16667).length,
        rasterOver = _rasterMicros.where((v) => v > 16667).length;
    _buildMicros.clear();
    _rasterMicros.clear();
    return {
      'time': DateTime.now().toUtc().toIso8601String(),
      'pid': pid,
      'buildMode': kReleaseMode
          ? 'release'
          : kProfileMode
          ? 'profile'
          : 'debug',
      'viewPhysicalWidth': view?.physicalSize.width ?? 0,
      'viewPhysicalHeight': view?.physicalSize.height ?? 0,
      'devicePixelRatio': view?.devicePixelRatio ?? 0,
      ...controller.resourceDiagnostics,
      'owners': owners,
      'imageCacheBytes': cache.currentSizeBytes,
      'imageCacheEntries': cache.currentSize,
      'imageCacheLive': cache.liveImageCount,
      'imageCachePending': cache.pendingImageCount,
      'imageCacheBudgetBytes': cache.maximumSizeBytes,
      'frameSamples': frames,
      'buildP95Micros': buildP95,
      'rasterP95Micros': rasterP95,
      'buildFramesOver16ms': buildOver,
      'rasterFramesOver16ms': rasterOver,
    };
  }

  Future<void> sample() async {
    if (_closed || _writing) return;
    _writing = true;
    try {
      await file.writeAsString(
        '${jsonEncode(snapshot())}\n',
        mode: FileMode.append,
        flush: true,
      );
    } catch (_) {
      close();
    } finally {
      _writing = false;
    }
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.detached) close();
  }

  void close() {
    if (_closed) return;
    _closed = true;
    _timer?.cancel();
    if (_observing) {
      WidgetsBinding.instance.removeObserver(this);
      WidgetsBinding.instance.removeTimingsCallback(_onTimings);
      _observing = false;
    }
  }
}
