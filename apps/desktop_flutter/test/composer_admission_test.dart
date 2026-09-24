import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
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
  @override
  bool get connected => true;
  @override
  Future<String?> sendParts(
    String text,
    List<Json> attachments, {
    String mode = 'queue',
  }) async {
    submissions++;
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
