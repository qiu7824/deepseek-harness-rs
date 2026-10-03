import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../integration_test/experience_stream_soak_test.dart';

void main() {
  testWidgets(
    'stream soak emits real frames, coalesces updates and preserves final replies',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1280, 800));
      final api = StreamSoakClient();
      final controller = DesktopController(
        StreamSoakPreferences(),
        clientFactory: (_) => api,
      );
      addTearDown(() async {
        await tester.pumpWidget(const SizedBox());
        controller.dispose();
        await tester.pump();
        await tester.binding.setSurfaceSize(null);
      });
      await controller.connect('http://127.0.0.1:1');
      await controller.select(StreamSoakClient.session);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: Conversation(controller: controller)),
        ),
      );
      await tester.pumpAndSettle();
      expect(controller.transcript.length, 800);
      final result = await driveStreamSoak(
        tester,
        controller,
        api,
        seconds: 12,
        nowMicros: () => tester.binding.clock.now().microsecondsSinceEpoch,
      );
      expect(result['deltas'], 360);
      expect(result['completedReplies'], 2);
      expect(result['messageNotifications'], lessThan(360));
      expect(api.eventEnvelopes, greaterThan(360));
      expect(controller.error, isNull);
      expect(tester.takeException(), isNull);
    },
  );
}
