import 'dart:async';
import 'dart:io';

// An actual Dart parent exits after starting a normal child. This reproduces
// the process/pipe lifetime separately from the Flutter test runner's lifetime.
Future<void> main(List<String> arguments) async {
  final process = await Process.start(
    arguments[0],
    [
      arguments[1], 'normal', arguments[2], '1',
      'web', '--host', '127.0.0.1', '--port', '0',
      '--ready-file', arguments[3],
      '--stdio-log', arguments[4],
    ],
    runInShell: false,
    mode: ProcessStartMode.normal,
  );
  unawaited(process.stdout.drain<void>().catchError((Object _) {}));
  unawaited(process.stderr.drain<void>().catchError((Object _) {}));
  final deadline = DateTime.now().add(const Duration(seconds: 5));
  while (DateTime.now().isBefore(deadline)) {
    if (await File(arguments[3]).exists()) exit(0);
    await Future<void>.delayed(const Duration(milliseconds: 25));
  }
  process.kill();
  await process.exitCode;
  exit(1);
}
