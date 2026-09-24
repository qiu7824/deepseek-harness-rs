import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

SessionSummary row(String title, int seq) => SessionSummary.fromJson({
  'sessionId': 's',
  'projections': {
    'asOfSeq': seq,
    'values': {'title': title},
  },
});

void main() {
  test(
    'rename sends the opening baseline after another window changes the title',
    () async {
      final api = FakeClient()..liveSessions = [row('Original', 10)];
      final controller = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await controller.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      await controller.refreshSessions();
      final opening = controller.titleEditBase('s');
      api.liveSessions = [row('Other window', 20)];
      await controller.refreshSessions();
      api.handleCall = (method, payload) async {
        if (method == 'session.rename') {
          expect(payload['expectedTitle'], {
            'value': 'Original',
            'throughSeq': 10,
          });
          throw DshException('title-conflict', 'Title changed');
        }
        return {'items': [], 'archivedSessionIds': []};
      };
      await expectLater(
        controller.renameSession(
          's',
          'My draft',
          expectedTitle: opening,
          expectedClient: api,
        ),
        throwsA(
          isA<DshException>().having(
            (error) => error.code,
            'code',
            'title-conflict',
          ),
        ),
      );
      expect(controller.titleEditBase('s'), {
        'value': 'Other window',
        'throughSeq': 20,
      });
      controller.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );

  testWidgets(
    'failed save keeps the editor draft until explicit recovery and save',
    (tester) async {
      var adopted = false, attempts = 0;
      String? accepted;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Builder(
              builder: (context) => TextButton(
                onPressed: () async {
                  accepted = await editTextDialog(
                    context,
                    '重命名会话',
                    'Original',
                    onSubmit: (value) async {
                      attempts++;
                      if (!adopted) {
                        throw DshException('title-conflict', 'Title changed');
                      }
                    },
                    recoveryRequired: (error) =>
                        error is DshException && error.code == 'title-conflict',
                    onRecover: () async {
                      adopted = true;
                      return '当前标题：Other window';
                    },
                  );
                },
                child: const Text('Open'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), 'My draft');
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsOneWidget);
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.text,
        'My draft',
      );
      expect(
        tester
            .widget<DshButton>(find.widgetWithText(DshButton, '保存'))
            .onPressed,
        isNull,
      );
      await tester.tap(find.text('读取最新状态'));
      await tester.pumpAndSettle();
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.text,
        'My draft',
      );
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(accepted, 'My draft');
      expect(attempts, 2);
      expect(find.byType(AlertDialog), findsNothing);
    },
  );
}
