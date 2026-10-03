import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:dsh_desktop/design/bounded_image.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('physical decode bounds cap both axes and total pixels', () {
    expect(imageDecodeBounds(const Size(200, 100), 2), (
      width: 400,
      height: 200,
    ));
    for (final size in [
      const Size(100, 100000),
      const Size(100000, 100),
      const Size(4000, 4000),
    ]) {
      final bounds = imageDecodeBounds(size, 3);
      expect(bounds.width, lessThanOrEqualTo(2400));
      expect(bounds.height, lessThanOrEqualTo(2400));
      expect(bounds.width * bounds.height, lessThanOrEqualTo(4 * 1024 * 1024));
    }
  });

  testWidgets('closing a preview evicts its actual resized image cache key', (
    tester,
  ) async {
    late Uint8List png;
    await tester.runAsync(() async {
      final recorder = ui.PictureRecorder();
      Canvas(recorder).drawColor(Colors.blue, BlendMode.src);
      final picture = recorder.endRecording();
      final image = await picture.toImage(16, 16);
      png = (await image.toByteData(format: ui.ImageByteFormat.png))!.buffer
          .asUint8List();
      image.dispose();
      picture.dispose();
    });
    await tester.pumpWidget(
      MaterialApp(
        home: Center(
          child: SizedBox(
            width: 128,
            height: 96,
            child: DshBoundedImage(
              image: MemoryImage(png),
              evictOnDispose: true,
            ),
          ),
        ),
      ),
    );
    await tester.runAsync(() async {
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await tester.pump();
    final provider =
        tester.widget<Image>(find.byType(Image)).image as ResizeImage;
    final key = await provider.obtainKey(ImageConfiguration.empty);
    expect(provider.width, (128 * tester.view.devicePixelRatio).ceil());
    expect(provider.height, (96 * tester.view.devicePixelRatio).ceil());
    expect(PaintingBinding.instance.imageCache.containsKey(key), isTrue);
    await tester.pumpWidget(const SizedBox());
    await tester.runAsync(() async {
      await Future<void>.delayed(Duration.zero);
    });
    expect(PaintingBinding.instance.imageCache.containsKey(key), isFalse);
  });
}
