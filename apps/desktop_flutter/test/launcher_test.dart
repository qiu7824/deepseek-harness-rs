import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
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
