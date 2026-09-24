import 'dart:async';

import 'package:dsh_desktop/src/voice_input.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show MemoryPreferences;

class VoiceConversationController extends DesktopController {
  VoiceConversationController() : super(MemoryPreferences());
  @override
  bool get connected => true;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('dsh/voice');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  late String generation;
  late List<dynamic> events;
  late List<String> calls;
  setUp(() {
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    generation = '';
    events = [];
    calls = [];
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call.method);
      if (call.method == 'start') generation = call.arguments as String;
      if (call.method == 'poll') {
        final batch = events;
        events = [];
        return batch;
      }
      return null;
    });
  });
  tearDown(() {
    messenger.setMockMethodCallHandler(channel, null);
    debugDefaultTargetPlatformOverride = null;
  });
  Map<String, String> event(String kind, [String text = '']) => {
    'generation': generation,
    'kind': kind,
    'text': text,
  };

  testWidgets(
    'dictation updates composer without rebuilding output and exposes errors',
    (tester) async {
      final c = VoiceConversationController()
        ..selectedId = 's'
        ..transcript = [
          TranscriptItem(id: 'reply', kind: 'assistant', text: '已完成的正文'),
        ];
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      tester.widget<VoiceInputButton>(find.byType(VoiceInputButton)).onStart!();
      await tester.pump();
      expect(find.text('正在启动语音识别…'), findsOneWidget);
      final output = tester.widget<MessageCard>(find.byType(MessageCard));
      events = [event('ready'), event('partial', '口述草稿')];
      await tester.pump(const Duration(milliseconds: 60));
      expect(
        tester.widget<MessageCard>(find.byType(MessageCard)),
        same(output),
      );
      expect(c.draft, '口述草稿');
      expect(find.text('正在聆听，松开结束'), findsOneWidget);
      events = [event('error', 'microphone:0x8004503A'), event('stopped')];
      await tester.pump(const Duration(milliseconds: 60));
      expect(
        tester.widget<Text>(find.byKey(const ValueKey('voice-status'))).data,
        contains('Windows 语音识别不可用'),
      );
      expect(
        tester.widget<MessageCard>(find.byType(MessageCard)),
        same(output),
      );
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      debugDefaultTargetPlatformOverride = null;
    },
  );

  testWidgets(
    'hold gesture stops on release and cancel; keyboard repeat starts once',
    (tester) async {
      var starts = 0, stops = 0;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: VoiceInputButton(
              listening: false,
              supported: true,
              onStart: () => starts++,
              onStop: () => stops++,
            ),
          ),
        ),
      );
      final button = find.byType(VoiceInputButton);
      expect(tester.getSize(button), const Size(28, 28));
      final pointer = await tester.startGesture(tester.getCenter(button));
      await tester.pump();
      expect(starts, 1);
      expect(stops, 0);
      await pointer.up();
      await tester.pump();
      expect(stops, 1);
      final cancelled = await tester.startGesture(tester.getCenter(button));
      await cancelled.cancel();
      await tester.pump();
      expect(starts, 2);
      expect(stops, 2);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.space);
      await tester.sendKeyRepeatEvent(LogicalKeyboardKey.space);
      expect(starts, 3);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.space);
      expect(stops, 3);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.enter);
      expect(starts, 4);
      expect(stops, 4);
      await tester.pumpWidget(const SizedBox());
      await tester.pump();
      expect(tester.takeException(), isNull);
      debugDefaultTargetPlatformOverride = null;
    },
  );

  testWidgets(
    'losing focus releases held recording and disabled input never starts',
    (tester) async {
      var starts = 0, stops = 0;
      final other = FocusNode();
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Column(
              children: [
                VoiceInputButton(
                  listening: false,
                  supported: true,
                  onStart: () => starts++,
                  onStop: () => stops++,
                ),
                Focus(focusNode: other, child: const Text('other')),
              ],
            ),
          ),
        ),
      );
      final pointer = await tester.startGesture(
        tester.getCenter(find.byType(VoiceInputButton)),
      );
      await tester.pump();
      other.requestFocus();
      await tester.pump();
      expect(starts, 1);
      expect(stops, 1);
      await pointer.up();
      expect(stops, 1);
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: VoiceInputButton(
              listening: false,
              supported: false,
              onStart: () => starts++,
              onStop: () => stops++,
            ),
          ),
        ),
      );
      await tester.tap(find.byType(VoiceInputButton));
      expect(starts, 1);
      await tester.pumpWidget(const SizedBox());
      other.dispose();
      debugDefaultTargetPlatformOverride = null;
    },
  );

  testWidgets(
    'accepted start is not ready; partial replaces interim text and stopped releases polling',
    (tester) async {
      final c = VoiceInputController();
      await c.start('已有草稿');
      expect(c.phase, 'starting');
      expect(c.listening, isFalse);
      events = [event('ready'), event('partial', '你')];
      await tester.pump(const Duration(milliseconds: 60));
      expect(c.listening, isTrue);
      expect(c.latestText, '已有草稿 你');
      events = [event('partial', '你好'), event('result', '你好')];
      await tester.pump(const Duration(milliseconds: 60));
      expect(c.latestText, '已有草稿 你好');
      await c.stop();
      expect(c.phase, 'stopping');
      events = [
        event('partial', '忽略中间文本'),
        event('result', '结束'),
        event('stopped'),
      ];
      await tester.pump(const Duration(milliseconds: 60));
      expect(c.latestText, '已有草稿 你好 结束');
      expect(c.phase, 'idle');
      final count = calls.length;
      await tester.pump(const Duration(seconds: 3));
      expect(calls.length, count);
      c.dispose();
      debugDefaultTargetPlatformOverride = null;
    },
  );

  testWidgets(
    'failure remains idle, retry ignores previous generation, disposal cancels an in-flight start',
    (tester) async {
      final c = VoiceInputController();
      await c.start('a');
      final old = generation;
      events = [event('error', 'microphone:0x8004503A'), event('stopped')];
      await tester.pump(const Duration(milliseconds: 60));
      expect(c.error, contains('microphone'));
      expect(c.active, isFalse);
      await c.start('b');
      events = [
        {'generation': old, 'kind': 'result', 'text': 'wrong'},
        event('ready'),
      ];
      await tester.pump(const Duration(milliseconds: 60));
      expect(c.latestText, 'b');
      c.cancel();
      final count = calls.length;
      await tester.pump(const Duration(seconds: 1));
      expect(calls.length, count);
      c.dispose();

      final pending = Completer<void>();
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call.method);
        if (call.method == 'start') await pending.future;
        return null;
      });
      final late = VoiceInputController();
      final start = late.start('draft');
      late.dispose();
      pending.complete();
      await start;
      await tester.pump(const Duration(seconds: 1));
      expect(calls.last, 'stop');
      expect(tester.takeException(), isNull);
      debugDefaultTargetPlatformOverride = null;
    },
  );
}
