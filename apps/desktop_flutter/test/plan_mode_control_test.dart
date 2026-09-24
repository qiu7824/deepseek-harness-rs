import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class ModePreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class ModeController extends DesktopController {
  ModeController() : super(ModePreferences()) {
    selectedId = 's';
  }
  final changes = <bool>[];
  @override
  bool get connected => true;
  @override
  Future<void> setPlanMode(bool enabled) async {
    changes.add(enabled);
  }

  void mode(bool active, bool pending, int seq) {
    projectionWindow.apply('plan', {'active': active, 'pending': pending}, seq);
    projectionChanges.value++;
  }
}

void main() {
  testWidgets('active plan controls fit a narrow conversation pane', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(360, 600));
    final c = ModeController()..mode(true, false, 1);
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byTooltip('关闭计划模式'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await tester.binding.setSurfaceSize(null);
  });
  testWidgets(
    'a stale mode control cannot affect a newly selected conversation',
    (tester) async {
      final c = ModeController()..mode(true, false, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      final button = tester.widget<DshIcon>(
        find.byWidgetPredicate((w) => w is DshIcon && w.label == '关闭计划模式'),
      );
      c.newConversation();
      c.selectedId = 'other';
      c.mode(true, false, 2);
      button.onPressed!();
      expect(c.changes, isEmpty);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      expect(tester.takeException(), isNull);
    },
  );
  test('plan projection preserves unavailable state for invalid wire data', () {
    expect(PlanModeProjection.from(null), isNull);
    expect(PlanModeProjection.from({'active': true}), isNull);
    expect(
      PlanModeProjection.from({'active': 'true', 'pending': false}),
      isNull,
    );
  });
  testWidgets(
    'composer follows plan projection rather than the visible history page',
    (tester) async {
      final c = ModeController();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      String? hint() => tester
          .widget<TextField>(find.byKey(const Key('prompt-input')))
          .decoration!
          .hintText;
      expect(find.byTooltip('关闭计划模式'), findsNothing);
      c.mode(false, true, 2);
      await tester.pump();
      expect(hint(), '描述你的任务以生成计划');
      expect(find.byTooltip('关闭计划模式'), findsOneWidget);
      await tester.tap(find.byTooltip('关闭计划模式'));
      await tester.pump();
      expect(c.changes, [false]);
      c.mode(true, true, 3);
      await tester.pump();
      expect(find.byTooltip('关闭计划模式'), findsNothing);
      expect(hint(), isNot('描述你的任务以生成计划'));
      c.window.replace(
        HistoryPage.fromJson({
          'events': [
            {
              'event': {
                'seq': 1,
                'type': 'plan/mode',
                'data': {'active': true},
              },
            },
          ],
        }),
      );
      c.mode(false, false, 4);
      await tester.pump();
      expect(hint(), isNot('描述你的任务以生成计划'));
      c.mode(true, false, 5);
      await tester.pump();
      expect(hint(), '描述你的任务以生成计划');
      c.clearProjections();
      await tester.pump();
      expect(find.byTooltip('关闭计划模式'), findsNothing);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      expect(tester.takeException(), isNull);
    },
  );
}
