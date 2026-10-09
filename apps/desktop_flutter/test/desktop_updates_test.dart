import 'dart:async';

import 'package:dsh_desktop/src/desktop_updates.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'offline checks retain a verified package and install failure',
    () async {
      final updates = DesktopUpdateController(
        runner: (args, progress) async {
          if (args.first == 'check') throw StateError('offline');
          return {
            'phase': 'ready',
            'version': 'v2.0.0',
            'restartSupported': true,
            'lastResult': {'ok': false, 'error': 'file locked'},
          };
        },
      );
      await updates.check();
      expect(updates.ready, isTrue);
      expect(updates.error, contains('offline'));
      expect((updates.state['lastResult'] as Map)['error'], 'file locked');
      expect(updates.checkedAt, isNull);
      updates.dispose();
    },
  );
  test('concurrent checks share an operation and never download', () async {
    final completion = Completer<Map<String, dynamic>>();
    final calls = <List<String>>[];
    final updates = DesktopUpdateController(
      runner: (args, progress) {
        calls.add(args);
        return completion.future;
      },
    );
    final first = updates.check();
    final second = updates.check();
    expect(identical(first, second), isTrue);
    expect(updates.busy, isTrue);
    completion.complete({'phase': 'available', 'version': 'v2.0.0'});
    await first;
    expect(calls, [
      ['check'],
    ]);
    expect(updates.available, isTrue);
    expect(updates.ready, isFalse);
    expect(updates.busy, isFalse);
    updates.dispose();
  });

  test('download errors preserve the offer and permit retry', () async {
    var attempts = 0;
    final updates = DesktopUpdateController(
      runner: (args, progress) async {
        if (args.first == 'check') {
          return {'phase': 'available', 'version': 'v2.0.0'};
        }
        attempts++;
        if (attempts == 1) throw StateError('checksum failed');
        return {
          'phase': 'ready',
          'version': 'v2.0.0',
          'restartSupported': true,
        };
      },
    );
    await updates.check();
    await updates.prepare();
    expect(updates.available, isTrue);
    expect(updates.ready, isFalse);
    expect(updates.error, contains('checksum failed'));
    await updates.prepare();
    expect(updates.ready, isTrue);
    expect(updates.error, isNull);
    updates.dispose();
  });

  test('failed draft save never arms installation or closes the app', () async {
    final calls = <List<String>>[];
    var closed = false;
    final updates = DesktopUpdateController(
      runner: (args, progress) async {
        calls.add(args);
        return {
          'phase': args.first == 'check' ? 'available' : 'ready',
          'restartSupported': true,
        };
      },
    );
    await updates.check();
    await updates.prepare();
    await updates.restart(
      save: () async => throw StateError('disk full'),
      close: () async => closed = true,
      desktopPid: 123,
    );
    expect(calls.map((row) => row.first), ['check', 'prepare']);
    expect(closed, isFalse);
    expect(updates.ready, isTrue);
    expect(updates.error, contains('disk full'));
    updates.dispose();
  });

  test(
    'blocked runtime keeps the package ready and never closes the app',
    () async {
      var closed = false;
      final updates = DesktopUpdateController(
        runner: (args, progress) async {
          if (args.first == 'restart') throw StateError('Host is running');
          return {
            'phase': args.first == 'check' ? 'available' : 'ready',
            'restartSupported': true,
          };
        },
      );
      await updates.check();
      await updates.prepare();
      await updates.restart(
        save: () async {},
        close: () async => closed = true,
      );
      expect(updates.ready, isTrue);
      expect(closed, isFalse);
      expect(updates.error, contains('Host is running'));
      updates.dispose();
    },
  );

  test(
    'restart saves first, arms helper next, then requests normal close',
    () async {
      final order = <String>[];
      final updates = DesktopUpdateController(
        runner: (args, progress) async {
          order.add(args.first);
          return {
            'phase': args.first == 'check' ? 'available' : 'ready',
            'restartSupported': true,
          };
        },
      );
      await updates.check();
      await updates.prepare();
      await updates.restart(
        save: () async => order.add('save'),
        close: () async => order.add('close'),
        desktopPid: 123,
      );
      expect(order, ['check', 'prepare', 'save', 'restart', 'close']);
      updates.dispose();
    },
  );
}
