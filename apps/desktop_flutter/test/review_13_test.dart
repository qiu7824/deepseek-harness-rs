import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/features/workspace_tree_row.dart';
import 'package:dsh_desktop/features/conversation/approval_card.dart';
import 'package:dsh_desktop/features/conversation/permission_control.dart';
import 'package:dsh_desktop/features/conversation/session_status.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'workbench_test.dart' show TestController;
import 'settings_fidelity_test.dart' show SettingsController;

void main() {
  testWidgets(
    'first messages start at top while long history keeps lazy viewport',
    (tester) async {
      final c = TestController()..selectedId = 's';
      c.transcript = [
        TranscriptItem(id: 'u', kind: 'user', text: '第一条消息', seq: 1),
      ];
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      final list = find.byType(ListView).first;
      expect(
        tester.getTopLeft(find.byKey(const ValueKey('bubble-u'))).dy -
            tester.getTopLeft(list).dy,
        lessThan(50),
      );
      c.transcript = List.generate(
        100,
        (i) => TranscriptItem(id: '$i', kind: 'assistant', text: '内容 $i'),
      );
      c.messageChanges.value++;
      await tester.pumpAndSettle();
      expect(tester.widget<ListView>(list).shrinkWrap, isFalse);
      expect(find.byType(MessageCard).evaluate().length, lessThan(100));
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );

  testWidgets(
    'workspace has one hover-replaced icon and no permanent add button',
    (tester) async {
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: WorkspaceTreeRow(
              title: '项目',
              path: r'\\?\C:\项目',
              expanded: true,
              active: true,
              onPressed: () {},
              onMenu: (_) {},
            ),
          ),
        ),
      );
      expect(find.byTooltip(r'C:\项目'), findsOneWidget);
      expect(find.byType(DshGlyph), findsOneWidget);
      expect(
        tester.widget<DshGlyph>(find.byType(DshGlyph)).data,
        LucideIcons.folder,
      );
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: const Offset(700, 500));
      await mouse.moveTo(tester.getCenter(find.byType(WorkspaceTreeRow)));
      await tester.pump();
      expect(
        tester.widget<DshGlyph>(find.byType(DshGlyph)).data,
        LucideIcons.chevronDown,
      );
      expect(find.byTooltip('在此工作区新建会话'), findsNothing);
      await mouse.removePointer();
    },
  );

  testWidgets(
    'settings desktop and mobile bounds follow Web breakpoints without sandbox',
    (tester) async {
      final c = SettingsController();
      for (final size in [const Size(1440, 900), const Size(700, 650)]) {
        await tester.binding.setSurfaceSize(size);
        await tester.pumpWidget(
          ShadApp(
            home: MediaQuery(
              data: MediaQueryData(size: size),
              child: SettingsShell(controller: c),
            ),
          ),
        );
        await tester.pumpAndSettle();
        final rect = tester.getRect(
          find.byKey(const ValueKey('settings-panel')),
        );
        expect(rect.width, size.width > 768 ? 1040 : size.width);
        expect(rect.height, size.width > 768 ? 800 : size.height);
        expect(find.text('Windows 沙箱'), findsNothing);
        expect(tester.takeException(), isNull);
      }
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await c.api.close();
      await tester.binding.setSurfaceSize(null);
    },
  );

  testWidgets(
    'approval scrolls details but retains visible actions and exact single-use scope',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(440, 650));
      final c = TestController();
      final frame = HostFrame.fromJson({
        'type': 'server-request',
        'rpcId': 'r',
        'payload': {
          'type': 'approval/requested',
          'sessionId': 's',
          'approvalId': 'a',
          'toolName': 'pwsh',
          'reason': '需要执行命令。' * 200,
          'rememberable': false,
        },
      });
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(
              child: ApprovalCard(controller: c, frame: frame),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        tester.getSize(find.byKey(const ValueKey('approval-card'))).height,
        lessThan(400),
      );
      final always = tester.widget<DshButton>(
        find.ancestor(of: find.text('始终允许'), matching: find.byType(DshButton)),
      );
      expect(always.onPressed, isNull);
      await tester.tap(find.text('允许一次'));
      await tester.pump();
      expect(c.answers.single['outcome'], 'allowed-once');
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );

  testWidgets('goal edits inline at the same bar and Escape preserves goal', (
    tester,
  ) async {
    final c = TestController()..selectedId = 's';
    c.projectionWindow.apply('goal', {
      'goal': {'id': 'g', 'revision': 1, 'phase': 'active', 'objective': '原目标'},
    }, 1);
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: Center(
            child: SizedBox(width: 400, child: ProgressDock(controller: c)),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final before = tester.getRect(
      find.byKey(const ValueKey('goal-status-bar')),
    );
    await tester.tap(find.byTooltip('编辑目标'));
    await tester.pumpAndSettle();
    expect(find.byType(Dialog), findsNothing);
    expect(
      tester.getRect(find.byKey(const ValueKey('goal-status-bar'))),
      before,
    );
    await tester.enterText(
      find.byKey(const ValueKey('goal-objective-input')),
      '未保存',
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('原目标'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });

  testWidgets('full access confirmation requires acknowledgement', (
    tester,
  ) async {
    await tester.pumpWidget(
      const ShadApp(home: Scaffold(body: FullAccessConfirmation())),
    );
    DshButton confirm() => tester.widget<DshButton>(
      find.ancestor(
        of: find.text('启用 Full access'),
        matching: find.byType(DshButton),
      ),
    );
    expect(confirm().onPressed, isNull);
    await tester.tap(find.byType(Checkbox));
    await tester.pumpAndSettle();
    expect(confirm().onPressed, isNotNull);
  });

  testWidgets(
    'session context menu copies actual ID and extra shell controls are absent',
    (tester) async {
      String? clipboard;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            clipboard = (call.arguments as Map)['text'] as String;
          }
          return null;
        },
      );
      final c = TestController()
        ..selectedId = 's'
        ..sessions = [
          SessionSummary.fromJson({
            'sessionId': 's',
            'cwd': 'C:/project',
            'projections': {
              'values': {'title': '会话'},
            },
          }),
        ];
      await tester.binding.setSurfaceSize(const Size(1400, 900));
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      expect(find.text('协作'), findsNothing);
      expect(find.byTooltip('协作'), findsOneWidget);
      expect(find.byTooltip('下载会话日志'), findsOneWidget);
      c.teamSettings = {'showButton': false};
      c.emit();
      await tester.pumpAndSettle();
      expect(find.byTooltip('协作'), findsNothing);
      expect(find.byTooltip('本机服务连接'), findsNothing);
      await tester.tap(
        find.byKey(const ValueKey('session-s')),
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('复制对话 ID'));
      await tester.pumpAndSettle();
      expect(clipboard, 's');
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      );
      await tester.binding.setSurfaceSize(null);
    },
  );
}
