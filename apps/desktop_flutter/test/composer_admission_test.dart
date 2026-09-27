import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class DraftPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class AdmissionController extends DesktopController {
  AdmissionController() : super(DraftPreferences()) {
    selectedId = 's';
  }
  final receipt = Completer<String?>();
  int submissions = 0;
  final modes = <String>[];
  @override
  bool get connected => true;
  @override
  Future<String?> sendParts(
    String text,
    List<Json> attachments, {
    String mode = 'queue',
  }) async {
    submissions++;
    modes.add(mode);
    sending = true;
    emit();
    try {
      return await receipt.future;
    } finally {
      sending = false;
      emit();
    }
  }
}

void main() {
  for (final platform in [TargetPlatform.windows, TargetPlatform.macOS]) {
    for (final busyEnter in ['queue', 'steer']) {
      for (final primary in [false, true]) {
        testWidgets('$platform Enter busy=$busyEnter primary=$primary', (
          tester,
        ) async {
          final c = AdmissionController()
            ..sessions = [
              SessionSummary.fromJson({'sessionId': 's', 'running': true}),
            ]
            ..conversationSettings = {'busyEnter': busyEnter};
          try {
            await tester.pumpWidget(
              ShadApp(
                home: Scaffold(body: Conversation(controller: c)),
              ),
            );
            await tester.pumpAndSettle();
            await tester.enterText(
              find.byKey(const Key('prompt-input')),
              'next message',
            );
            final modifier = platform == TargetPlatform.macOS
                ? LogicalKeyboardKey.metaLeft
                : LogicalKeyboardKey.controlLeft;
            if (primary) await tester.sendKeyDownEvent(modifier);
            await tester.sendKeyEvent(LogicalKeyboardKey.enter);
            if (primary) await tester.sendKeyUpEvent(modifier);
            await tester.pump();
            expect(c.submissions, 1);
            expect(
              c.modes.single,
              ((busyEnter == 'steer') != primary) ? 'steer' : 'queue',
            );
            c.receipt.complete('s');
            await tester.pumpAndSettle();
            expect(tester.takeException(), isNull);
          } finally {
            await tester.pumpWidget(const SizedBox());
            c.dispose();
          }
        }, variant: TargetPlatformVariant.only(platform));
      }
    }
  }
  testWidgets(
    'macOS Command Enter continues a numbered draft without sending',
    (tester) async {
      final c = AdmissionController();
      try {
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(body: Conversation(controller: c)),
          ),
        );
        await tester.pumpAndSettle();
        await tester.enterText(
          find.byKey(const Key('prompt-input')),
          '1. First',
        );
        await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
        await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
        await tester.pump();
        expect(c.submissions, 0);
        final input = tester
            .widget<TextField>(find.byKey(const Key('prompt-input')))
            .controller!;
        expect(input.text, '1. First\n2. ');
        expect(c.draft, input.text);
        expect(tester.takeException(), isNull);
      } finally {
        await tester.pumpWidget(const SizedBox());
        c.dispose();
      }
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );
  testWidgets(
    'macOS modified Enter does not submit while composing or adding a line',
    (tester) async {
      final c = AdmissionController();
      try {
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(body: Conversation(controller: c)),
          ),
        );
        await tester.pumpAndSettle();
        await tester.enterText(find.byKey(const Key('prompt-input')), 'draft');
        final input = tester
            .widget<TextField>(find.byKey(const Key('prompt-input')))
            .controller!;
        input.value = const TextEditingValue(
          text: 'draft',
          selection: TextSelection.collapsed(offset: 5),
          composing: TextRange(start: 0, end: 5),
        );
        await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
        await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
        await tester.pump();
        expect(c.submissions, 0);
        input.value = const TextEditingValue(
          text: 'draft',
          selection: TextSelection.collapsed(offset: 5),
        );
        await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
        await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
        await tester.pump();
        expect(c.submissions, 0);
        await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
        await tester.sendKeyEvent(LogicalKeyboardKey.enter);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
        await tester.pump();
        expect(c.submissions, 0);
        expect(tester.takeException(), isNull);
      } finally {
        await tester.pumpWidget(const SizedBox());
        c.dispose();
      }
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );
  for (final accepted in [false, true]) {
    testWidgets('composer clears only an actual receipt: $accepted', (
      tester,
    ) async {
      final c = AdmissionController();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      await tester.enterText(find.byKey(const Key('prompt-input')), 'original');
      await tester.pump();
      await tester.tap(find.byKey(const Key('send-message')));
      await tester.pump();
      c.receipt.complete(accepted ? 's' : null);
      expect(c.submissions, 1);
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('prompt-input')))
            .controller!
            .text,
        accepted ? '' : 'original',
      );
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      expect(tester.takeException(), isNull);
    });
  }
  testWidgets(
    'old receipt cannot clear the newly selected conversation draft',
    (tester) async {
      final c = AdmissionController();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const Key('prompt-input')),
        'same text',
      );
      await tester.pump();
      await tester.tap(find.byKey(const Key('send-message')));
      await tester.pump();
      c.preferences.drafts['other'] = 'same text';
      c.selectedId = 'other';
      c.emit();
      await tester.pump();
      c.receipt.complete('s');
      expect(c.submissions, 1);
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<TextField>(find.byKey(const Key('prompt-input')))
            .controller!
            .text,
        'same text',
      );
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      expect(tester.takeException(), isNull);
    },
  );
}
