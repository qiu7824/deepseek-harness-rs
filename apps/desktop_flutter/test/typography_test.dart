import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/design/icon_assets.dart';
import 'package:dsh_desktop/design/select.dart';

void main() {
  testWidgets(
    'settings switch has the web track size and responds to keyboard',
    (tester) async {
      var value = false;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(
              child: StatefulBuilder(
                builder: (_, setState) => DshSwitch(
                  value: value,
                  onChanged: (next) => setState(() => value = next),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.getSize(find.byType(DshSwitch)), const Size(36, 22));
      await tester.sendKeyEvent(LogicalKeyboardKey.tab);
      await tester.sendKeyEvent(LogicalKeyboardKey.space);
      await tester.pumpAndSettle();
      expect(value, isTrue);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets('search vector remains 16px inside a 36px input prefix', (
    tester,
  ) async {
    await tester.pumpWidget(
      const ShadApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: 400,
              child: DshField(prefix: LucideIcons.search, hint: '搜索会话'),
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(tester.getSize(find.byType(TextField)).height, 36);
    expect(tester.getSize(find.byType(SvgPicture)), const Size(16, 16));
    expect(tester.takeException(), isNull);
  });
  testWidgets(
    'dropdown height, chevron size and controlled selection stay stable',
    (tester) async {
      String value = 'queue';
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(
              child: StatefulBuilder(
                builder: (context, setState) => DshSelect<String>(
                  options: const {'queue': '排队发送', 'steer': '立即引导'},
                  value: value,
                  onChanged: (v) => setState(() => value = v),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.getSize(find.byType(DshSelect<String>)).height, 36);
      expect(tester.getSize(find.byType(SvgPicture)), const Size(14, 14));
      await tester.tap(find.text('排队发送'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('立即引导'));
      await tester.pumpAndSettle();
      expect(value, 'steer');
      expect(find.text('立即引导'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'compact archive buttons preserve requested control and vector sizes',
    (tester) async {
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(
              child: DshButton(
                primary: true,
                pill: true,
                height: 32,
                fontSize: 13,
                icon: LucideIcons.archive,
                onPressed: () {},
                child: const Text('恢复'),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.getSize(find.byType(DshButton)).height, 32);
      expect(tester.getSize(find.byType(SvgPicture)), const Size(16, 16));
      expect(tester.takeException(), isNull);
    },
  );
  test('web typography is explicit for Latin and Chinese glyphs', () {
    expect(DshTypography.body.fontSize, 14);
    expect(DshTypography.body.height, 22 / 14);
    expect(DshTypography.composer.fontSize, 16);
    expect(DshTypography.composer.height, 1.5);
    expect(DshTypography.headline.fontSize, 26);
    expect(DshTypography.headline.height, 32 / 26);
    expect(DshTypography.shad.family, 'Segoe UI');
    expect(
      DshTypography.shad.p.fontFamilyFallback,
      contains('Microsoft YaHei'),
    );
    expect(
      DshTypography.material(const TextTheme()).bodyMedium!.fontFamilyFallback,
      contains('Microsoft YaHei'),
    );
  });
  testWidgets(
    'core navigation uses shipped vectors rather than icon font glyphs',
    (tester) async {
      final icons = [
        LucideIcons.settings,
        LucideIcons.circlePlus,
        LucideIcons.search,
        LucideIcons.paperclip,
        LucideIcons.brain,
        LucideIcons.users,
        LucideIcons.database,
      ];
      await tester.pumpWidget(
        ShadApp(
          home: Row(
            children: [for (final icon in icons) DshGlyph(icon, size: 16)],
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(SvgPicture), findsNWidgets(icons.length));
      for (final icon in icons) {
        final svg = await rootBundle.loadString(dshIconAssets[icon.codePoint]!);
        expect(svg, contains('<svg'));
        expect(svg, contains('<path'));
        expect(svg, isNot(contains('data:image')));
      }
      expect(tester.takeException(), isNull);
    },
  );
}
