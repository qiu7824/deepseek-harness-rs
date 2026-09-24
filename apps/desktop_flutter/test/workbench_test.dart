import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/interactions.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart' show ShadButton, ShadApp;

class MemoryPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class TestController extends DesktopController {
  TestController() : super(MemoryPreferences());
  bool online = true;
  List<Json> answers = [];
  @override
  bool get connected => online;
  @override
  Future<void> answer(HostFrame frame, Json value) async {
    answers.add(value);
  }
}

void main() {
  testWidgets('workbench renders narrow and wide layouts without overflow', (
    tester,
  ) async {
    final c = TestController();
    c.sessions = [
      SessionSummary.fromJson({
        'sessionId': 's',
        'cwd': r'D:\项目\客户端',
        'projections': {
          'values': {'title': '开发客户端'},
        },
      }),
    ];
    c.selectedId = 's';
    c.transcript = [
      TranscriptItem(
        id: '1',
        kind: 'assistant',
        text: '## 开发进度\n\n客户端已连接。\n\n```rust\nfn main() {}\n```',
      ),
    ];
    for (final size in [
      const Size(1440, 900),
      const Size(800, 700),
      const Size(540, 650),
    ]) {
      await tester.binding.setSurfaceSize(size);
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      expect(find.byKey(const Key('prompt-input')), findsOneWidget);
    }
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await tester.binding.setSurfaceSize(null);
  });
  testWidgets('approval requires a deliberate click and sends exact scope', (
    tester,
  ) async {
    final c = TestController();
    final frame = HostFrame.fromJson({
      'type': 'server-request',
      'rpcId': 'r',
      'payload': {
        'type': 'approval/requested',
        'sessionId': 's',
        'approvalId': 'a',
        'toolName': 'read',
        'rememberable': false,
      },
    });
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: InteractionCard(controller: c, frame: frame),
        ),
      ),
    );
    expect(c.answers, isEmpty);
    expect(find.text('始终允许此权限'), findsNothing);
    await tester.tap(find.text('允许一次'));
    await tester.pump();
    expect(c.answers.single, {
      'sessionId': 's',
      'approvalId': 'a',
      'outcome': 'allowed-once',
    });
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });
  testWidgets('question selections and free text submit as one batch', (
    tester,
  ) async {
    final c = TestController();
    final frame = HostFrame.fromJson({
      'type': 'server-request',
      'rpcId': 'r',
      'payload': {
        'type': 'question/requested',
        'sessionId': 's',
        'questions': [
          {
            'id': 'q1',
            'question': '选择方案',
            'options': [
              {'label': 'A'},
              {'label': 'B'},
            ],
          },
          {'id': 'q2', 'question': '说明'},
        ],
      },
    });
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: InteractionCard(controller: c, frame: frame),
        ),
      ),
    );
    final submit = find.byKey(const Key('question-continue'));
    expect(tester.widget<DshButton>(submit).onPressed, isNull);
    await tester.tap(find.text('B'));
    await tester.pump();
    expect(tester.widget<DshButton>(submit).onPressed, isNull);
    await tester.enterText(find.byKey(const ValueKey('answer-q2')), '中文说明');
    await tester.pump();
    await tester.tap(submit);
    await tester.pump();
    expect(c.answers.single, {
      'sessionId': 's',
      'answer': {
        'answers': [
          {
            'id': 'q1',
            'selected': ['B'],
          },
          {'id': 'q2', 'selected': <String>[], 'custom': '中文说明'},
        ],
      },
    });
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });
  testWidgets('offline task preserves editable draft and disables send', (
    tester,
  ) async {
    final c = TestController()..online = false;
    c.selectedId = 's';
    c.preferences.drafts['s'] = '未发送草稿';
    await tester.binding.setSurfaceSize(const Size(1100, 760));
    await tester.pumpWidget(DesktopApp(controller: c));
    await tester.pump();
    expect(find.text('未发送草稿'), findsOneWidget);
    expect(
      tester
          .widget<ShadButton>(find.byKey(const Key('send-message')))
          .onPressed,
      isNull,
    );
    await tester.enterText(find.byKey(const Key('prompt-input')), '新草稿');
    expect(c.preferences.drafts['s'], '新草稿');
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await tester.binding.setSurfaceSize(null);
  });
}
