import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/design/typography.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

Widget _app(Widget child, {bool dark = false, double scale = 1}) => ShadApp(
  themeMode: dark ? ThemeMode.dark : ThemeMode.light,
  materialThemeBuilder: (_, theme) => theme.copyWith(
    tooltipTheme: DshTooltip.theme(dark ? DshTokens.dark : DshTokens.light),
  ),
  home: MediaQuery(
    data: MediaQueryData(textScaler: TextScaler.linear(scale)),
    child: Scaffold(body: Center(child: child)),
  ),
);

Future<TestGesture> _hover(WidgetTester tester, Finder finder) async {
  final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
  addTearDown(mouse.removePointer);
  await mouse.addPointer(location: Offset.zero);
  await mouse.moveTo(tester.getCenter(finder));
  await tester.pump();
  return mouse;
}

void main() {
  testWidgets('icon hint appears after hover delay and closes on departure', (
    tester,
  ) async {
    const key = Key('hinted-icon');
    await tester.pumpWidget(
      _app(
        DshIcon(
          DshIcons.search.data,
          key: key,
          label: '搜索会话',
          shortcut: 'Ctrl+K',
          onPressed: () {},
        ),
      ),
    );
    final mouse = await _hover(tester, find.byKey(key));
    await tester.pump(const Duration(milliseconds: 499));
    expect(find.text('搜索会话 (Ctrl+K)'), findsNothing);
    await tester.pump(const Duration(milliseconds: 1));
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('搜索会话 (Ctrl+K)'), findsOneWidget);
    await mouse.moveTo(Offset.zero);
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpAndSettle();
    expect(find.text('搜索会话 (Ctrl+K)'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('disabled icon retains explanation and does not activate', (
    tester,
  ) async {
    const key = Key('disabled-icon');
    await tester.pumpWidget(
      _app(DshIcon(DshIcons.send.data, key: key, label: '发送消息')),
    );
    await _hover(tester, find.byKey(key));
    await tester.pump(const Duration(milliseconds: 700));
    await tester.pumpAndSettle();
    expect(find.text('发送消息'), findsOneWidget);
    expect(
      tester
          .widget<ShadButton>(
            find.descendant(
              of: find.byKey(key),
              matching: find.byType(ShadButton),
            ),
          )
          .enabled,
      isFalse,
    );
    await tester.tap(find.byKey(key), warnIfMissed: false);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets('tip cannot capture the click which opens a menu', (
    tester,
  ) async {
    const key = Key('open-menu');
    var opened = 0;
    await tester.pumpWidget(
      _app(
        Builder(
          builder: (context) => DshButton(
            key: key,
            tooltip: '选择一个模型',
            onPressed: () {
              opened++;
              showMenu<String>(
                context: context,
                position: const RelativeRect.fromLTRB(100, 100, 100, 100),
                items: const [PopupMenuItem(value: 'one', child: Text('模型一'))],
              );
            },
            child: const Text('当前模型'),
          ),
        ),
      ),
    );
    await _hover(tester, find.byKey(key));
    await tester.pump(const Duration(milliseconds: 700));
    await tester.pumpAndSettle();
    expect(find.text('选择一个模型'), findsOneWidget);
    await tester.tap(find.byKey(key));
    await tester.pumpAndSettle();
    expect(opened, 1);
    expect(find.text('模型一'), findsOneWidget);
    expect(find.text('选择一个模型'), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('模型一'), findsNothing);
    expect(find.text('选择一个模型'), findsNothing);
  });

  testWidgets('keyboard activation clears an already visible hint', (
    tester,
  ) async {
    final focus = FocusNode();
    addTearDown(focus.dispose);
    var activations = 0;
    await tester.pumpWidget(
      _app(
        DshButton(
          focusNode: focus,
          tooltip: '新建会话 · Ctrl+N',
          onPressed: () => activations++,
          child: const Text('新会话'),
        ),
      ),
    );
    await _hover(tester, find.byType(DshButton));
    await tester.pump(const Duration(milliseconds: 700));
    await tester.pumpAndSettle();
    expect(find.text('新建会话 · Ctrl+N'), findsOneWidget);
    focus.requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    expect(activations, 1);
    expect(find.text('新建会话 · Ctrl+N'), findsNothing);
  });

  for (final dark in [false, true]) {
    testWidgets(
      'hint remains legible at 200% scale in ${dark ? 'dark' : 'light'}',
      (tester) async {
        await tester.pumpWidget(
          _app(
            DshButton(
              tooltip: '显示工作台 · Ctrl+Shift+B',
              onPressed: () {},
              child: const Text('工作台'),
            ),
            dark: dark,
            scale: 2,
          ),
        );
        await _hover(tester, find.byType(DshButton));
        await tester.pump(const Duration(milliseconds: 700));
        await tester.pumpAndSettle();
        expect(find.text('显示工作台 · Ctrl+Shift+B'), findsOneWidget);
        final tooltip = tester.widget<Tooltip>(find.byType(Tooltip));
        expect(tooltip.ignorePointer, isTrue);
        final theme = TooltipTheme.of(tester.element(find.byType(Tooltip)));
        expect(theme.textStyle?.fontFamily, DshTypography.family);
        expect(theme.textStyle?.fontFamilyFallback, DshTypography.fallback);
        final tokens = dark ? DshTokens.dark : DshTokens.light;
        expect(theme.textStyle?.color, tokens.base);
        expect((theme.decoration as BoxDecoration).color, tokens.text);
        expect(tester.takeException(), isNull);
      },
    );
  }
}
