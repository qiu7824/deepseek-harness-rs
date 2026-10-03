import 'dart:async';
import 'dart:convert';
import 'dart:collection';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'dart:ui' show FrameTiming;

import 'controller.dart';
import 'resource_diagnostics.dart';

/// Bounded recent samples, with exact event and missed-budget counts for the
/// entire reporting interval. A stalled writer cannot grow this buffer.
class DesktopFrameWindow {
  static const capacity = 1200;
  final _build = Queue<int>(), _raster = Queue<int>();
  int _reported = 0, _buildOver = 0, _rasterOver = 0;

  void add({required int buildMicros, required int rasterMicros}) {
    _reported++;
    if (buildMicros > 16667) _buildOver++;
    if (rasterMicros > 16667) _rasterOver++;
    if (_build.length == capacity) {
      _build.removeFirst();
      _raster.removeFirst();
    }
    _build.add(buildMicros);
    _raster.add(rasterMicros);
  }

  Map<String, int> take() {
    int percentile(Queue<int> samples) {
      if (samples.isEmpty) return 0;
      final sorted = samples.toList()..sort();
      return sorted[((sorted.length - 1) * .95).round()];
    }

    final result = {
      'frameSamples': _build.length,
      'frameReportedSamples': _reported,
      'frameSamplesDiscarded': _reported - _build.length,
      'buildP95Micros': percentile(_build),
      'rasterP95Micros': percentile(_raster),
      'buildFramesOver16ms': _buildOver,
      'rasterFramesOver16ms': _rasterOver,
    };
    _build.clear();
    _raster.clear();
    _reported = _buildOver = _rasterOver = 0;
    return result;
  }
}

/// Opt-in local JSONL counters. No control commands or conversation data.
class DesktopDiagnostics with WidgetsBindingObserver {
  DesktopDiagnostics(this.controller, this.file);
  final DesktopController controller;
  final File file;
  Timer? _timer;
  bool _writing = false, _closed = false, _observing = false;
  final _frames = DesktopFrameWindow();
  final _frameWindow = Stopwatch()..start();
  void _onTimings(List<FrameTiming> frames) {
    for (final frame in frames) {
      _frames.add(
        buildMicros: frame.buildDuration.inMicroseconds,
        rasterMicros: frame.rasterDuration.inMicroseconds,
      );
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
    final scopes = <Map<String, Object>>[];
    void visit(Element element) {
      if (element is StatefulElement && element.state is ResourceDiagnostics) {
        final state = element.state as ResourceDiagnostics;
        final counters = state.resourceDiagnostics;
        if (state is ScopedResourceDiagnostics) {
          scopes.add({
            'id': state.resourceScopeId,
            'kind': state.resourceScopeKind,
            'counters': counters,
          });
        }
        for (final value in counters.entries) {
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
    final windowMs = _frameWindow.elapsedMilliseconds;
    _frameWindow.reset();
    return {
      'time': DateTime.now().toUtc().toIso8601String(),
      'pid': pid,
      'processCurrentRssBytes': ProcessInfo.currentRss,
      'processMaxRssBytes': ProcessInfo.maxRss,
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
      'scopes': scopes,
      'imageCacheBytes': cache.currentSizeBytes,
      'imageCacheEntries': cache.currentSize,
      'imageCacheLive': cache.liveImageCount,
      'imageCachePending': cache.pendingImageCount,
      'imageCacheBudgetBytes': cache.maximumSizeBytes,
      'frameWindowMilliseconds': windowMs,
      'framePercentilePolicy': 'latest-1200-since-previous-sample',
      ..._frames.take(),
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
