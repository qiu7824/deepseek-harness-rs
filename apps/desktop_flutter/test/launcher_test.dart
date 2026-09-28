import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:dsh_desktop/src/desktop_paths.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('bundled Host discovery requires a complete runtime layout', () async {
    final root = await Directory.systemTemp.createTemp('dsh-bundled-host-');
    try {
      final host = Directory('${root.path}${Platform.pathSeparator}host');
      await host.create();
      final hostName = DesktopPaths.hostName(Platform.operatingSystem);
      final nodeName = DesktopPaths.nodeName(Platform.operatingSystem);
      await File('${host.path}${Platform.pathSeparator}$hostName')
          .writeAsBytes([0]);
      expect(HostLauncher.bundledAt(root.path), isNull);
      await File('${host.path}${Platform.pathSeparator}PACKAGE.json')
          .writeAsString('{}');
      final web = Directory(
        '${host.path}${Platform.pathSeparator}web${Platform.pathSeparator}dist',
      );
      await web.create(recursive: true);
      await File('${web.path}${Platform.pathSeparator}index.html')
          .writeAsString('<!doctype html>');
      final node = Directory(
        '${host.path}${Platform.pathSeparator}runtime${Platform.pathSeparator}node',
      );
      await node.create(recursive: true);
      await File('${node.path}${Platform.pathSeparator}$nodeName')
          .writeAsBytes([0]);
      expect(
        HostLauncher.bundledAt(root.path),
        '${host.path}${Platform.pathSeparator}$hostName',
      );
    } finally {
      await root.delete(recursive: true);
    }
  });
  test('packaged Host version comes from the inventory beside it', () async {
    final root = await Directory.systemTemp.createTemp('dsh-host-version-');
    try {
      final executable = '${root.path}${Platform.pathSeparator}host.exe';
      expect(HostLauncher.packagedVersion(executable), isNull);
      final inventory = File(
        '${root.path}${Platform.pathSeparator}PACKAGE.json',
      );
      await inventory.writeAsString('{broken');
      expect(HostLauncher.packagedVersion(executable), isNull);
      await inventory.writeAsString('{"version":"0.1.3-alpha.36"}');
      expect(HostLauncher.packagedVersion(executable), '0.1.3-alpha.36');
    } finally {
      await root.delete(recursive: true);
    }
  });

  test('an idle port is reported promptly instead of after a refused connect', () async {
    final socket = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = socket.port;
    await socket.close();
    final watch = Stopwatch()..start();
    expect(await HostLauncher.live('http://127.0.0.1:$port'), isNull);
    // Windows reports a refused loopback connect only after about two seconds.
    expect(watch.elapsed, lessThan(const Duration(milliseconds: 1500)));
  });

  test('a version mismatch names both Host versions', () {
    final message = hostVersionMismatch(
      'http://127.0.0.1:58080',
      '0.1.3-alpha.12',
      '0.1.3-alpha.36',
    );
    expect(message, contains('0.1.3-alpha.12'));
    expect(message, contains('0.1.3-alpha.36'));
    expect(message, contains('http://127.0.0.1:58080'));
  });

  final executable = Platform.environment['DSH_TEST_BINARY'];
  final home = Platform.environment['DSH_HOME'];
  test(
    'launches an isolated Host and waits for readiness',
    () async {
      expect(home, contains('launcher-fixture'));
      final socket = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      final port = socket.port;
      await socket.close();
      final address = 'http://127.0.0.1:$port';
      final pid = await HostLauncher.start(executable!, address);
      final client = DshClient(address);
      try {
        final host = await client.describe();
        expect(
          host.home.replaceAll('\\', '/').replaceFirst('//?/', ''),
          home!.replaceAll('\\', '/').replaceFirst('//?/', ''),
        );
        expect(await client.sessions(), isEmpty);
      } finally {
        await client.close();
        Process.killPid(pid);
      }
    },
    skip: executable == null
        ? 'Set DSH_TEST_BINARY and an isolated DSH_HOME'
        : false,
    timeout: const Timeout(Duration(seconds: 50)),
  );
}
