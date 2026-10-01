import 'dart:async';

import 'package:flutter/widgets.dart';

import '../../src/resource_diagnostics.dart';

/// Retains navigation state in the caller, while releasing reconstructible
/// renderers after inactivity. Terminal and control sessions must not be children.
class ReclaimablePreview extends StatefulWidget {
  const ReclaimablePreview({
    super.key,
    required this.builder,
    this.onRelease,
    this.retention = const Duration(minutes: 2),
  });

  final WidgetBuilder builder;
  final VoidCallback? onRelease;
  final Duration retention;

  @override
  State<ReclaimablePreview> createState() => _ReclaimablePreviewState();
}

class _ReclaimablePreviewState extends State<ReclaimablePreview>
    with WidgetsBindingObserver, ResourceDiagnosticScope {
  Timer? _releaseTimer;
  bool _panelVisible = true, _appVisible = true, _released = false;
  bool get _visible => _panelVisible && _appVisible;

  @override
  String get resourceScopeKind => 'preview-retention';
  @override
  Map<String, int> get resourceDiagnostics => {
    'previewRetentionTimers': _releaseTimer?.isActive == true ? 1 : 0,
    'suspendedPreviews': _released ? 1 : 0,
  };

  @override
  void initState() {
    super.initState();
    final lifecycle = WidgetsBinding.instance.lifecycleState;
    _appVisible = lifecycle == null || lifecycle == AppLifecycleState.resumed;
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _panelVisible = TickerMode.valuesOf(context).enabled;
    _updateVisibility();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _appVisible = state == AppLifecycleState.resumed;
    _updateVisibility();
  }

  void _updateVisibility() {
    if (_visible) {
      _releaseTimer?.cancel();
      _releaseTimer = null;
      if (_released) setState(() => _released = false);
    } else if (!_released && _releaseTimer?.isActive != true) {
      _releaseTimer = Timer(widget.retention, _release);
    }
  }

  void _release() {
    _releaseTimer?.cancel();
    _releaseTimer = null;
    if (!mounted || _visible || _released) return;
    setState(() => _released = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _released) widget.onRelease?.call();
    });
    // A minimized application may not receive vsync. Finish one disposal frame
    // so native viewers and post-frame cache eviction do not wait for resume.
    if (!WidgetsBinding.instance.framesEnabled) {
      WidgetsBinding.instance.scheduleWarmUpFrame();
    }
  }

  @override
  void didHaveMemoryPressure() => _release();

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _releaseTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      _released ? const SizedBox.shrink() : widget.builder(context);
}
