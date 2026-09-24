import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

void main() {
  test('opening a running command exposes Stop without a model turn', () async {
    final api = FakeClient();
    final controller = DesktopController(
      MemoryPreferences(),
      clientFactory: (_) => api,
    );
    await controller.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    api.handleRpc = (method, payload, scope) async => {'active': true};
    await controller.select('owner');
    expect(controller.running, isFalse);
    expect(controller.commandRunning, isTrue);
    expect(controller.interruptible, isTrue);
    controller.dispose();
    await Future<void>.delayed(Duration.zero);
  });

  test(
    'stale command snapshots and abandoned selections cannot revive Stop',
    () async {
      final api = FakeClient();
      final controller = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await controller.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      await controller.select('owner');
      final pending = <Completer<Json>>[];
      final scopes = <RequestScope?>[];
      api.handleRpc = (method, payload, scope) {
        expect(method, 'commands.activity');
        final result = Completer<Json>();
        pending.add(result);
        scopes.add(scope);
        return result.future;
      };
      final old = controller.refreshCommandActivity();
      final fresh = controller.refreshCommandActivity();
      expect(scopes.first!.cancelled, isTrue);
      pending[1].complete({'active': false});
      await fresh;
      pending[0].complete({'active': true});
      await old;
      expect(controller.commandRunning, isFalse);
      final abandoned = controller.refreshCommandActivity();
      controller.newConversation();
      expect(scopes.last!.cancelled, isTrue);
      pending.last.complete({'active': true});
      await abandoned;
      expect(controller.commandRunning, isFalse);
      expect(controller.interruptible, isFalse);
      controller.dispose();
      await Future<void>.delayed(Duration.zero);
    },
  );
}
