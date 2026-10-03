import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/artifacts_view.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'artifacts_view_test.dart' show ArtifactClient;

Future<ArtifactClient> pumpSummary(
  WidgetTester tester,
  List<Json> entries, {
  VoidCallback? onOpenAll,
}) async {
  final api = ArtifactClient()..entriesOverride = entries;
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: SingleChildScrollView(
          child: ArtifactSummary(
            api: api,
            session: 'session-a',
            onOpenAll: onOpenAll,
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return api;
}

void main() {
  testWidgets('changed files show below the reply with a short list', (
    tester,
  ) async {
    var opened = 0;
    final api = await pumpSummary(tester, [
      for (var i = 0; i < 6; i++)
        {
          'path':
              r'D:\Projects\desktop-client\docs\report-'
              '$i.md',
          'change': i == 5 ? 'deleted' : (i.isEven ? 'created' : 'modified'),
          'size': 2048,
        },
    ], onOpenAll: () => opened++);
    expect(find.text('本会话产物'), findsOneWidget);
    expect(find.text('6 个文件'), findsOneWidget);
    expect(find.byType(ArtifactRow), findsNWidgets(4));
    expect(
      find.textContaining('report-0.md', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('新增'), findsWidgets);
    await tester.tap(find.byKey(const ValueKey('artifact-summary-toggle')));
    await tester.pumpAndSettle();
    expect(find.byType(ArtifactRow), findsNWidgets(6));
    await tester.tap(find.byKey(const ValueKey('artifact-summary-open-all')));
    expect(opened, 1);
    // The card reads once per settled turn instead of polling.
    final reads = api.calls
        .where((call) => call.path == '/__dsh-artifacts/list')
        .length;
    await tester.pump(const Duration(seconds: 10));
    expect(
      api.calls.where((call) => call.path == '/__dsh-artifacts/list').length,
      reads,
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('a session without file changes shows no card', (tester) async {
    await pumpSummary(tester, const []);
    expect(find.byKey(const ValueKey('artifact-summary-card')), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
