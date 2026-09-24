import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/shortcuts.dart';
import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class InMemoryPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class ShellController extends DesktopController {
  ShellController() : super(InMemoryPreferences());
  @override
  bool get connected => true;
}

Future<void> control(WidgetTester tester, LogicalKeyboardKey key) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  await tester.sendKeyEvent(key);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
  await tester.pumpAndSettle();
}

void main() {
  test('session age uses the Web minute, hour and day buckets', () {
    const now = 2000000000000;
    expect(relativeSessionAge(now, nowMillis: now), '刚刚');
    expect(relativeSessionAge(now - 5 * 60000, nowMillis: now), '5分钟');
    expect(relativeSessionAge(now - 17 * 3600000, nowMillis: now), '17小时');
    expect(relativeSessionAge(now - 2 * 86400000, nowMillis: now), '2天');
  });
  testWidgets(
    'selected workspace starts expanded and manual expansion persists',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1280, 800));
      final c = ShellController()
        ..workspaceId = 'current'
        ..selectedId = 'current-task'
        ..workspaces = [
          {
            'workspaceId': 'older',
            'path': r'E:\older',
            'title': '旧工作区',
            'sessionIds': ['old-task'],
          },
          {
            'workspaceId': 'current',
            'path': r'E:\current',
            'title': '当前工作区',
            'sessionIds': ['current-task'],
          },
        ]
        ..sessions = [
          SessionSummary.fromJson({
            'sessionId': 'old-task',
            'cwd': r'E:\older',
            'displayTitle': '旧任务',
          }),
          SessionSummary.fromJson({
            'sessionId': 'current-task',
            'cwd': r'E:\current',
            'displayTitle': '当前任务',
          }),
        ];
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('session-old-task')), findsNothing);
      expect(
        find.byKey(const ValueKey('session-current-task')),
        findsOneWidget,
      );
      expect(tester.getSize(find.byTooltip('视图选项')), const Size(28, 28));
      await tester.tap(find.text('旧工作区'));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('session-old-task')), findsOneWidget);
      expect(object(c.preferences.layout['groupExpansion'])['older'], isTrue);
      await tester.pumpWidget(const SizedBox());
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('session-old-task')), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets(
    'shortcut editor rejects conflicts and persists the selected binding',
    (tester) async {
      final c = ShellController();
      await tester.pumpWidget(ShadApp(home: ShortcutEditor(controller: c)));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Ctrl+B'));
      await tester.pumpAndSettle();
      await control(tester, LogicalKeyboardKey.keyK);
      expect(find.text('此快捷键已绑定其他操作。'), findsOneWidget);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyB);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
      await tester.pumpAndSettle();
      expect(find.text('Ctrl+Alt+B'), findsOneWidget);
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(shortcutLabel(configuredShortcuts(c)['sidebar']!), 'Ctrl+Alt+B');
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
  testWidgets(
    'collapsed rail preserves actions and Ctrl B/Ctrl K/Ctrl L work from input focus',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1280, 800));
      debugDefaultTargetPlatformOverride = TargetPlatform.windows;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final c = ShellController();
      c.subscriptionAccounts = [
        {'name': 'ChatGPT / Codex', 'id': 'openai-codex', 'signedIn': true},
      ];
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      await control(tester, LogicalKeyboardKey.keyB);
      expect(c.preferences.layout['sideOpen'], false);
      await control(tester, LogicalKeyboardKey.keyB);
      expect(c.preferences.layout['sideOpen'], true);
      await tester.tap(find.byKey(const Key('prompt-input')));
      await tester.pump();
      await control(tester, LogicalKeyboardKey.keyB);
      expect(c.preferences.layout['sideOpen'], false);
      expect(find.byTooltip('展开侧边栏'), findsOneWidget);
      expect(find.byTooltip('新建会话'), findsOneWidget);
      expect(find.byTooltip('添加工作区'), findsOneWidget);
      expect(find.byTooltip('ChatGPT / Codex · 已连接'), findsOneWidget);
      expect(find.byTooltip('设置'), findsOneWidget);
      await control(tester, LogicalKeyboardKey.keyK);
      expect(c.preferences.layout['sideOpen'], true);
      final search = find.byWidgetPredicate(
        (w) => w is TextField && w.decoration?.hintText == '搜索会话',
      );
      expect(search, findsOneWidget);
      expect((tester.widget(search) as TextField).focusNode!.hasFocus, true);
      await control(tester, LogicalKeyboardKey.keyL);
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('prompt-input')))
            .focusNode!
            .hasFocus,
        true,
      );
      expect(tester.takeException(), isNull);
      c.selectedId = 'to-clear';
      c.emit();
      await tester.pumpAndSettle();
      await control(tester, LogicalKeyboardKey.keyN);
      expect(c.selectedId, isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
      debugDefaultTargetPlatformOverride = null;
    },
  );
  test('custom shortcut encoding survives preferences roundtrip', () {
    const original = SingleActivator(
      LogicalKeyboardKey.keyP,
      control: true,
      shift: true,
    );
    final restored = decodeShortcut(
      encodeShortcut(original),
      defaultShortcuts['sidebar']!,
    );
    expect(shortcutLabel(restored), 'Ctrl+Shift+P');
  });
  test('Ctrl Enter continues selected numbered draft without submission', () {
    final result = continueNumberedDraft(
      const TextEditingValue(
        text: '  9、检查图标',
        selection: TextSelection.collapsed(offset: 8),
      ),
    );
    expect(result!.text, '  9、检查图标\n  10、 ');
    expect(result.selection.baseOffset, result.text.length);
    expect(
      continueNumberedDraft(
        const TextEditingValue(
          text: '普通段落',
          selection: TextSelection.collapsed(offset: 4),
        ),
      ),
      isNull,
    );
  });
}
