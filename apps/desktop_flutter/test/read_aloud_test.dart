import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/read_aloud.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

void main() {
  testWidgets(
    'speech chunks preserve a surrogate pair and stop after the last chunk',
    (tester) async {
      final starts = <String>[];
      var stops = 0;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        ReadAloudController.channel,
        (call) async {
          if (call.method == 'start') {
            starts.add((call.arguments as Map)['text'] as String);
          }
          if (call.method == 'stop') stops++;
          if (call.method == 'status') return true;
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          ReadAloudController.channel,
          null,
        ),
      );
      final controller = ReadAloudController();
      final answer = '${'a' * 4095}😀${'b' * 4095}';
      await controller.toggle('tail', answer);
      expect(starts, ['a' * 4095]);
      for (var i = 0; i < 4; i++) {
        await tester.pump(const Duration(milliseconds: 350));
        await tester.pump();
      }
      expect(starts.join(), answer);
      expect(starts[1].startsWith('😀'), isTrue);
      expect(controller.activeId, isNull);
      expect(stops, 1);
      final count = starts.length;
      await tester.pump(const Duration(seconds: 3));
      expect(starts.length, count);
      controller.dispose();
    },
  );

  testWidgets(
    'the completed answer has one native speak button that toggles playback',
    (tester) async {
      final starts = <String>[];
      var stops = 0;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        ReadAloudController.channel,
        (call) async {
          if (call.method == 'start') {
            starts.add((call.arguments as Map)['text'] as String);
          }
          if (call.method == 'stop') stops++;
          if (call.method == 'status') return false;
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          ReadAloudController.channel,
          null,
        ),
      );
      final voice = ReadAloudController();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: MessageCard(
              item: TranscriptItem(
                id: 'turn-tail:1',
                kind: 'turn-tail',
                text: '完整回复。',
              ),
              readAloud: voice,
            ),
          ),
        ),
      );
      expect(find.byTooltip('朗读'), findsOneWidget);
      await tester.tap(find.byTooltip('朗读'));
      await tester.pump();
      expect(starts, ['完整回复。']);
      expect(find.byTooltip('停止朗读'), findsOneWidget);
      await tester.tap(find.byTooltip('停止朗读'));
      await tester.pump();
      expect(stops, 1);
      expect(find.byTooltip('朗读'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      voice.dispose();
    },
  );
  testWidgets('native stop failure still releases text and timer ownership', (
    tester,
  ) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      ReadAloudController.channel,
      (call) async {
        if (call.method == 'stop') {
          throw PlatformException(code: 'closing');
        }
        if (call.method == 'status') return false;
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        ReadAloudController.channel,
        null,
      ),
    );
    final voice = ReadAloudController();
    await voice.toggle('tail', '完整回复');
    expect(voice.retainedTextUnits, 4);
    await voice.stop();
    expect(voice.activeId, isNull);
    expect(voice.retainedTextUnits, 0);
    await tester.pump(const Duration(seconds: 1));
    expect(tester.takeException(), isNull);
    voice.dispose();
  });
}
