import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/error.dart';
import 'package:dsh_desktop/design/shortcuts.dart';
import 'package:dsh_desktop/features/command_palette.dart';
import 'package:dsh_desktop/l10n/zh.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class NavigationController extends DesktopController {
  NavigationController() : super(DesktopPreferences(writer: (_) async {}));
  @override
  bool get connected => true;
  final selections = <String>[];
  @override
  Future<void> select(
    String id, {
    bool adoptDraft = false,
    bool loadModels = true,
  }) async {
    selections.add(id);
    selectedId = id;
    emit();
  }
}

Future<void> chord(
  WidgetTester tester,
  LogicalKeyboardKey key, {
  bool control = false,
  bool shift = false,
}) async {
  if (control) await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
  if (shift) await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
  await tester.sendKeyEvent(key);
  if (shift) await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
  if (control) await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
  await tester.pumpAndSettle();
}

void main() {
  test(
    'desktop navigation defaults retain explicit bindings across upgrade',
    () {
      final c = NavigationController();
      c.preferences.layout['shortcuts'] = {
        'search': encodeShortcut(
          const SingleActivator(LogicalKeyboardKey.period, control: true),
        ),
      };
      final configured = configuredShortcuts(c);
      expect(configured.entries.first.key, 'search');
      expect(shortcutConflict(configured), contains('停止当前执行'));
      expect(defaultShortcuts['focus-next']!.trigger, LogicalKeyboardKey.f6);
      expect(defaultShortcuts['cycle-next']!.control, isTrue);
      expect(defaultShortcuts['session-9']!.trigger, LogicalKeyboardKey.digit9);
      final oldPlatform = debugDefaultTargetPlatformOverride;
      try {
        debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
        expect(defaultShortcuts['stop']!.meta, isTrue);
        expect(defaultShortcuts['session-1']!.meta, isTrue);
        expect(defaultShortcuts['cycle-next']!.control, isTrue);
      } finally {
        debugDefaultTargetPlatformOverride = oldPlatform;
        c.dispose();
      }
    },
  );

  test(
    'structured failures preserve unknown outcomes and redact credentials',
    () {
      final unknown = DshError.describe(
        DshException(
          'timeout',
          'request failed',
          outcomeUnknown: true,
          details: {
            'authorization': 'Bearer secret-header',
            'apiKey': 'secret-key',
            'password': 'secret-password',
          },
        ),
      );
      expect(unknown.message, DshZh.outcomeUnknown);
      expect(unknown.retryable, isFalse);
      expect(unknown.code, 'timeout');
      expect(unknown.details, isNot(contains('secret-header')));
      expect(unknown.details, isNot(contains('secret-key')));
      expect(unknown.details, isNot(contains('secret-password')));
      expect(
        DshError.redact(
          'Authorization: Basic dXNlcjpwYXNz\nCookie: sid=secret-cookie',
        ),
        isNot(contains('dXNlcjpwYXNz')),
      );
      expect(
        DshError.redact('Cookie: sid=secret-cookie'),
        isNot(contains('secret-cookie')),
      );
      expect(
        DshError.describe(DshException('cancelled', 'cancelled')).cancelled,
        isTrue,
      );
      expect(
        DshError.describe(DshException('title-conflict', 'conflict')).message,
        DshZh.conflict,
      );
      expect(
        DshError.describe(StateError('internal stack')).message,
        DshZh.unknownError,
      );
      expect(DshError.describe(StateError('连接暂时中断')).message, '连接暂时中断');
      expect(
        DshError.describe('保存失败 (revision-conflict)\n草稿已保留，可读取最新配置后再保存。')
            .message,
        contains('草稿已保留'),
      );
      expect(
        DshError.redact('https://user:secret@example.com/?token=hidden'),
        isNot(contains('secret')),
      );
    },
  );

  testWidgets(
    'technical details start collapsed and uncertain mutations cannot retry',
    (tester) async {
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: DshErrorView(
              error: DshException(
                'timeout',
                'private diagnostics',
                outcomeUnknown: true,
              ),
              onRetry: () {},
              onDismiss: () {},
            ),
          ),
        ),
      );
      expect(find.text(DshZh.outcomeUnknown), findsOneWidget);
      expect(find.text(DshZh.retry), findsNothing);
      expect(find.byType(SelectableText), findsNothing);
      await tester.tap(find.text(DshZh.details));
      await tester.pumpAndSettle();
      expect(find.byType(SelectableText), findsOneWidget);
      expect(find.text(DshZh.copyDetails), findsOneWidget);
    },
  );

  testWidgets(
    'palette searches a thousand sessions and invokes only the keyboard selection',
    (tester) async {
      String? invoked;
      final commands = [
        for (var i = 0; i < 1000; i++)
          DshCommand(
            id: '$i',
            group: DshZh.sessionsGroup,
            title: '会话 $i',
            subtitle: '项目路径',
            icon: Icons.chat_bubble_outline,
            onInvoke: () => invoked = '$i',
          ),
      ];
      await tester.pumpWidget(
        ShadApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () async {
                  final command = await showDialog<DshCommand>(
                    context: context,
                    builder: (_) => DshCommandPalette(commands: commands),
                  );
                  command?.onInvoke();
                },
                child: const Text('打开'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('打开'));
      await tester.pumpAndSettle();
      expect(find.byType(InkWell).evaluate().length, lessThan(30));
      await tester.enterText(find.byType(TextField), '会话 999');
      await tester.pump();
      expect(find.byKey(const ValueKey('command-999')), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(invoked, '999');
    },
  );

  testWidgets('palette keeps IME candidate Enter and skips disabled commands', (
    tester,
  ) async {
    await tester.pumpWidget(
      ShadApp(
        home: DshCommandPalette(
          commands: [
            DshCommand(
              id: 'disabled',
              group: DshZh.commandsGroup,
              title: '不可用',
              icon: Icons.stop,
              enabled: false,
              onInvoke: () {},
            ),
            DshCommand(
              id: 'enabled',
              group: DshZh.commandsGroup,
              title: '执行',
              icon: Icons.play_arrow,
              onInvoke: () {},
            ),
          ],
        ),
      ),
    );
    await tester.pumpAndSettle();
    final field = tester.widget<TextField>(find.byType(TextField));
    field.controller!.value = const TextEditingValue(
      text: '执',
      composing: TextRange(start: 0, end: 1),
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(find.byType(DshCommandPalette), findsOneWidget);
    field.controller!.clear();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.pump();
    final selected = tester.widget<Semantics>(
      find
          .ancestor(
            of: find.byKey(const ValueKey('command-enabled')),
            matching: find.byType(Semantics),
          )
          .first,
    );
    expect(selected.properties.selected, isTrue);
  });

  testWidgets('F6 traverses shell regions and palette Escape restores focus', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1280, 800));
    final c = NavigationController();
    await tester.pumpWidget(DesktopApp(controller: c));
    await tester.pumpAndSettle();
    await chord(tester, LogicalKeyboardKey.f6);
    expect(
      FocusManager.instance.primaryFocus!.ancestors.any(
        (n) => n.debugLabel == 'sidebar-region',
      ),
      isTrue,
    );
    await chord(tester, LogicalKeyboardKey.f6);
    expect(
      FocusManager.instance.primaryFocus!.ancestors.any(
        (n) => n.debugLabel == 'conversation-region',
      ),
      isTrue,
    );
    final previous = FocusManager.instance.primaryFocus;
    await chord(tester, LogicalKeyboardKey.keyK, control: true);
    expect(find.byType(DshCommandPalette), findsOneWidget);
    final paletteFocus = FocusManager.instance.primaryFocus;
    await chord(tester, LogicalKeyboardKey.f6);
    expect(FocusManager.instance.primaryFocus, same(paletteFocus));
    await chord(tester, LogicalKeyboardKey.escape);
    expect(find.byType(DshCommandPalette), findsNothing);
    expect(FocusManager.instance.primaryFocus, same(previous));
    await tester.tap(find.byKey(const Key('open-knowledge')));
    await tester.pumpAndSettle();
    final hiddenComposer = tester.widget<TextField>(
      find.byKey(const Key('prompt-input'), skipOffstage: false),
    );
    expect(hiddenComposer.focusNode!.canRequestFocus, isFalse);
    await chord(tester, LogicalKeyboardKey.keyL, control: true);
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('prompt-input')))
          .focusNode!
          .hasFocus,
      isTrue,
    );
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets('visible-session shortcuts refresh after sidebar is collapsed', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1280, 800));
    final c = NavigationController()
      ..sessions = [
        SessionSummary.fromJson({'sessionId': 'old', 'displayTitle': '旧会话'}),
      ];
    await tester.pumpWidget(DesktopApp(controller: c));
    await tester.pumpAndSettle();
    await chord(tester, LogicalKeyboardKey.keyB, control: true);
    c.sessions = [
      SessionSummary.fromJson({'sessionId': 'new', 'displayTitle': '新会话'}),
    ];
    c.emit();
    await tester.pumpAndSettle();
    await chord(tester, LogicalKeyboardKey.digit1, control: true);
    expect(c.selections, ['new']);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await tester.binding.setSurfaceSize(null);
  });

  testWidgets(
    'shortcut editor remains scrollable in a small window at double text scale',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(720, 520));
      final c = NavigationController();
      await tester.pumpWidget(
        ShadApp(
          builder: (context, child) => MediaQuery(
            data: MediaQuery.of(context)
                .copyWith(textScaler: TextScaler.linear(2)),
            child: child!,
          ),
          home: ShortcutEditor(controller: c),
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      await tester.enterText(find.byType(TextField), DshZh.visibleSession(9));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('shortcut-row-session-9')),
        findsOneWidget,
      );
      expect(find.text(DshZh.save).hitTestable(), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );
}
