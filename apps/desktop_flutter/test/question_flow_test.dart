import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/interactions.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class QuestionPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class QuestionController extends DesktopController {
  QuestionController() : super(QuestionPreferences());
  final answers = <Json>[];
  int cancellations = 0;
  bool fail = false;
  @override
  bool get connected => true;
  @override
  Future<void> answer(HostFrame frame, Json value) async {
    if (fail) throw StateError('连接暂时中断');
    answers.add(value);
  }

  @override
  Future<void> cancelQuestion(HostFrame frame) async {
    cancellations++;
  }
}

HostFrame request({bool multi = false, int options = 2}) => HostFrame.fromJson({
  'type': 'server-request',
  'rpcId': 'q',
  'payload': {
    'type': 'question/requested',
    'sessionId': 's',
    'questions': [
      {
        'id': 'one',
        'question': '选择方案',
        'multiSelect': multi,
        'options': [
          for (var i = 0; i < options; i++) {'label': i == 0 ? 'A（推荐）' : 'B$i'},
        ],
      },
      {'id': 'two', 'question': '补充说明'},
    ],
  },
});
Future<void> mount(
  WidgetTester tester,
  QuestionController c, {
  bool multi = false,
  int options = 2,
}) async {
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: Center(
          child: SizedBox(
            width: 680,
            child: InteractionCard(
              controller: c,
              frame: request(multi: multi, options: options),
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Future<void> done(WidgetTester tester, QuestionController c) async {
  await tester.pumpWidget(const SizedBox());
  c.dispose();
  expect(tester.takeException(), isNull);
}

void main() {
  testWidgets(
    'IME confirmation and Shift Enter do not advance or submit the batch',
    (tester) async {
      final c = QuestionController();
      await mount(tester, c);
      await tester.enterText(find.byKey(const ValueKey('answer-one')), '中');
      tester.testTextInput.updateEditingValue(
        const TextEditingValue(
          text: '中',
          selection: TextSelection.collapsed(offset: 1),
          composing: TextRange(start: 0, end: 1),
        ),
      );
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(find.text('1 / 2'), findsOneWidget);
      tester.testTextInput.updateEditingValue(
        const TextEditingValue(
          text: '中文',
          selection: TextSelection.collapsed(offset: 2),
        ),
      );
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(find.text('2 / 2'), findsOneWidget);
      await tester.enterText(find.byKey(const ValueKey('answer-two')), '说明');
      await tester.pump();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.pump();
      expect(c.answers, isEmpty);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(c.answers, hasLength(1));
      await done(tester, c);
    },
  );
  testWidgets(
    'single choice auto advances; back keeps draft; custom replaces selection; retry preserves answers',
    (tester) async {
      final c = QuestionController();
      await mount(tester, c);
      expect(find.text('1 / 2'), findsOneWidget);
      expect(find.text('补充说明'), findsNothing);
      await tester.tap(find.text('A'));
      await tester.pump();
      expect(find.text('2 / 2'), findsOneWidget);
      await tester.tap(find.byTooltip('上一题'));
      await tester.pump();
      await tester.enterText(
        find.byKey(const ValueKey('answer-one')),
        'custom',
      );
      await tester.pump();
      await tester.tap(find.byKey(const Key('question-continue')));
      await tester.pump();
      await tester.enterText(
        find.byKey(const ValueKey('answer-two')),
        'detail',
      );
      await tester.pump();
      c.fail = true;
      await tester.tap(find.byKey(const Key('question-continue')));
      await tester.pump();
      expect(find.textContaining('连接暂时中断'), findsOneWidget);
      expect(find.text('detail'), findsOneWidget);
      c.fail = false;
      await tester.tap(find.byKey(const Key('question-continue')));
      await tester.pump();
      final answers = objects(object(c.answers.single['answer'])['answers']);
      expect(answers, [
        {'id': 'one', 'selected': <String>[], 'custom': 'custom'},
        {'id': 'two', 'selected': <String>[], 'custom': 'detail'},
      ]);
      await done(tester, c);
    },
  );
  testWidgets(
    'multi-select stays on its page and preserves original recommendation labels',
    (tester) async {
      final c = QuestionController();
      await mount(tester, c, multi: true);
      await tester.tap(find.text('A'));
      await tester.tap(find.text('B1'));
      await tester.pump();
      expect(find.text('1 / 2'), findsOneWidget);
      await tester.enterText(
        find.byKey(const ValueKey('answer-one')),
        'additional',
      );
      await tester.pump();
      await tester.tap(find.byKey(const Key('question-continue')));
      await tester.pump();
      await tester.tap(find.text('跳过本题'));
      await tester.pump();
      expect(objects(object(c.answers.single['answer'])['answers']), [
        {
          'id': 'one',
          'selected': ['A（推荐）', 'B1'],
          'custom': 'additional',
        },
        {'id': 'two', 'selected': <String>[]},
      ]);
      await done(tester, c);
    },
  );
  testWidgets(
    'all questions may be skipped and whole-request cancellation uses a separate action',
    (tester) async {
      final c = QuestionController();
      await mount(tester, c);
      await tester.tap(find.text('跳过本题'));
      await tester.pump();
      await tester.tap(find.text('跳过本题'));
      await tester.pump();
      expect(objects(object(c.answers.single['answer'])['answers']), [
        {'id': 'one', 'selected': <String>[]},
        {'id': 'two', 'selected': <String>[]},
      ]);
      await done(tester, c);
      final next = QuestionController();
      await mount(tester, next);
      await tester.tap(find.byTooltip('放弃整组问题'));
      await tester.pump();
      expect(next.cancellations, 1);
      expect(next.answers, isEmpty);
      await done(tester, next);
    },
  );
  testWidgets(
    'narrow long question keeps paging controls visible and height bounded',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(360, 600));
      final c = QuestionController();
      await mount(tester, c, options: 30);
      expect(
        tester.getSize(find.byKey(const Key('question-card'))).height,
        lessThanOrEqualTo(360),
      );
      expect(tester.getBottomLeft(find.text('跳过本题')).dy, lessThan(600));
      expect(tester.takeException(), isNull);
      await done(tester, c);
      await tester.binding.setSurfaceSize(null);
    },
  );
}
