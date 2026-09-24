import 'dart:io';
import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter_test/flutter_test.dart';

class LiveModePreferences extends DesktopPreferences {
  LiveModePreferences(String address) : super(address: address);
  @override
  Future<void> save() async {}
}

void main() {
  final address = Platform.environment['DSH_TEST_HOST'],
      cwd = Platform.environment['DSH_TEST_CWD'],
      home = Platform.environment['DSH_TEST_HOME'];
  test(
    'installed Host exposes authoritative mode to controller after commands and reconnect',
    () async {
      final c = DesktopController(LiveModePreferences(address!));
      Future<void> until(bool Function() check) async {
        final end = DateTime.now().add(const Duration(seconds: 20));
        while (!check()) {
          if (DateTime.now().isAfter(end)) {
            fail('Mode did not settle: ${c.error}');
          }
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
      }

      try {
        await c.connect(address);
        expect(home, contains('question-flow-host-qa'));
        expect(c.host!.home.replaceFirst(r'\\?\', ''), home);
        await until(() => c.connected);
        await c.addWorkspace(cwd!);
        c.preset = 'standard';
        await c.create(cwd);
        expect(
          c.planMode,
          isNotNull,
          reason: 'Standard preset must publish the plan projection',
        );
        final id = c.selectedId!;
        await c.setPlanMode(true);
        expect(c.planMode?.active, isTrue);
        expect(c.planMode?.pending, isFalse);
        await c.connect(address);
        await until(() => c.connected && !c.loading && c.selectedId == id);
        expect(c.planMode?.active, isTrue);
        expect(await c.sendParts('/plan off', []), id);
        expect(c.planMode?.active, isFalse);
        final events = (await c.client!.history(id)).events;
        expect(
          events
              .where((e) => e.type == 'command/run' && e.data['name'] == 'plan')
              .length,
          2,
        );
        expect(events.any((e) => e.type == 'assistant/chunk'), isFalse);
        final output = Platform.environment['DSH_QA_RESULT'];
        if (output != null) {
          await File(output).writeAsString(
            jsonEncode({
              'modeOn': true,
              'restoredAfterReconnect': true,
              'slashOffUsesCommand': true,
              'noModelTurnCreated': true,
              'scope': 'headless controller against isolated installed Host; no desktop input',
            }),
          );
        }
      } finally {
        c.dispose();
        await until(() => DshClient.resourceCounts.values.every((v) => v == 0));
      }
    },
    skip: address == null || cwd == null || home == null,
    timeout: const Timeout(Duration(minutes: 1)),
  );
}
