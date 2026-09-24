import 'dart:convert';
import 'dart:io';

import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/src/desktop_diagnostics.dart';
import 'package:dsh_desktop/src/resource_diagnostics.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:flutter/material.dart';

class DiagnosticPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class OwnedResource extends StatefulWidget {
  const OwnedResource({super.key});
  @override
  State<OwnedResource> createState() => OwnedState();
}

class OwnedState extends State<OwnedResource> implements ResourceDiagnostics {
  @override
  Map<String, int> get resourceDiagnostics => {'owned': 1, 'ownedBytes': 123};
  @override
  Widget build(BuildContext context) => const SizedBox();
}

void main() {
  testWidgets(
    'conversation reports speech and attachment owners only while mounted',
    (tester) async {
      final c = DesktopController(DiagnosticPreferences());
      final monitor = DesktopDiagnostics(
        c,
        File(
          r'D:\codex操作目录\deepseek-flutter-20260923\unused-speech-diagnostics.jsonl',
        ),
      );
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: c)),
        ),
      );
      await tester.pumpAndSettle();
      final owner = monitor.snapshot()['owners'] as Map;
      expect(owner['conversationViews'], 1);
      expect(owner['speechActive'], 0);
      expect(owner['speechTextUnits'], 0);
      expect(owner['pendingAttachmentBytes'], 0);
      await tester.pumpWidget(const SizedBox());
      expect(monitor.snapshot()['owners'], isEmpty);
      monitor.close();
      c.dispose();
    },
  );
  testWidgets(
    'disabled tracing has no writer; mounted counts disappear and draft content never enters metrics',
    (tester) async {
      final prefs = DiagnosticPreferences()
        ..drafts['private-session'] = 'PRIVATE_DRAFT_SENTINEL';
      final c = DesktopController(prefs);
      expect(await DesktopDiagnostics.start(c, null), isNull);
      expect(await DesktopDiagnostics.start(c, ''), isNull);
      final monitor = DesktopDiagnostics(
        c,
        File(
          r'D:\codex操作目录\deepseek-flutter-20260923\never-created-diagnostics.jsonl',
        ),
      );
      await tester.pumpWidget(
        const Directionality(
          textDirection: TextDirection.ltr,
          child: Column(children: [OwnedResource(), OwnedResource()]),
        ),
      );
      final snapshot = monitor.snapshot();
      expect(snapshot['owners'], {'owned': 2, 'ownedBytes': 246});
      expect(jsonEncode(snapshot), isNot(contains('PRIVATE_DRAFT_SENTINEL')));
      expect(jsonEncode(snapshot), isNot(contains('private-session')));
      await tester.pumpWidget(const SizedBox());
      expect(monitor.snapshot()['owners'], isEmpty);
      monitor.close();
      c.dispose();
      expect(tester.takeException(), isNull);
    },
  );
}
