import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';

class ProbePreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

void main() {
  testWidgets('native shell raster probe', (tester) async {
    await tester.binding.setSurfaceSize(const Size(1280, 720));
    await tester.runAsync(() async {
      final fonts = Platform.environment['WINDIR'] ?? 'C:/Windows';
      for (final entry in {
        'Segoe UI': 'segoeui.ttf',
        'Microsoft YaHei': 'msyh.ttc',
      }.entries) {
        final file = File('$fonts/Fonts/${entry.value}');
        if (await file.exists()) {
          final loader = FontLoader(entry.key)
            ..addFont(
              file.readAsBytes().then((bytes) => ByteData.sublistView(bytes)),
            );
          await loader.load();
        }
      }
    });
    final c = DesktopController(ProbePreferences()), key = GlobalKey();
    await tester.pumpWidget(
      RepaintBoundary(
        key: key,
        child: DesktopApp(controller: c),
      ),
    );
    await tester.pumpAndSettle();
    await tester.runAsync(() async {
      final image =
          await (key.currentContext!.findRenderObject()!
                  as RenderRepaintBoundary)
              .toImage();
      final data = await image.toByteData(format: ui.ImageByteFormat.png);
      await File(Platform.environment['DSH_QA_IMAGE']!)
          .writeAsBytes(data!.buffer.asUint8List());
      image.dispose();
    });
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    expect(tester.takeException(), isNull);
  }, skip: Platform.environment['DSH_QA_IMAGE'] == null);
}
