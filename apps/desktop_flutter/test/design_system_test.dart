import 'package:dsh_desktop/design/breakpoints.dart';
import 'package:dsh_desktop/design/icon_assets.dart';
import 'package:dsh_desktop/design/motion.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/design/select.dart';
import 'package:dsh_desktop/design/typography.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

double _contrast(Color a, Color b) {
  final x = a.computeLuminance();
  final y = b.computeLuminance();
  return ((x > y ? x : y) + .05) / ((x < y ? x : y) + .05);
}

void main() {
  test('normal and small text meet contrast on all supported surfaces', () {
    for (final tokens in [DshTokens.light, DshTokens.dark]) {
      for (final surface in [
        tokens.base,
        tokens.sidebar,
        tokens.layer,
        tokens.hover,
        tokens.selected,
        tokens.bubble,
      ]) {
        expect(_contrast(tokens.text, surface), greaterThanOrEqualTo(4.5));
        expect(_contrast(tokens.muted, surface), greaterThanOrEqualTo(4.5));
        expect(_contrast(tokens.focus, surface), greaterThanOrEqualTo(3));
      }
      expect(
        _contrast(tokens.onAccent, tokens.accent),
        greaterThanOrEqualTo(4.5),
      );
      for (final status in [
        tokens.success,
        tokens.warning,
        tokens.error,
        tokens.info,
      ]) {
        expect(
          _contrast(status.foreground, status.background),
          greaterThanOrEqualTo(4.5),
        );
      }
    }
  });

  test(
    'selection stays distinct from hover while accent keeps blue semantics',
    () {
      for (final tokens in [DshTokens.light, DshTokens.dark]) {
        expect(tokens.selected, isNot(tokens.hover));
        expect(tokens.accent.b, greaterThan(tokens.accent.r));
        expect(tokens.accent.b, greaterThan(tokens.accent.g));
        expect(tokens.disabled, isNot(tokens.text));
        expect(tokens.switchTrack, isNot(tokens.accent));
      }
      expect(
        DshTokens.light.selected.computeLuminance(),
        lessThan(DshTokens.light.hover.computeLuminance()),
      );
    },
  );

  test('font families and all reading roles adapt to the platform', () {
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    for (final platform in [
      TargetPlatform.windows,
      TargetPlatform.macOS,
      TargetPlatform.linux,
    ]) {
      debugDefaultTargetPlatformOverride = platform;
      expect(DshTypography.body.fontFamily, DshTypography.familyFor(platform));
      expect(
        DshTypography.code.fontFamily,
        DshTypography.monospaceFor(platform),
      );
      expect(
        DshTypography.body.fontFamilyFallback,
        DshTypography.fallbackFor(platform),
      );
    }
    expect(
      DshTypography.fallbackFor(TargetPlatform.windows),
      contains('Microsoft YaHei'),
    );
    expect(
      DshTypography.fallbackFor(TargetPlatform.macOS),
      contains('PingFang SC'),
    );
    expect(
      DshTypography.fallbackFor(TargetPlatform.linux),
      contains('Noto Sans CJK SC'),
    );
    expect(DshTypography.conversation.fontSize, 15);
    expect(DshTypography.conversation.height, 24 / 15);
    expect(DshTypography.code.fontSize, 13);
    expect(DshTypography.code.height, 20 / 13);
  });

  test('breakpoints keep reading content inside a narrow window', () {
    expect(DshBreakpoints.collapseSidebar(899), isTrue);
    expect(DshBreakpoints.collapseSidebar(900), isFalse);
    expect(DshBreakpoints.overlayWorkbench(1099), isTrue);
    expect(DshBreakpoints.overlayWorkbench(1100), isFalse);
    expect(DshBreakpoints.readingWidth(600), 600);
    expect(DshBreakpoints.readingWidth(1600), 920);
  });

  testWidgets('theme extension is shared by the compatibility palette', (
    tester,
  ) async {
    final custom = DshTokens.light.copyWith(
      accent: Colors.purple,
      selected: Colors.green,
      touchMode: true,
    );
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(extensions: [custom]),
        home: Builder(
          builder: (context) {
            expect(DshColors(context).blue, Colors.purple);
            expect(DshColors(context).selected, Colors.green);
            expect(DshTokens.of(context).controlHeight(context), 48);
            return const SizedBox();
          },
        ),
      ),
    );
  });

  testWidgets('hover preserves selection and disabled controls reject focus', (
    tester,
  ) async {
    const ordinary = ValueKey('ordinary-control');
    const selected = ValueKey('selected-control');
    const disabled = ValueKey('disabled-control');
    final selectedFocus = FocusNode();
    final disabledFocus = FocusNode();
    addTearDown(selectedFocus.dispose);
    addTearDown(disabledFocus.dispose);
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: Row(
            children: [
              DshButton(
                key: ordinary,
                onPressed: () {},
                child: const Text('普通操作'),
              ),
              DshButton(
                key: selected,
                active: true,
                focusNode: selectedFocus,
                onPressed: () {},
                child: const Text('当前选择'),
              ),
              DshButton(
                key: disabled,
                focusNode: disabledFocus,
                child: const Text('不可用操作'),
              ),
              DshIcon(DshIcons.close.data, label: '不可用图标'),
              const DshSwitch(value: false, onChanged: null),
            ],
          ),
        ),
      ),
    );
    Color? surface(Key key) {
      final containers = tester.widgetList<Container>(
        find.descendant(of: find.byKey(key), matching: find.byType(Container)),
      );
      for (final container in containers) {
        final decoration = container.decoration;
        if (decoration is BoxDecoration && decoration.color != null) {
          return decoration.color;
        }
      }
      return null;
    }

    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    addTearDown(mouse.removePointer);
    await mouse.addPointer(location: Offset.zero);
    await mouse.moveTo(tester.getCenter(find.byKey(ordinary)));
    await tester.pumpAndSettle();
    expect(surface(ordinary), DshTokens.light.hover);
    expect(surface(selected), DshTokens.light.selected);
    await mouse.moveTo(tester.getCenter(find.byKey(selected)));
    await tester.pumpAndSettle();
    expect(surface(selected), DshTokens.light.selected);
    await tester.sendKeyEvent(LogicalKeyboardKey.tab);
    selectedFocus.requestFocus();
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<ShadDecorator>(
            find.descendant(
              of: find.byKey(selected),
              matching: find.byType(ShadDecorator),
            ),
          )
          .focused,
      isTrue,
    );
    disabledFocus.requestFocus();
    await tester.pump();
    expect(disabledFocus.hasFocus, isFalse);
    await tester.tap(find.byKey(disabled), warnIfMissed: false);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    final disabledButton = tester.widget<ShadButton>(
      find.descendant(
        of: find.byKey(disabled),
        matching: find.byType(ShadButton),
      ),
    );
    expect(disabledButton.enabled, isFalse);
    expect(
      tester
          .widget<ShadButton>(
            find.descendant(
              of: find.byType(DshIcon),
              matching: find.byType(ShadButton),
            ),
          )
          .enabled,
      isFalse,
    );
    expect(tester.widget<ShadSwitch>(find.byType(ShadSwitch)).enabled, isFalse);
    expect(tester.takeException(), isNull);
  });

  for (final scale in [1.0, 1.25, 1.5, 2.0]) {
    testWidgets('controls preserve content at ${scale}x text scale', (
      tester,
    ) async {
      await tester.pumpWidget(
        ShadApp(
          home: MediaQuery(
            data: MediaQueryData(textScaler: TextScaler.linear(scale)),
            child: Scaffold(
              body: Center(
                child: SizedBox(
                  width: 500,
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      DshButton(onPressed: () {}, child: const Text('保存修改')),
                      DshField(hint: '搜索会话', prefix: DshIcons.search.data),
                      DshSelect<String>(
                        options: const {'session': '会话'},
                        value: 'session',
                        onChanged: (_) {},
                      ),
                      DshIcon(
                        DshIcons.close.data,
                        label: '关闭',
                        size: 24,
                        onPressed: () {},
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final expectedHeight = 22 * scale + 14;
      expect(
        tester.getSize(find.byType(DshButton)).height,
        greaterThanOrEqualTo(expectedHeight),
      );
      expect(
        tester.getSize(find.byType(TextField)).height,
        greaterThanOrEqualTo(expectedHeight),
      );
      expect(
        tester.getSize(find.byType(DshSelect<String>)).height,
        greaterThanOrEqualTo(expectedHeight),
      );
      expect(
        tester.getSize(find.byType(DshIcon)).height,
        greaterThanOrEqualTo(36),
      );
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets(
    'loading button rejects duplicate activation and external focus works',
    (tester) async {
      final focus = FocusNode();
      addTearDown(focus.dispose);
      var activations = 0;
      Widget build(bool loading) => ShadApp(
        home: Scaffold(
          body: Center(
            child: DshButton(
              focusNode: focus,
              loading: loading,
              onPressed: () => activations++,
              child: const Text('保存'),
            ),
          ),
        ),
      );
      await tester.pumpWidget(build(false));
      focus.requestFocus();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(activations, 1);
      await tester.pumpWidget(build(true));
      // Disabled controls intentionally do not participate in hit testing.
      await tester.tap(find.text('保存'), warnIfMissed: false);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(activations, 1);
    },
  );

  testWidgets(
    'reduced motion honors both animation and navigation preferences',
    (tester) async {
      for (final data in [
        const MediaQueryData(disableAnimations: true),
        const MediaQueryData(accessibleNavigation: true),
        const MediaQueryData(),
      ]) {
        await tester.pumpWidget(
          MediaQuery(
            data: data,
            child: Builder(
              builder: (context) {
                expect(
                  DshMotion.duration(context, DshMotion.panel),
                  data.disableAnimations || data.accessibleNavigation
                      ? Duration.zero
                      : DshMotion.panel,
                );
                return const SizedBox();
              },
            ),
          ),
        );
      }
    },
  );

  testWidgets('every semantic vector loads at desktop sizes in both themes', (
    tester,
  ) async {
    expect(
      DshIcons.values.map((icon) => icon.data).toSet().length,
      DshIcons.values.length,
    );
    for (final brightness in Brightness.values) {
      for (final size in [16.0, 20.0, 24.0]) {
        await tester.pumpWidget(
          ShadApp(
            themeMode: brightness == Brightness.dark
                ? ThemeMode.dark
                : ThemeMode.light,
            home: Scaffold(
              body: Wrap(
                children: [
                  for (final icon in DshIcons.values)
                    DshGlyph(icon.data, size: size),
                ],
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(find.byType(SvgPicture), findsNWidgets(DshIcons.values.length));
        expect(tester.takeException(), isNull);
      }
    }
    for (final path in {
      ...DshIcons.values.map((icon) => icon.asset),
      ...dshDynamicIconAssets,
    }) {
      final svg = await rootBundle.loadString(path);
      expect(svg, contains('<svg'));
      expect(svg, contains('currentColor'));
      expect(svg, isNot(contains('data:image')));
    }
  });

  test('unknown semantic identity has a visible fallback', () {
    expect(
      DshIcons.assetFor(const IconData(0xffff, fontFamily: 'DshVectorIcons')),
      DshIcons.unknown.asset,
    );
    expect(DshIcons.assetFor(Icons.add), isNull);
  });
}
