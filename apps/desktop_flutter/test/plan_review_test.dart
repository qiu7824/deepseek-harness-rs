import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/plan_review.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/conversation/question_flow.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/interactions.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class PlanPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class PlanController extends DesktopController {
  PlanController() : super(PlanPreferences());
  final answers = <Json>[];
  int cancelled = 0;
  bool online = true, fail = false;
  Completer<void>? pendingAnswer;
  @override
  bool get connected => online;
  @override
  Future<void> answer(HostFrame frame, Json value) async {
    answers.add(value);
    if (fail) throw StateError('response rejected');
    await pendingAnswer?.future;
  }

  @override
  Future<void> cancelQuestion(HostFrame frame) async {
    cancelled++;
  }
}

Json question({bool multi = false, int choices = 2, bool detail = true}) => {
  'id': 'plan-review',
  'question': '是否执行此计划？',
  'multiSelect': multi,
  if (detail) 'detail': '# 实施计划\n\n- 检查现状\n- 修改并验证',
  'intent': {'kind': 'plan-review', 'approve': 'Approve'},
  'options': [
    {'label': 'Approve', 'description': 'Execute the plan'},
    if (choices >= 2) {'label': 'Keep planning'},
    if (choices >= 3) {'label': 'Defer'},
  ],
};
HostFrame frame(List<Json> questions) => HostFrame.fromJson({
  'type': 'server-request',
  'rpcId': 'review',
  'payload': {
    'type': 'question/requested',
    'sessionId': 's',
    'questions': questions,
  },
});
Future<void> mount(
  WidgetTester tester,
  PlanController c,
  List<Json> questions, {
  ThemeMode mode = ThemeMode.light,
}) async {
  await tester.pumpWidget(
    ShadApp(
      themeMode: mode,
      darkTheme: ShadThemeData(
        brightness: Brightness.dark,
        colorScheme: const ShadZincColorScheme.dark(),
      ),
      home: Scaffold(
        body: Center(
          child: SizedBox(
            width: 680,
            child: InteractionCard(controller: c, frame: frame(questions)),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Future<void> done(WidgetTester tester, PlanController c) async {
  await tester.pumpWidget(const SizedBox());
  c.dispose();
  expect(tester.takeException(), isNull);
}

void main() {
  testWidgets(
    'intent specialization never removes extra options, batches, or multi-select',
    (tester) async {
      final c = PlanController();
      for (final questions in [
        [question(multi: true)],
        [question(choices: 3)],
        [question(detail: false)],
        [question(), question()..['id'] = 'second'],
        [
          question()
            ..['intent'] = {'kind': 'plan-review', 'approve': 'missing'},
        ],
      ]) {
        await mount(tester, c, questions);
        expect(find.byType(PlanReviewCard), findsNothing);
        expect(find.byType(QuestionFlow), findsOneWidget);
        await tester.pumpWidget(const SizedBox());
      }
      await done(tester, c);
    },
  );
  for (final choice in ['确认执行', '拒绝', '去聊天里说']) {
    testWidgets('plan decision $choice uses the original wire semantics', (
      tester,
    ) async {
      final c = PlanController();
      await mount(tester, c, [question()]);
      expect(find.text('计划待审'), findsOneWidget);
      expect(c.answers, isEmpty);
      await tester.tap(find.text(choice));
      await tester.pump();
      if (choice == '去聊天里说') {
        expect(c.cancelled, 1);
        expect(c.answers, isEmpty);
      } else {
        expect(c.cancelled, 0);
        expect(c.answers.single, {
          'sessionId': 's',
          'answer': {
            'answers': [
              {
                'id': 'plan-review',
                'selected': [choice == '确认执行' ? 'Approve' : 'Keep planning'],
              },
            ],
          },
        });
      }
      await done(tester, c);
    });
  }
  testWidgets(
    'failure preserves plan and permits retry; in-flight decision cannot repeat',
    (tester) async {
      final c = PlanController()..fail = true;
      await mount(tester, c, [question()]);
      await tester.tap(find.text('确认执行'));
      await tester.pump();
      expect(find.textContaining('response rejected'), findsOneWidget);
      c.fail = false;
      c.pendingAnswer = Completer<void>();
      final repeated = tester
          .widget<DshButton>(find.byKey(const Key('approve-plan')))
          .onPressed!;
      await tester.tap(find.text('确认执行'));
      await tester.pump();
      repeated();
      await tester.pump();
      expect(c.answers, hasLength(2));
      c.pendingAnswer!.complete();
      await tester.pump();
      await done(tester, c);
    },
  );
  testWidgets(
    'long plan stays height bounded in both themes and one-option plan has no invented refusal',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(360, 600));
      final c = PlanController();
      for (final mode in [ThemeMode.light, ThemeMode.dark]) {
        await mount(tester, c, [
          question(choices: 1)..['detail'] = '# 长计划\n\n${'- 一个计划步骤\n' * 150}',
        ], mode: mode);
        expect(
          tester.getSize(find.byKey(const Key('plan-review-card'))).height,
          lessThanOrEqualTo(360),
        );
        expect(find.text('拒绝'), findsNothing);
        expect(find.text('确认执行'), findsOneWidget);
        final color = mode == ThemeMode.dark
            ? const Color(0xff27241f)
            : const Color(0xfffef5e7);
        expect(
          find.byWidgetPredicate((w) => w is Container && w.color == color),
          findsOneWidget,
        );
      }
      await done(tester, c);
      await tester.binding.setSurfaceSize(null);
    },
  );
}
