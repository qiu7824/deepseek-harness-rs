import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

/// Decode bounds are physical pixels, independently of the source aspect ratio.
/// Bounding both dimensions also limits unusually tall screenshots.
({int width, int height}) imageDecodeBounds(
  Size logicalSize,
  double devicePixelRatio, {
  int maxDimension = 2400,
  int maxPixels = 4 * 1024 * 1024,
}) {
  final ratio = devicePixelRatio.isFinite && devicePixelRatio > 0
      ? devicePixelRatio
      : 1.0;
  int dimension(double value) =>
      ((value.isFinite && value > 0 ? value : 800) * ratio).ceil().clamp(
        1,
        maxDimension,
      );
  var width = dimension(logicalSize.width);
  var height = dimension(logicalSize.height);
  if (width * height > maxPixels) {
    final scale = math.sqrt(maxPixels / (width * height));
    width = math.max(1, (width * scale).floor());
    height = math.max(1, (height * scale).floor());
  }
  return (width: width, height: height);
}

class DshBoundedImage extends StatefulWidget {
  const DshBoundedImage({
    super.key,
    required this.image,
    this.fit = BoxFit.contain,
    this.semanticLabel,
    this.errorBuilder,
    this.maxDimension = 2400,
    this.maxPixels = 4 * 1024 * 1024,
    this.filterQuality = FilterQuality.medium,
    this.evictOnDispose = false,
  });

  final ImageProvider image;
  final BoxFit fit;
  final String? semanticLabel;
  final ImageErrorWidgetBuilder? errorBuilder;
  final int maxDimension, maxPixels;
  final FilterQuality filterQuality;
  final bool evictOnDispose;

  @override
  State<DshBoundedImage> createState() => _DshBoundedImageState();
}

class _DshBoundedImageState extends State<DshBoundedImage> {
  ResizeImage? decoded;

  @override
  void dispose() {
    if (widget.evictOnDispose && decoded != null) unawaited(decoded!.evict());
    decoded = null;
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final view = MediaQuery.sizeOf(context);
      final bounds = imageDecodeBounds(
        Size(
          constraints.hasBoundedWidth ? constraints.maxWidth : view.width,
          constraints.hasBoundedHeight ? constraints.maxHeight : view.height,
        ),
        MediaQuery.devicePixelRatioOf(context),
        maxDimension: widget.maxDimension,
        maxPixels: widget.maxPixels,
      );
      final next = ResizeImage(
        widget.image,
        width: bounds.width,
        height: bounds.height,
        policy: ResizeImagePolicy.fit,
        allowUpscaling: false,
      );
      if (decoded != next) {
        final old = decoded;
        decoded = next;
        if (widget.evictOnDispose && old != null) {
          WidgetsBinding.instance.addPostFrameCallback(
            (_) => unawaited(old.evict()),
          );
        }
      }
      return Image(
        image: next,
        fit: widget.fit,
        semanticLabel: widget.semanticLabel,
        errorBuilder: widget.errorBuilder,
        filterQuality: widget.filterQuality,
      );
    },
  );
}
