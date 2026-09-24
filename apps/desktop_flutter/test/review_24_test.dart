import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/features/conversation/streaming_presentation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

void main() {
  test(
    'model selection persists the Host-confirmed default for new sessions',
    () async {
      final calls = <({String method, Json payload})>[];
      final api = FakeClient()
        ..handleCall = (method, payload) async {
          calls.add((method: method, payload: payload));
          return {};
        };
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await c.select('session');
      await c.chooseModel(
        ModelChoice(provider: 'p', id: 'chosen', name: 'Chosen'),
      );
      final saved = calls
          .where((call) => call.method == 'settings.replace')
          .single;
      expect(saved.payload['ns'], 'agent-default-model');
      expect(
        object(saved.payload['section'])['model'],
        c.catalog!.current['model'],
      );
      c.dispose();
    },
  );

  testWidgets(
    'remounting an existing streamed paragraph reveals history immediately',
    (tester) async {
      final content = '已有内容' * 500;
      await tester.pumpWidget(
        MaterialApp(
          home: ProgressiveText(
            text: content,
            streaming: true,
            revealInitial: false,
            builder: (value) => Text(value),
          ),
        ),
      );
      expect(find.text(content), findsOneWidget);
      expect(tester.binding.transientCallbackCount, 0);
      await tester.pumpWidget(const SizedBox());
    },
  );
}
