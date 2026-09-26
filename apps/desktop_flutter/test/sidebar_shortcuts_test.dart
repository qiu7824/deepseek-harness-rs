import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/shortcuts.dart';
import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/gestures.dart';
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
  final restored = <String>[];
  @override
  Future<void> archive(String id, {bool restore = false}) async {
    if (restore) {
      restored.add(id);
      archivedSessionIds.remove(id);
      emit();
    }
  }
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
      expect(find.byTooltip('展开侧边栏 · Ctrl+B'), findsOneWidget);
      expect(find.byTooltip('新建会话 · Ctrl+N'), findsOneWidget);
      expect(find.byTooltip('添加工作区'), findsOneWidget);
      expect(find.byTooltip('ChatGPT / Codex · 已连接'), findsOneWidget);
      expect(find.byTooltip('设置 · Ctrl+,'), findsOneWidget);
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
  testWidgets(
    'archive filters persist, include complete data and hide empty archive groups',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1280, 900));
      final c = ShellController()
        ..workspaces = [
          {
            'workspaceId': 'mixed',
            'path': r'E:\mixed',
            'title': '混合工作区',
            'sessionIds': ['active', 'archived', 'blank'],
          },
          {
            'workspaceId': 'active-only',
            'path': r'E:\active',
            'title': '活动工作区',
            'sessionIds': ['other'],
          },
          {
            'workspaceId': 'empty',
            'path': r'E:\empty',
            'title': '空工作区',
            'sessionIds': [],
          },
        ]
        ..sessions = [
          SessionSummary.fromJson({
            'sessionId': 'active',
            'cwd': r'E:\mixed',
            'displayTitle': '待办任务',
          }),
          SessionSummary.fromJson({
            'sessionId': 'archived',
            'cwd': r'E:\mixed',
            'displayTitle': '归档任务',
          }),
          SessionSummary.fromJson({
            'sessionId': 'blank',
            'cwd': r'E:\mixed',
            'displayTitle': '空白归档',
            'blank': true,
          }),
          SessionSummary.fromJson({
            'sessionId': 'other',
            'cwd': r'E:\active',
            'displayTitle': '其他待办',
          }),
          SessionSummary.fromJson({
            'sessionId': 'loose',
            'cwd': r'E:\loose',
            'displayTitle': '独立归档',
          }),
        ]
        ..archivedSessionIds = {'archived', 'blank', 'loose'};
      c.preferences.layout['groupExpansion'] = {
        'mixed': true,
        'active-only': true,
      };
      addTearDown(() async {
        await tester.pumpWidget(const SizedBox());
        c.dispose();
        await tester.binding.setSurfaceSize(null);
      });
      Future<void> filter(String label) async {
        await tester.tap(find.byTooltip('视图选项'));
        await tester.pumpAndSettle();
        await tester.tap(
          find.ancestor(
            of: find.text(label),
            matching: find.byType(CheckedPopupMenuItem<String>),
          ),
        );
        await tester.pumpAndSettle();
      }

      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('session-active')), findsOneWidget);
      expect(find.byKey(const ValueKey('session-archived')), findsNothing);
      expect(find.text('空工作区'), findsOneWidget);
      await filter('全部对话');
      for (final id in ['active', 'archived', 'blank', 'other', 'loose']) {
        expect(find.byKey(ValueKey('session-$id')), findsOneWidget);
      }
      await filter('仅显示已归档');
      expect(c.preferences.layout['archiveFilter'], 'archived');
      expect(c.sessions, hasLength(5));
      expect(find.byKey(const ValueKey('session-active')), findsNothing);
      expect(find.text('活动工作区'), findsNothing);
      expect(find.text('空工作区'), findsNothing);
      expect(find.byKey(const ValueKey('session-blank')), findsOneWidget);
      await control(tester, LogicalKeyboardKey.keyK);
      final search = find.byWidgetPredicate(
        (w) => w is TextField && w.decoration?.hintText == '搜索会话',
      );
      await tester.enterText(search, '待办');
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('session-active')), findsNothing);
      expect(find.text('混合工作区'), findsNothing);
      await filter('全部对话');
      expect(find.byKey(const ValueKey('session-active')), findsOneWidget);
      expect(find.byKey(const ValueKey('session-archived')), findsNothing);
      await tester.enterText(search, '');
      await tester.pumpAndSettle();
      await filter('仅显示已归档');
      await tester.pumpWidget(const SizedBox());
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('session-archived')), findsOneWidget);
      expect(find.byKey(const ValueKey('session-active')), findsNothing);
      final pointer = await tester.startGesture(
        tester.getCenter(find.byKey(const ValueKey('session-archived'))),
        kind: PointerDeviceKind.mouse,
        buttons: kSecondaryMouseButton,
      );
      await pointer.up();
      await tester.pumpAndSettle();
      expect(find.text('恢复归档'), findsOneWidget);
      expect(find.text('归档'), findsNothing);
      await tester.tap(find.text('恢复归档'));
      await tester.pumpAndSettle();
      expect(c.restored, ['archived']);
      expect(find.byKey(const ValueKey('session-archived')), findsNothing);
      await filter('隐藏已归档');
      expect(find.byKey(const ValueKey('session-archived')), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'shortcut search retains hidden bindings and sidebar reflects custom keys',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1280, 800));
      final c = ShellController();
      addTearDown(() async {
        await tester.pumpWidget(const SizedBox());
        c.dispose();
        await tester.binding.setSurfaceSize(null);
      });
      await tester.pumpWidget(ShadApp(home: ShortcutEditor(controller: c)));
      await tester.pumpAndSettle();
      final search = find.descendant(
        of: find.byKey(const Key('shortcut-search')),
        matching: find.byType(TextField),
      );
      await tester.enterText(search, 'ctrl + n');
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('shortcut-row-new')), findsOneWidget);
      expect(find.byKey(const ValueKey('shortcut-row-sidebar')), findsNothing);
      await tester.tap(find.text('Ctrl+N'));
      await control(tester, LogicalKeyboardKey.keyB);
      expect(find.text('此快捷键已绑定其他操作。'), findsOneWidget);
      await control(tester, LogicalKeyboardKey.keyP);
      await tester.enterText(search, '新建');
      await tester.pumpAndSettle();
      expect(find.text('Ctrl+P'), findsOneWidget);
      await tester.enterText(search, 'does-not-exist');
      await tester.pumpAndSettle();
      expect(find.text('未找到匹配的快捷键'), findsOneWidget);
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(shortcutLabel(configuredShortcuts(c)['new']!), 'Ctrl+P');
      expect(shortcutLabel(configuredShortcuts(c)['sidebar']!), 'Ctrl+B');
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<Text>(find.byKey(const ValueKey('sidebar-shortcut-new')))
            .data,
        'Ctrl+P',
      );
      c.preferences.layout['shortcuts'] = {
        'new': encodeShortcut(
          const SingleActivator(LogicalKeyboardKey.keyO, control: true),
        ),
      };
      c.emit();
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<Text>(find.byKey(const ValueKey('sidebar-shortcut-new')))
            .data,
        'Ctrl+O',
      );
      await control(tester, LogicalKeyboardKey.keyB);
      expect(find.byTooltip('新建会话 · Ctrl+O'), findsOneWidget);
      expect(tester.takeException(), isNull);
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
