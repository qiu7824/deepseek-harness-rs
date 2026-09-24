import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/features/conversation/retry_message.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  testWidgets('retry details expand and countdown stops after disposal', (
    tester,
  ) async {
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: RetryMessage(
            item: TranscriptItem(
              id: 'r',
              kind: 'retry',
              text: '',
              status: 'scheduled',
              output: '{"retry":1,"maximum":2,"delayMs":3000,"failure":{"code":"TIMEOUT","message":"等待响应超时"}}',
            ),
          ),
        ),
      ),
    );
    expect(find.text('正在重试模型请求（1/2） · 3s'), findsOneWidget);
    await tester.pump(const Duration(seconds: 1));
    await tester.tap(find.text('正在重试模型请求（1/2） · 2s'));
    await tester.pump();
    expect(find.textContaining('等待响应超时'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(seconds: 10));
    expect(tester.takeException(), isNull);
  });
  testWidgets(
    'tool and reasoning rows retain the Web flow gap before the next reply',
    (tester) async {
      for (final kind in ['tool', 'reasoning']) {
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: Column(
                children: [
                  MessageCard(
                    key: const Key('preceding'),
                    item: TranscriptItem(
                      id: 'preceding',
                      kind: kind,
                      title: '工具调用',
                      text: '检查内容',
                    ),
                  ),
                  MessageCard(
                    key: const Key('following'),
                    bottomSpacing: 0,
                    item: TranscriptItem(
                      id: 'following',
                      kind: 'assistant',
                      text: '继续回复',
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(tester.getSize(find.byKey(const Key('preceding'))).height, 40);
        expect(tester.getTopLeft(find.byKey(const Key('following'))).dy, 40);
        expect(tester.takeException(), isNull);
      }
    },
  );
  testWidgets(
    'only the closed turn has actions; copy uses its complete answer and time follows hover',
    (tester) async {
      final events = <HistoryEvent>[];
      for (var step = 1; step <= 3; step++) {
        events.add(
          HistoryEvent.fromJson({
            'seq': step,
            'time': 1000 * step,
            'type': 'assistant/message',
            'data': {
              'turn': 1,
              'step': step,
              'message': {
                'id': 'm$step',
                'content': [
                  {'type': 'text', 'text': step == 3 ? '第一段' : '继续检查 $step'},
                  if (step == 3) {'type': 'text', 'text': '第二段'},
                ],
              },
            },
          }),
        );
      }
      Future<void> show() => tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: Column(
                children: [
                  for (final item in projectTranscript(events))
                    MessageCard(item: item, onBranch: () {}),
                ],
              ),
            ),
          ),
        ),
      );
      await show();
      await tester.pumpAndSettle();
      expect(find.byTooltip('复制'), findsNothing);
      expect(find.byTooltip('在新对话中分支'), findsNothing);
      events.add(
        HistoryEvent.fromJson({
          'seq': 4,
          'type': 'turn/end',
          'data': {'turn': 1},
        }),
      );
      await show();
      await tester.pumpAndSettle();
      expect(find.byTooltip('复制'), findsOneWidget);
      expect(find.byTooltip('在新对话中分支'), findsOneWidget);
      expect(find.text('第一段'), findsOneWidget);
      expect(find.text('第二段'), findsOneWidget);
      String? copied;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied = (call.arguments as Map)['text'] as String;
          }
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      await tester.tap(find.byTooltip('复制'));
      expect(copied, '第一段\n\n第二段');
      final opacity = find.descendant(
        of: find.byType(MessageActionRow),
        matching: find.byKey(const ValueKey('message-time-opacity')),
      );
      expect(tester.widget<Opacity>(opacity).opacity, 0);
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: Offset.zero);
      await mouse.moveTo(tester.getCenter(find.byType(MessageActionRow)));
      await tester.pump();
      expect(tester.widget<Opacity>(opacity).opacity, 1);
      await mouse.moveTo(Offset.zero);
      await tester.pump();
      expect(tester.widget<Opacity>(opacity).opacity, 0);
      await mouse.removePointer();
      expect(tester.takeException(), isNull);
    },
  );
}
