import 'dart:ffi' show Abi;

import 'package:dsh_desktop/design/primitives.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  final goldenDirectory = Abi.current() == Abi.macosArm64
      ? 'goldens/macos-arm64'
      : 'goldens';
  for (final tokens in [DshTokens.light, DshTokens.dark]) {
    for (final ratio in [1.0, 1.5]) {
      testWidgets('semantic icon matrix ${tokens.brightness.name} at $ratio', (
        tester,
      ) async {
        tester.view.devicePixelRatio = ratio;
        tester.view.physicalSize = Size(700 * ratio, 760 * ratio);
        addTearDown(tester.view.resetDevicePixelRatio);
        addTearDown(tester.view.resetPhysicalSize);
        await tester.pumpWidget(
          MaterialApp(
            theme: ThemeData(
              brightness: tokens.brightness,
              extensions: [tokens],
            ),
            home: RepaintBoundary(
              key: const Key('icon-matrix'),
              child: ColoredBox(
                color: tokens.base,
                child: Padding(
                  padding: const EdgeInsets.all(16),
                  child: Column(
                    children: [
                      for (final size in [16.0, 20.0, 24.0]) ...[
                        Wrap(
                          children: [
                            for (final icon in DshIcons.values)
                              SizedBox(
                                width: 36,
                                height: 36,
                                child: DshGlyph(
                                  icon.data,
                                  size: size,
                                  color: tokens.text,
                                ),
                              ),
                          ],
                        ),
                        const SizedBox(height: 8),
                      ],
                    ],
                  ),
                ),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
        final boundary = tester.renderObject<RenderRepaintBoundary>(
          find.byKey(const Key('icon-matrix')),
        );
        final image = await tester.runAsync(
          () => boundary.toImage(pixelRatio: ratio),
        );
        try {
          await expectLater(
            image,
            matchesGoldenFile(
              '$goldenDirectory/icons_${tokens.brightness.name}_${ratio}x.png',
            ),
          );
        } finally {
          image?.dispose();
        }
      });
    }
  }
}
