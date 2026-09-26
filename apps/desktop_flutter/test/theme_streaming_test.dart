import 'dart:async';

import 'package:dsh_desktop/src/window_theme.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/design/rich_content.dart';
import 'package:dsh_desktop/features/conversation/streaming_presentation.dart';
import 'package:dsh_desktop/features/conversation/turn_activity.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;
import 'workbench_test.dart' show TestController;

void main() {
  testWidgets('a newly received final message is complete on its first frame', (
    tester,
  ) async {
    final c = TestController()..selectedId = 's';
    final old = TranscriptItem(id: 'old', kind: 'assistant', text: '历史正文');
    c.transcript = [old];
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('历史正文'), findsOneWidget);
    const incoming = '刚刚收到的完整回复立即显示。';
    c.transcript = [
      old,
      TranscriptItem(id: 'new', kind: 'assistant', text: incoming),
    ];
    c.messageChanges.value++;
    await tester.pump();
    final fresh = find.descendant(
      of: find.byKey(const ValueKey('new')).last,
      matching: find.byType(ProgressiveText),
    );
    expect(tester.widget<ProgressiveText>(fresh).streaming, isFalse);
    expect(tester.widget<ProgressiveText>(fresh).revealInitial, isFalse);
    expect(find.text(incoming), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
  });
  testWidgets(
    'loading history while sending does not replay received paragraphs',
    (tester) async {
      final c = TestController()
        ..selectedId = 's'
        ..sending = true
        ..transcript = [
          TranscriptItem(
            id: 'restored',
            kind: 'assistant',
            text: '恢复的历史正文应完整可见',
          ),
        ];
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      expect(find.text('恢复的历史正文应完整可见'), findsOneWidget);
      expect(
        tester.widget<ProgressiveText>(find.byType(ProgressiveText)).streaming,
        isFalse,
      );
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
  testWidgets(
    'scrolling away and back remounts a streamed row with all received text',
    (tester) async {
      final history = [
        for (var i = 0; i < 30; i++)
          TranscriptItem(
            id: 'history-$i',
            kind: 'assistant',
            text: '历史 $i\n\n${'已有内容 ' * 40}',
          ),
      ];
      final c = TestController()
        ..selectedId = 's'
        ..transcript = history;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      const received = '刚收到的正文，滚动回到这里仍然完整。';
      c.transcript = [
        ...history,
        TranscriptItem(
          id: 'live',
          kind: 'assistant',
          text: received,
          streaming: true,
        ),
      ];
      c.messageChanges.value++;
      await tester.pump();
      expect(find.text(received), findsOneWidget);
      final row = find.byKey(const ValueKey('live')).last;
      final firstState = tester.state(
        find.descendant(of: row, matching: find.byType(ProgressiveText)),
      );
      final list = tester.widget<ListView>(
        find.byKey(const PageStorageKey('messages-s')),
      );
      final controller = list.controller!;
      expect(controller.position.maxScrollExtent, greaterThan(1000));
      controller.jumpTo(controller.position.maxScrollExtent);
      await tester.pump();
      expect(find.byKey(const ValueKey('live')), findsNothing);
      controller.jumpTo(0);
      await tester.pump();
      expect(find.text(received), findsOneWidget);
      final remounted = tester.state(
        find.descendant(of: row, matching: find.byType(ProgressiveText)),
      );
      expect(remounted, isNot(same(firstState)));
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
  testWidgets(
    'dark sidebar remains visible across collapse expand and theme changes',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1440, 900));
      final c = TestController()..preferences.dark = true;
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      for (final dark in [true, false, true]) {
        c.preferences.dark = dark;
        c.emit();
        await tester.pumpAndSettle();
        final color = dark ? const Color(0xfff9fafb) : const Color(0xff0f1115);
        SvgAssetLoader loader(String state) =>
            (tester
                    .widget<SvgPicture>(
                      find.byKey(ValueKey('brand-$state-$dark')),
                    )
                    .bytesLoader
                as SvgAssetLoader);
        expect(loader('expanded').theme!.currentColor, color);
        await tester.tap(find.byTooltip('收起侧边栏 · Ctrl+B'));
        await tester.pumpAndSettle();
        expect(loader('rail').theme!.currentColor, color);
        await tester.tap(find.byTooltip('展开侧边栏 · Ctrl+B'));
        await tester.pumpAndSettle();
        expect(loader('expanded').theme!.currentColor, color);
        expect(tester.takeException(), isNull);
      }
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets(
    'native chrome follows latest app preference and stops after disposal',
    (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.windows;
      final calls = <bool>[], pending = Completer<void>();
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        WindowThemeBinding.channel,
        (call) async {
          calls.add(call.arguments as bool);
          if (calls.length == 1) await pending.future;
          return null;
        },
      );
      final c = DesktopController(MemoryPreferences()..dark = true);
      final theme = WindowThemeBinding(c), started = theme.start();
      await tester.pump();
      c.preferences.dark = false;
      c.emit();
      pending.complete();
      await started;
      expect(calls, [true, false]);
      c.emit();
      await tester.pump();
      expect(calls.length, 2);
      theme.dispose();
      c.preferences.dark = true;
      c.emit();
      await tester.pump();
      expect(calls.length, 2);
      c.dispose();
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        WindowThemeBinding.channel,
        null,
      );
      debugDefaultTargetPlatformOverride = null;
    },
  );
  testWidgets(
    'received text progresses by graphemes then catches up; history and reduced motion are instant',
    (tester) async {
      const source = '你好👨‍👩‍👧‍👦，逐字显示。';
      Future<void> show(
        String text,
        bool streaming, {
        bool reduced = false,
        Key? key,
      }) => tester.pumpWidget(
        MaterialApp(
          home: MediaQuery(
            data: MediaQueryData(disableAnimations: reduced),
            child: ProgressiveText(
              key: key,
              text: text,
              streaming: streaming,
              builder: (s) => Text(s, key: const Key('visible')),
            ),
          ),
        ),
      );
      await show(source, true);
      expect(
        tester.widget<Text>(find.byKey(const Key('visible'))).data,
        isNot(source),
      );
      await tester.pump(const Duration(milliseconds: 50));
      final text = tester.widget<Text>(find.byKey(const Key('visible'))).data!;
      expect(source.startsWith(text), isTrue);
      expect(source.characters.map((s) => s).join().startsWith(text), isTrue);
      await tester.pump(const Duration(milliseconds: 400));
      expect(find.text(source), findsOneWidget);
      await show('$source 已结束', false);
      expect(find.text('$source 已结束'), findsOneWidget);
      await show('历史完整消息', false, key: const ValueKey('history'));
      expect(find.text('历史完整消息'), findsOneWidget);
      await show('减少动态效果', true, reduced: true, key: const ValueKey('reduced'));
      expect(find.text('减少动态效果'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      await tester.pump(const Duration(seconds: 1));
      expect(tester.binding.transientCallbackCount, 0);
    },
  );
  testWidgets(
    'stream completion immediately flushes a pending received append',
    (tester) async {
      Future<void> show(String text, bool streaming) => tester.pumpWidget(
        MaterialApp(
          home: ProgressiveText(
            text: text,
            streaming: streaming,
            revealInitial: false,
            builder: (visible) => Text(visible),
          ),
        ),
      );
      await show('已有正文', true);
      final received = '已有正文${'新追加内容' * 100}';
      await show(received, true);
      expect(find.text(received), findsNothing);
      await show(received, false);
      expect(find.text(received), findsOneWidget);
      expect(tester.binding.transientCallbackCount, 0);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets(
    'muted tickers flush pending text and never replay it when resumed',
    (tester) async {
      Future<void> show(String text, bool enabled) => tester.pumpWidget(
        MaterialApp(
          home: TickerMode(
            enabled: enabled,
            child: ProgressiveText(
              text: text,
              streaming: true,
              revealInitial: false,
              builder: (visible) => Text(visible),
            ),
          ),
        ),
      );
      await show('已有正文', true);
      final received = '已有正文${'新追加内容' * 100}';
      await show(received, true);
      expect(find.text(received), findsNothing);
      await show(received, false);
      expect(find.text(received), findsOneWidget);
      await show('$received 隐藏时又收到一段', false);
      expect(find.text('$received 隐藏时又收到一段'), findsOneWidget);
      await show('$received 隐藏时又收到一段', true);
      expect(find.text('$received 隐藏时又收到一段'), findsOneWidget);
      expect(tester.binding.transientCallbackCount, 0);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets(
    'leaving follow mode flushes the pending text of a mounted message',
    (tester) async {
      final history = [
        for (var i = 0; i < 12; i++)
          TranscriptItem(
            id: 'history-$i',
            kind: 'assistant',
            text: '历史 $i\n\n${'已有内容 ' * 40}',
          ),
      ];
      TranscriptItem live(String text) => TranscriptItem(
        id: 'live',
        kind: 'assistant',
        text: text,
        streaming: true,
      );
      const initial = '已有正文\n\n第二段已经收到\n\n第三段已经收到\n\n第四段已经收到';
      final c = TestController()
        ..selectedId = 's'
        ..transcript = [...history, live(initial)];
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      final row = find.byKey(const ValueKey('live')).last;
      final markdown = find.descendant(
        of: row,
        matching: find.byType(DshMarkdown),
      );
      final progressive = find.descendant(
        of: row,
        matching: find.byType(ProgressiveText),
      );
      final state = tester.state(progressive);
      // Keep part of the live row painted after crossing the 60 px follow
      // threshold, so the same mounted row must flush its pending text.
      expect(tester.getSize(row).height, greaterThan(80));
      final received = '$initial${'新追加内容' * 20}';
      c.transcript = [...history, live(received)];
      c.messageChanges.value++;
      await tester.pump();
      expect(tester.widget<DshMarkdown>(markdown).data, isNot(received));
      final list = tester.widget<ListView>(
        find.byKey(const PageStorageKey('messages-s')),
      );
      list.controller!.jumpTo(80);
      await tester.pump();
      expect(tester.state(progressive), same(state));
      expect(tester.widget<DshMarkdown>(markdown).data, received);
      list.controller!.jumpTo(0);
      await tester.pump();
      expect(tester.widget<DshMarkdown>(markdown).data, received);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
  testWidgets(
    'streaming append keeps a visible grapheme when it gains code points',
    (tester) async {
      Future<void> show(String value) => tester.pumpWidget(
        MaterialApp(
          home: ProgressiveText(
            text: value,
            streaming: true,
            builder: (visible) => Text(visible),
          ),
        ),
      );
      await show('👨');
      await tester.pump(const Duration(milliseconds: 400));
      expect(find.text('👨'), findsOneWidget);
      await show('👨‍👩');
      expect(find.text('👨‍👩'), findsOneWidget);
      await show('👨‍👩a');
      await tester.pump(const Duration(milliseconds: 400));
      expect(find.text('👨‍👩a'), findsOneWidget);
      await show('👨‍👩a\u0301');
      expect(find.text('👨‍👩a\u0301'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets(
    'thinking sweeps and follows latest line; completed request is not a completed turn',
    (tester) async {
      final c = TestController()
        ..selectedId = 's'
        ..sessions = [
          SessionSummary.fromJson({'sessionId': 's', 'running': true}),
        ];
      c.projectionWindow.apply('sessionStats', {
        'requestPhase': {'phase': 'completed'},
      }, 1);
      expect(turnActivityLabel(c), '正在继续处理');
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ReasoningMessage(
              item: TranscriptItem(
                id: 'r',
                kind: 'reasoning',
                text: '早期思考\n${'过程' * 500}末尾进度',
                streaming: true,
              ),
              hintDisplay: 'both',
            ),
          ),
        ),
      );
      await tester.pump(const Duration(milliseconds: 100));
      expect(
        tester.widget<ThinkingSweep>(find.byType(ThinkingSweep)).running,
        isTrue,
      );
      expect(find.textContaining('末尾进度'), findsOneWidget);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ReasoningMessage(
              item: TranscriptItem(
                id: 'r',
                kind: 'reasoning',
                text: '早期思考\n末尾进度',
              ),
              hintDisplay: 'both',
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        tester.widget<ThinkingSweep>(find.byType(ThinkingSweep)).running,
        isFalse,
      );
      expect(find.text('早期思考'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      expect(tester.binding.transientCallbackCount, 0);
    },
  );
}
