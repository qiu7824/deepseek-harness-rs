import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/feedback_controller.dart';
import 'package:dsh_desktop/features/conversation/feedback_actions.dart';
import 'package:dsh_desktop/features/shell.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  testWidgets(
    'rating requires confirmation and rejected save retains the note',
    (tester) async {
      final fake = FeedbackApi();
      final c = MessageFeedbackController(fake, 's')..retainTargets(['m']);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: MessageFeedbackActions(controller: c, messageId: 'm'),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('好的回答'));
      await tester.pumpAndSettle();
      expect(fake.calls, isEmpty);
      await tester.enterText(find.byType(TextField), '保留说明');
      await tester.tap(find.text('确认评价'));
      await tester.pumpAndSettle();
      expect(c.item('m'), isNull);
      expect(find.textContaining('session-not-found'), findsOneWidget);
      expect(find.text('保留说明'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await fake.close();
    },
  );

  testWidgets(
    'session business failure stays open; retry preserves id until the draft changes',
    (tester) async {
      final fake = FeedbackApi();
      await tester.pumpWidget(
        ShadApp(
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                child: const Text('open'),
                onPressed: () => showDialog<void>(
                  context: context,
                  builder: (_) =>
                      SessionFeedbackDialog(api: fake, sessionId: 's'),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), '保留会话反馈');
      await tester.tap(find.text('保存反馈'));
      await tester.pumpAndSettle();
      expect(find.text('保留会话反馈'), findsOneWidget);
      expect(find.textContaining('session-not-found'), findsOneWidget);
      final requestId = fake.calls.last['requestId'];
      await tester.tap(find.text('保存反馈'));
      await tester.pumpAndSettle();
      expect(fake.calls.last['requestId'], requestId);
      await tester.enterText(find.byType(TextField), '修改后的会话反馈');
      fake.fail = false;
      await tester.tap(find.text('保存反馈'));
      await tester.pumpAndSettle();
      expect(fake.calls.last['requestId'], isNot(requestId));
      expect(find.byType(SessionFeedbackDialog), findsNothing);
      await tester.pumpWidget(const SizedBox());
      await fake.close();
    },
  );
}

class FeedbackApi extends DshClient {
  FeedbackApi() : super('http://127.0.0.1:1');
  final calls = <Json>[];
  bool fail = true;
  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    if (method == 'messageFeedback.list') {
      return {
        'ok': true,
        'value': {'items': []},
      };
    }
    return call(method, payload, mutation);
  }

  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    calls.add(payload);
    return fail
        ? {
            'ok': false,
            'error': {'code': 'session-not-found'},
          }
        : {
            'ok': true,
            'value': {'recorded': true},
          };
  }
}
