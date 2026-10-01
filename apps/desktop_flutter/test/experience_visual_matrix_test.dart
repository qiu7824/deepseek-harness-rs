import 'dart:io';
import 'dart:ui' as ui;

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'workbench_test.dart' show TestController;

void main() {
  var fontsLoaded = false;
  for (final dark in [false, true]) {
    for (final item in [
      (size: const Size(720, 520), scale: 1.0),
      (size: const Size(720, 520), scale: 2.0),
      (size: const Size(1280, 800), scale: 1.0),
      (size: const Size(1280, 800), scale: 1.5),
      (size: const Size(1440, 1000), scale: 2.0),
    ]) {
      testWidgets(
        'desktop reading matrix dark=$dark size=${item.size} text=${item.scale}',
        (tester) async {
          final output = Platform.environment['DSH_EXPERIENCE_VISUAL_DIR'];
          await tester.binding.setSurfaceSize(item.size);
          tester.platformDispatcher.textScaleFactorTestValue = item.scale;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
          addTearDown(() => tester.binding.setSurfaceSize(null));
          if (output != null && !fontsLoaded) {
            await tester.runAsync(() async {
              final fonts = Platform.environment['WINDIR'] ?? 'C:/Windows';
              for (final font in {
                'Segoe UI': 'segoeui.ttf',
                'Microsoft YaHei UI': 'msyh.ttc',
                'Microsoft YaHei': 'msyh.ttc',
                'Cascadia Mono': 'CascadiaMono.ttf',
                'Consolas': 'consola.ttf',
              }.entries) {
                final file = File('$fonts/Fonts/${font.value}');
                if (await file.exists()) {
                  final loader = FontLoader(font.key)
                    ..addFont(file.readAsBytes().then(ByteData.sublistView));
                  await loader.load();
                }
              }
              fontsLoaded = true;
            });
          }
          final c = TestController()..selectedId = 'visual-fixture';
          c.preferences.dark = dark;
          c.sessions = [
            SessionSummary.fromJson({
              'sessionId': 'visual-fixture',
              'displayTitle': '桌面交互与阅读体验',
              'cwd': r'D:\项目\桌面客户端',
            }),
          ];
          c.transcript = [
            TranscriptItem(
              id: 'request',
              kind: 'user',
              text: '检查界面文字、代码和表格的阅读效果。',
            ),
            TranscriptItem(
              id: 'answer',
              kind: 'assistant',
              text: '# 阅读体验\n\n中文正文采用清晰的行距，English、数字 123 与中文自然对齐。\n\n## 检查项目\n\n- 输入框支持多行编辑\n- 工具操作可以通过键盘完成\n\n```dart\nfinal message = "你好，世界";\nprint(message);\n```\n\n| 项目 | 状态 |\n| --- | --- |\n| 字体 | 可读 |\n| 图标 | 清晰 |',
            ),
          ];
          c.emit();
          final boundary = GlobalKey();
          await tester.pumpWidget(
            RepaintBoundary(
              key: boundary,
              child: DesktopApp(controller: c),
            ),
          );
          await tester.pumpAndSettle();
          expect(find.byKey(const Key('prompt-input')), findsOneWidget);
          expect(tester.takeException(), isNull);
          if (output != null) {
            await tester.runAsync(() async {
              final image =
                  await (boundary.currentContext!.findRenderObject()!
                          as RenderRepaintBoundary)
                      .toImage(pixelRatio: 1);
              final data = await image.toByteData(
                format: ui.ImageByteFormat.png,
              );
              await Directory(output).create(recursive: true);
              await File(
                '$output/${dark ? 'dark' : 'light'}-${item.size.width.toInt()}-${item.scale}.png',
              ).writeAsBytes(data!.buffer.asUint8List());
              image.dispose();
            });
          }
          await tester.pumpWidget(const SizedBox());
          c.dispose();
        },
        variant: TargetPlatformVariant({TargetPlatform.windows}),
      );
    }
  }
}
