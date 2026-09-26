import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';

import '../../design/primitives.dart';

/// Animates received text; history is displayed immediately when [revealInitial]
/// is false. The pending tail catches up within a short interval and is flushed
/// when streaming or animations stop.
class ProgressiveText extends StatefulWidget {
  const ProgressiveText({
    super.key,
    required this.text,
    required this.streaming,
    this.revealInitial = true,
    required this.builder,
  });
  final String text;
  final bool streaming;
  final bool revealInitial;
  final Widget Function(String text) builder;
  @override
  State<ProgressiveText> createState() => _ProgressiveTextState();
}

class _ProgressiveTextState extends State<ProgressiveText>
    with SingleTickerProviderStateMixin {
  late final Ticker ticker = createTicker(tick);
  List<int> ends = [];
  int shown = 0;
  Duration previous = Duration.zero;
  Duration catchUpAt = const Duration(milliseconds: 300);
  bool reduced = false, tickersEnabled = true;
  @override
  void initState() {
    super.initState();
    index();
    shown = widget.streaming && widget.revealInitial ? 0 : ends.length;
  }

  void index() {
    var end = 0;
    ends = [
      for (final character in widget.text.characters) (end += character.length),
    ];
  }

  void extend() {
    if (ends.isEmpty) {
      index();
      return;
    }
    // The new code points can join the previous grapheme (combining marks,
    // emoji joins or regional indicators), so only that final cluster needs
    // to be segmented again.
    final restart = ends.length == 1 ? 0 : ends[ends.length - 2];
    ends.removeLast();
    var end = restart;
    for (final character in widget.text.substring(restart).characters) {
      end += character.length;
      ends.add(end);
    }
  }

  void sync() {
    if (!widget.streaming || reduced || !tickersEnabled) shown = ends.length;
    if (shown < ends.length && !ticker.isActive) {
      previous = Duration.zero;
      catchUpAt = const Duration(milliseconds: 300);
      ticker.start();
    }
    if (shown >= ends.length && ticker.isActive) ticker.stop();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    reduced = MediaQuery.disableAnimationsOf(context);
    tickersEnabled = TickerMode.valuesOf(context).enabled;
    sync();
  }

  @override
  void didUpdateWidget(ProgressiveText old) {
    super.didUpdateWidget(old);
    if (old.text != widget.text) {
      if (widget.text.startsWith(old.text) &&
          (old.streaming || widget.streaming)) {
        extend();
        shown = math.min(shown, ends.length);
      } else {
        index();
        shown = ends.length;
      }
      catchUpAt = previous + const Duration(milliseconds: 300);
    }
    sync();
  }

  void tick(Duration elapsed) {
    final micros = (elapsed - previous).inMicroseconds;
    if (micros < 33000) return;
    previous = elapsed;
    final count = math
        .max(1, math.max(60, (ends.length - shown) / .24) * micros / 1000000)
        .ceil();
    setState(
      () => shown = elapsed >= catchUpAt
          ? ends.length
          : math.min(ends.length, shown + count),
    );
    sync();
  }

  @override
  void dispose() {
    ticker.dispose();
    ends = [];
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.builder(
    shown >= ends.length
        ? widget.text
        : widget.text.substring(0, shown == 0 ? 0 : ends[shown - 1]),
  );
}

class ThinkingSweep extends StatefulWidget {
  const ThinkingSweep({super.key, required this.running, required this.child});
  final bool running;
  final Widget child;
  @override
  State<ThinkingSweep> createState() => _ThinkingSweepState();
}

class _ThinkingSweepState extends State<ThinkingSweep>
    with SingleTickerProviderStateMixin {
  late final animation = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 2600),
  );
  void sync() {
    if (widget.running && !MediaQuery.disableAnimationsOf(context)) {
      if (!animation.isAnimating) animation.repeat();
    } else {
      animation.stop();
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    sync();
  }

  @override
  void didUpdateWidget(ThinkingSweep old) {
    super.didUpdateWidget(old);
    sync();
  }

  @override
  void dispose() {
    animation.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => RepaintBoundary(
    child: ClipRect(
      child: Stack(
        children: [
          widget.child,
          if (widget.running && !MediaQuery.disableAnimationsOf(context))
            Positioned.fill(
              child: IgnorePointer(
                child: CustomPaint(
                  painter: _SweepPainter(animation, DshColors(context).base),
                ),
              ),
            ),
        ],
      ),
    ),
  );
}

class _SweepPainter extends CustomPainter {
  _SweepPainter(this.animation, this.base) : super(repaint: animation);
  final Animation<double> animation;
  final Color base;
  @override
  void paint(Canvas canvas, Size size) {
    final x =
        (size.width + 300) * Curves.easeOut.transform(animation.value) - 300;
    final rect = Rect.fromLTWH(x, 0, 300, size.height);
    canvas.drawRect(
      rect,
      Paint()
        ..shader = LinearGradient(
          colors: [
            base.withValues(alpha: 0),
            base.withValues(alpha: .6),
            base.withValues(alpha: 0),
          ],
          stops: const [0, .55, 1],
        ).createShader(rect),
    );
  }

  @override
  bool shouldRepaint(_SweepPainter old) =>
      old.base != base || old.animation != animation;
}
