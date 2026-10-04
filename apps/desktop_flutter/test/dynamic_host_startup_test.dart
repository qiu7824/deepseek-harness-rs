import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/design/error.dart';
import 'package:dsh_desktop/l10n/runtime_zh.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:path/path.dart' as p;
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient;

String nodeExecutable() {
  final name = Platform.isWindows ? 'node.exe' : 'node';
  for (final entry in (Platform.environment['PATH'] ?? '')
      .split(Platform.isWindows ? ';' : ':')) {
    final candidate = File(p.join(entry, name));
    if (candidate.existsSync()) return candidate.resolveSymbolicLinksSync();
  }
  throw StateError('The fixed CI Node runtime must be on PATH.');
}

String dartExecutable() {
  final name = Platform.isWindows ? 'dart.exe' : 'dart';
  var directory = File(Platform.resolvedExecutable).parent.path;
  for (var level = 0; level < 8; level++) {
    for (final candidate in [
      p.join(directory, 'dart-sdk', 'bin', name),
      p.join(directory, 'bin', 'cache', 'dart-sdk', 'bin', name),
    ]) {
      if (File(candidate).existsSync()) return candidate;
    }
    directory = p.dirname(directory);
  }
  throw StateError('The Flutter test runner must have its fixed Dart SDK.');
}

class HostFixture {
  HostFixture(this.directory) : executable = nodeExecutable();
  final Directory directory;
  final String executable;
  final processes = <Process>[];
  final readinessPaths = <String>[];

  Future<LocalHostProcess> launch({
    String mode = 'normal',
    int fakePid = 1,
    Duration timeout = const Duration(seconds: 5),
  }) => HostLauncher.start(
    executable,
    DesktopPreferences.automaticAddress,
    readinessTimeout: timeout,
    logDirectory: Directory(p.join(directory.path, 'logs')),
    processStarter: starter(mode: mode, fakePid: fakePid),
  );

  Future<HostStartupException> failure(String mode) async {
    try {
      await launch(mode: mode);
    } on HostStartupException catch (error) {
      return error;
    }
    throw StateError('The failing fixture unexpectedly became ready.');
  }

  HostProcessStarter starter({String mode = 'normal', int fakePid = 1, int port = 0}) =>
    (executable, arguments, _) async {
      expect(arguments.take(5), ['web', '--host', '127.0.0.1', '--port', '$port']);
      final readyPath = arguments[arguments.indexOf('--ready-file') + 1];
      expect(await File(readyPath).exists(), isFalse);
      readinessPaths.add(readyPath);
      final fixture = File('test/fixtures/dynamic_host_fixture.cjs').absolute;
      final process = await Process.start(
        executable,
        [fixture.path, mode, directory.path, '$fakePid', ...arguments],
        workingDirectory: directory.path,
        runInShell: false,
      );
      processes.add(process);
      return process;
    };

  Future<void> close() async {
    for (final process in processes) {
      process.kill();
      await process.exitCode.timeout(const Duration(seconds: 5));
    }
    await directory.delete(recursive: true);
  }
}

Future<HostFixture> fixture() async {
  final value = HostFixture(
    await Directory.systemTemp.createTemp('dsh-dynamic-launch-fixture-'),
  );
  addTearDown(value.close);
  return value;
}

Future<void> changeReportedHome(LocalHostProcess owner, String home) async {
  final client = HttpClient();
  try {
    final request = await client.postUrl(Uri.parse(owner.address).resolve('/__fixture/home'));
    request.headers.set('x-dsh-fixture-instance', owner.instanceId);
    request.write(jsonEncode({'home': home}));
    final response = await request.close();
    expect(response.statusCode, 200);
    await response.drain<void>();
  } finally {
    client.close(force: true);
  }
}

LocalHostProcess changedIdentity(
  LocalHostProcess original, {
  int? pid,
  String? instanceId,
  String? version,
}) => LocalHostProcess(
  pid: pid ?? original.pid,
  instanceId: instanceId ?? original.instanceId,
  address: original.address,
  executable: original.executable,
  home: original.home,
  hostVersion: version ?? original.hostVersion,
);

void main() {
  group('real process startup', () {
    HttpOverrides? previousHttpOverrides;
    setUp(() {
      // The UI test below initializes Flutter's mock HttpClient at registration.
      // These process fixtures must reach the actual loopback HTTP listeners.
      previousHttpOverrides = HttpOverrides.current;
      HttpOverrides.global = null;
    });
    tearDown(() => HttpOverrides.global = previousHttpOverrides);

    test('two starts allocate distinct ports while an unrelated service is listening', () async {
      // Fixed ports can be unavailable on Windows. Bind a real foreign service
      // on an OS-assigned port so this ownership test stays portable.
      final occupied = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final foreignPort = occupied.port;
      occupied.listen((request) async {
        await request.drain<void>();
        request.response.write('foreign-service');
        await request.response.close();
      });
      addTearDown(() => occupied.close(force: true));
      final hosts = await fixture();
      final launched = await Future.wait([hosts.launch(), hosts.launch()]);
      final ports = launched.map((host) => Uri.parse(host.address).port).toSet();
      expect(ports, hasLength(2));
      expect(ports, isNot(contains(foreignPort)));
      expect(ports, isNot(contains(0)));
      expect(launched.map((host) => host.pid).toSet(),
          hosts.processes.map((process) => process.pid).toSet());
      expect(hosts.readinessPaths.toSet(), hasLength(2));
      for (final host in launched) {
        expect(await HostLauncher.reusable(host, hosts.executable), isNotNull);
        expect(host.logFile, startsWith(p.join(hosts.directory.path, 'logs')));
        expect(await File(host.logFile!).readAsString(), contains('fixture started'));
      }
      final client = HttpClient();
      try {
        final request = await client.getUrl(Uri.parse('http://127.0.0.1:$foreignPort'));
        expect(await utf8.decoder.bind(await request.close()).join(), 'foreign-service');
      } finally {
        client.close(force: true);
      }
      for (final ready in hosts.readinessPaths) {
        expect(await File(ready).parent.exists(), isFalse);
      }
    });

    test('saved ownership requires PID, instance, version and executable', () async {
      final hosts = await fixture();
      final original = await hosts.launch();
      final saved = LocalHostProcess.fromSaved(original.toJson());
      expect(await HostLauncher.reusable(saved, hosts.executable), isNotNull);
      for (final other in [
        changedIdentity(original, pid: original.pid + 1),
        changedIdentity(original, instanceId: 'another-instance'),
        changedIdentity(original, version: 'another-version'),
      ]) {
        expect(await HostLauncher.reusable(other, hosts.executable), isNull);
      }
      expect(await HostLauncher.reusable(original, p.join(hosts.directory.path, 'other.exe')), isNull);
      expect(LocalHostProcess.fromSaved({...original.toJson()}..remove('instanceId')), isNull);
      expect(await HostLauncher.reusable(original, hosts.executable), isNotNull);
    });

    test('wrong ready PID is rejected without stopping that other Host', () async {
      final hosts = await fixture();
      final original = await hosts.launch();
      await expectLater(
        hosts.launch(mode: 'wrong-pid', fakePid: original.pid),
        throwsA(isA<HostStartupException>().having(
          (error) => error.cause, 'cause', isA<FormatException>(),
        )),
      );
      expect(await HostLauncher.reusable(original, hosts.executable), isNotNull);
      expect(await hosts.processes.last.exitCode.timeout(const Duration(seconds: 5)), isNotNull);
    });

    for (final mode in ['malformed', 'oversized', 'wrong-url', 'wrong-instance', 'rpc-wrong-pid']) {
      test('invalid readiness $mode fails and cleans up only its child', () async {
        final hosts = await fixture();
        await expectLater(hosts.launch(mode: mode), throwsA(
          isA<HostStartupException>().having(
            (error) => error.cause, 'cause', isA<FormatException>(),
          ),
        ));
        expect(await hosts.processes.single.exitCode.timeout(const Duration(seconds: 5)), isNotNull);
        expect(await File(hosts.readinessPaths.single).parent.exists(), isFalse);
      });
    }

    test('readiness timeout stops the just-created child and removes its files', () async {
      final hosts = await fixture();
      await expectLater(
        hosts.launch(mode: 'timeout', timeout: const Duration(milliseconds: 300)),
        throwsA(isA<HostStartupException>().having(
          (error) => error.cause, 'cause', isA<StateError>(),
        )),
      );
      expect(await hosts.processes.single.exitCode.timeout(const Duration(seconds: 5)), isNotNull);
      expect(await File(hosts.readinessPaths.single).parent.exists(), isFalse);
    });

    test('a child that exits before readiness fails promptly', () async {
      final hosts = await fixture();
      final watch = Stopwatch()..start();
      final failure = await hosts.failure('exit');
      expect(failure.exitCode, 23);
      expect(failure.cause, isA<StateError>());
      expect(failure.toString(), isNot(contains('Bad state:')));
      expect(DshError.describe(failure).details, contains(failure.logFile));
      expect(watch.elapsed, lessThan(const Duration(seconds: 4)));
      expect(await hosts.processes.single.exitCode, 23);
    });

    test('early stderr is retained when the Host never creates its log', () async {
      final hosts = await fixture();
      final failure = await hosts.failure('early-stderr');
      expect(failure.exitCode, 1);
      expect(failure.diagnostics, contains('--ready-file parent directory'));
      expect(failure.diagnostics, contains('路径错误中文🙂'));
      for (final error in <Object>[failure, failure.toString()]) {
        expect(DshError.describe(error).message, DshRuntimeZh.hostStartupReadinessFailed);
      }
      expect(p.isAbsolute(failure.logFile), isTrue);
      expect(failure.logFile, startsWith(p.join(hosts.directory.path, 'logs')));
      expect(await File(failure.logFile).exists(), isFalse);
      expect(DshError.describe(failure.toString()).details, contains(failure.logFile));
      expect(await File(hosts.readinessPaths.single).parent.exists(), isFalse);
    });

    test('early stdout also explains a child failure', () async {
      final hosts = await fixture();
      final failure = await hosts.failure('early-stdout');
      expect(failure.diagnostics, contains('startup stdout-only failure 中文🙂'));
      expect(DshError.describe(failure).message,
          DshRuntimeZh.hostExitedBeforeReady(1));
    });

    test('ordinary stdout cannot become a trusted failure summary', () async {
      final hosts = await fixture();
      final failure = await hosts.failure('stdout-fake-home-lock');
      for (final error in <Object>[failure, failure.toString()]) {
        final description = DshError.describe(error);
        expect(description.message, DshRuntimeZh.hostExitedBeforeReady(1));
        expect(description.outcomeUnknown, isFalse);
        expect(description.details, contains('该数据目录正在使用'));
      }
    });

    test('an unreadable log preserves the original stderr and exit cause', () async {
      final hosts = await fixture();
      final failure = await hosts.failure('unreadable-log');
      expect(failure.exitCode, 1);
      expect(failure.cause, isA<StateError>());
      expect(failure.diagnostics, contains('desktop stdio redirection failed'));
      expect(failure.diagnostics, contains('Access is denied. (os error 5)'));
      expect(DshError.describe(failure).details, contains(failure.logFile));
      expect(DshError.describe(failure).message,
          DshRuntimeZh.hostStartupPermissionDenied);
      expect(await Directory(failure.logFile).exists(), isTrue);
    });

    test('redirected Host log has priority and includes supplementary stderr', () async {
      final hosts = await fixture();
      final failure = await hosts.failure('log-startup-error');
      expect(failure.exitCode, 1);
      expect(failure.diagnostics, contains('该数据目录正在使用'));
      expect(failure.diagnostics, contains('additional startup stderr'));
      expect(failure.diagnostics.indexOf('该数据目录正在使用'),
          lessThan(failure.diagnostics.indexOf('additional startup stderr')));
      for (final error in <Object>[failure, failure.toString()]) {
        expect(DshError.describe(error).message, DshRuntimeZh.hostStartupHomeInUse);
      }
    });

    for (final mode in ['exit-large-tail', 'log-large-tail', 'exit-malformed-tail']) {
      test('$mode retains final Unicode evidence within the byte limit', () async {
        final hosts = await fixture();
        final failure = await hosts.failure(mode);
        expect(failure.exitCode, 1);
        expect(utf8.encode(failure.diagnostics).length,
            lessThanOrEqualTo(HostLauncher.startupDiagnosticByteLimit));
        expect(failure.diagnostics, isNot(contains('discard-')));
        if (mode != 'exit-malformed-tail') {
          expect(failure.diagnostics, isNot(contains('\uFFFD')));
        }
        expect(failure.diagnostics, contains(mode == 'log-large-tail'
            ? '该数据目录正在使用'
            : 'final stderr 中文🙂 after large output'));
        if (mode == 'log-large-tail') {
          expect(failure.diagnostics.length, greaterThan(500));
          for (final error in <Object>[failure, failure.toString()]) {
            expect(DshError.describe(error).message, DshRuntimeZh.hostStartupHomeInUse);
            expect(DshError.describe(error).details, contains(failure.logFile));
          }
        }
      });
    }

    test('exit diagnostics do not wait indefinitely for inherited pipes', () async {
      final hosts = await fixture();
      final watch = Stopwatch()..start();
      final failure = await hosts.failure('exit-open-pipes');
      expect(failure.exitCode, 1);
      expect(failure.diagnostics, contains('descendant keeps pipes open'));
      expect(watch.elapsed, lessThan(const Duration(seconds: 4)));
    });

    test('normal child remains available after its actual Dart parent exits', () async {
      final directory = await Directory.systemTemp.createTemp('dsh-parent-exit-');
      final ready = File(p.join(directory.path, 'ready.json'));
      final result = await Process.run(
        dartExecutable(),
        [
          File('test/fixtures/host_parent_exit.dart').absolute.path,
          nodeExecutable(),
          File('test/fixtures/dynamic_host_fixture.cjs').absolute.path,
          directory.path, ready.path, p.join(directory.path, 'child.log'),
        ],
        runInShell: false,
      );
      expect(result.exitCode, 0);
      final data = object(jsonDecode(await ready.readAsString()));
      final saved = LocalHostProcess.fromSaved({
        ...data, 'address': data['url'], 'hostVersion': 'fixture',
      })!;
      addTearDown(() async {
        final client = HttpClient();
        try {
          final request = await client.postUrl(Uri.parse(saved.address).resolve('/__fixture/stop'));
          request.headers.set('x-dsh-fixture-instance', saved.instanceId);
          await (await request.close()).drain<void>();
        } finally {
          client.close(force: true);
          await directory.delete(recursive: true);
        }
      });
      expect(await HostLauncher.reusable(saved, nodeExecutable()), isNotNull);
      expect(await File(p.join(directory.path, 'child.log')).readAsString(),
          contains('fixture request after startup'));
      await expectLater(saved.stopStartedProcess(), throwsStateError);
    });

    test('readiness accepts only a nonzero IPv4 loopback origin', () {
      for (final address in [
        'http://127.0.0.1:0', 'http://localhost:1234', 'http://[::1]:1234',
        'https://127.0.0.1:1234', 'http://user@127.0.0.1:1234',
        'http://127.0.0.1:1234/api', 'http://127.0.0.1:1234?key=value',
        'http://127.0.0.1:1234#fragment',
      ]) {
        expect(() => HostLauncher.readyUri(address), throwsFormatException);
      }
      expect(HostLauncher.readyUri('http://127.0.0.1:1234/').port, 1234);
    });

    test('new defaults are automatic but old explicit addresses remain manual', () async {
      final directory = await Directory.systemTemp.createTemp('dsh-start-preferences-');
      addTearDown(() => directory.delete(recursive: true));
      final file = File(p.join(directory.path, 'preferences.json'));
      final fresh = await DesktopPreferences.load(fromFile: file);
      expect(fresh.automaticHost, isTrue);
      expect(fresh.address, DesktopPreferences.automaticAddress);
      for (final address in ['http://127.0.0.1:58080', 'https://localhost:61234/']) {
        await file.writeAsString(jsonEncode({'address': address, 'executable': 'custom-host'}));
        final legacy = await DesktopPreferences.load(fromFile: file);
        expect(legacy.automaticHost, isFalse);
        expect(legacy.address, address);
        expect(legacy.executable, 'custom-host');
        await legacy.save();
        expect((await DesktopPreferences.load(fromFile: file)).automaticHost, isFalse);
      }
      final hosts = await fixture();
      final owned = await hosts.launch();
      fresh.address = owned.address;
      fresh.ownedHost = owned;
      await fresh.save();
      final reopened = await DesktopPreferences.load(fromFile: file);
      expect(reopened.automaticHost, isTrue);
      expect(await HostLauncher.reusable(reopened.ownedHost, hosts.executable), isNotNull);
    });

    test('legacy bundled default cold-starts and reuses its migrated Host', () async {
      final hosts = await fixture();
      final file = File(p.join(hosts.directory.path, 'preferences.json'));
      final oldDraft = '${DesktopController.unnamedDraftPrefix}${jsonEncode(['http://127.0.0.1:58080', null])}';
      await file.writeAsString(jsonEncode({
        'address': 'http://127.0.0.1:58080',
        'executable': hosts.executable,
        'drafts': {oldDraft: 'old unsent draft', 'saved-session': 'saved session draft'},
      }));
      final prefs = await DesktopPreferences.load(fromFile: file);
      DesktopController controller(DesktopPreferences preferences) => DesktopController(
        preferences,
        bundledHostFinder: () => hosts.executable,
        hostProcessStarter: hosts.starter(),
        hostLogDirectory: Directory(p.join(hosts.directory.path, 'logs')),
        clientFactory: (_) => FakeClient()..handleCall = (method, _) async {
          if (method != 'host.describe') return {'items': [], 'archivedSessionIds': []};
          final probe = DshClient(preferences.ownedHost!.address);
          try {
            return await probe.call('host.describe');
          } finally {
            await probe.close();
          }
        },
      );
      final first = controller(prefs);
      await first.initialize();
      expect(first.error, isNull);
      expect(prefs.automaticHost, isTrue);
      expect(Uri.parse(prefs.address).port, greaterThan(0));
      expect(first.draft, 'old unsent draft');
      expect(prefs.drafts['saved-session'], 'saved session draft');
      final original = prefs.ownedHost!;
      final migrated = await Directory(p.join(hosts.directory.path, 'migrated-home')).create();
      await changeReportedHome(original, migrated.path);
      first.dispose();
      expect(await HostLauncher.reusable(original, hosts.executable), isNotNull);
      final reopened = await DesktopPreferences.load(fromFile: file);
      final second = controller(reopened);
      await second.initialize();
      expect(second.error, isNull);
      expect(second.preferences.ownedHost!.instanceId, original.instanceId);
      expect(second.preferences.ownedHost!.pid, original.pid);
      expect(second.preferences.ownedHost!.address, original.address);
      expect(second.preferences.ownedHost!.executable, original.executable);
      expect(second.preferences.ownedHost!.logFile, original.logFile);
      expect(second.preferences.ownedHost!.home, endsWith('migrated-home'));
      expect(second.host!.home, migrated.path);
      final savedMigration = await DesktopPreferences.load(fromFile: file);
      expect(savedMigration.ownedHost!.home, endsWith('migrated-home'));
      expect(hosts.processes, hasLength(1));
      expect(second.draft, 'old unsent draft');
      second.dispose();
    });

    test('first manual start carries unassigned workspace drafts to its actual URL', () async {
      final hosts = await fixture();
      // Manual startup still requests an explicit nonzero port. Ask the OS for
      // an allowed port instead of assuming the historical 58080 is bindable.
      final reservation = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      final port = reservation.port;
      expect(port, greaterThan(0));
      await reservation.close();
      final address = 'http://127.0.0.1:$port';
      final prefs = DesktopPreferences(
        executable: hosts.executable,
        writer: (_) async {},
      );
      final api = FakeClient()..handleCall = (_, _) async => {
        'items': [{'workspaceId': 'one', 'path': hosts.directory.path}],
        'archivedSessionIds': [],
      };
      final controller = DesktopController(
        prefs,
        hostProcessStarter: hosts.starter(port: port),
        hostLogDirectory: Directory(p.join(hosts.directory.path, 'logs')),
        clientFactory: (_) => api,
      );
      addTearDown(controller.dispose);
      controller.targetWorkspace('one');
      controller.setDraft('before choosing a manual service');
      final originalKey = controller.draftScopeKey;
      // Match the settings dialog's mutations before its Start action.
      prefs.automaticHost = false;
      prefs.address = address;
      await controller.startHost();
      await Future<void>.delayed(Duration.zero);
      controller.targetWorkspace('one');
      expect(controller.draft, 'before choosing a manual service');
      expect(controller.draftScopeKey, isNot(originalKey));
      expect(prefs.drafts[originalKey], 'before choosing a manual service');
      expect(prefs.ownedHost!.address, address);
      expect(prefs.automaticHost, isFalse);
      expect(hosts.processes, hasLength(1));
    });

    test('a late automatic start cannot replace a completed manual connection', () async {
      final hosts = await fixture();
      final entered = Completer<void>();
      final release = Completer<void>();
      final starter = hosts.starter();
      final prefs = DesktopPreferences(
        executable: hosts.executable,
        writer: (_) async {},
      );
      final api = FakeClient();
      final controller = DesktopController(
        prefs,
        hostProcessStarter: (executable, arguments, directory) async {
          entered.complete();
          await release.future;
          return starter(executable, arguments, directory);
        },
        hostLogDirectory: Directory(p.join(hosts.directory.path, 'logs')),
        clientFactory: (_) => api,
      );
      addTearDown(controller.dispose);
      final starting = controller.startHost();
      await entered.future;
      await controller.connect('http://127.0.0.1:61234');
      controller.setDraft('the selected manual Host');
      final key = controller.draftScopeKey;
      release.complete();
      await starting;
      expect(controller.client, same(api));
      expect(prefs.address, 'http://127.0.0.1:61234');
      expect(prefs.automaticHost, isFalse);
      expect(prefs.ownedHost, isNull);
      expect(controller.connecting, isFalse);
      expect(controller.draftScopeKey, key);
      expect(controller.draft, 'the selected manual Host');
      expect(hosts.processes, hasLength(1));
      expect(await hosts.processes.single.exitCode.timeout(const Duration(seconds: 5)), isA<int>());
    });

    test('a manual connection during ownership save retires only the old child', () async {
      final hosts = await fixture();
      final saving = Completer<void>();
      final release = Completer<void>();
      var firstSave = true;
      final prefs = DesktopPreferences(
        executable: hosts.executable,
        writer: (_) async {
          if (!firstSave) return;
          firstSave = false;
          saving.complete();
          await release.future;
        },
      );
      final api = FakeClient();
      final controller = DesktopController(
        prefs,
        hostProcessStarter: hosts.starter(),
        hostLogDirectory: Directory(p.join(hosts.directory.path, 'logs')),
        clientFactory: (_) => api,
      );
      addTearDown(controller.dispose);
      final starting = controller.startHost();
      await saving.future;
      final connecting = controller.connect('http://127.0.0.1:61234');
      await Future<void>.delayed(Duration.zero);
      release.complete();
      await Future.wait([starting, connecting]);
      expect(controller.client, same(api));
      expect(prefs.address, 'http://127.0.0.1:61234');
      expect(prefs.automaticHost, isFalse);
      expect(prefs.ownedHost, isNull);
      expect(controller.connecting, isFalse);
      expect(hosts.processes, hasLength(1));
      expect(await hosts.processes.single.exitCode.timeout(const Duration(seconds: 5)), isA<int>());
    });

    test('fresh readiness still reports a failure of its final identity check', () async {
      final hosts = await fixture();
      final prefs = DesktopPreferences(
        executable: hosts.executable,
        writer: (_) async {},
      );
      final api = FakeClient()..handleCall = (_, _) async => {
        'processId': prefs.ownedHost!.pid,
        'instanceId': 'a-replacement-instance',
        'version': prefs.ownedHost!.hostVersion,
        'home': prefs.ownedHost!.home,
      };
      final controller = DesktopController(
        prefs,
        hostProcessStarter: hosts.starter(),
        hostLogDirectory: Directory(p.join(hosts.directory.path, 'logs')),
        clientFactory: (_) => api,
      );
      addTearDown(controller.dispose);
      await expectLater(controller.startHost(), throwsFormatException);
      expect(controller.host, isNull);
      expect(controller.client, isNull);
      expect(controller.connecting, isFalse);
      expect(api.channels, isEmpty);
      expect(await HostLauncher.reusable(prefs.ownedHost, hosts.executable), isNotNull);
    });

    test('same identified process still cannot publish a relative home', () async {
      final hosts = await fixture();
      final original = await hosts.launch();
      await changeReportedHome(original, 'relative-home');
      expect(await HostLauncher.reusable(original, hosts.executable), isNull);
      final prefs = DesktopPreferences(automaticHost: true)..ownedHost = original;
      final api = FakeClient()..handleCall = (method, _) async {
        final probe = DshClient(original.address);
        try {
          return await probe.call(method);
        } finally {
          await probe.close();
        }
      };
      final controller = DesktopController(prefs, clientFactory: (_) => api);
      await expectLater(controller.connect(original.address, desktopOwned: true), throwsFormatException);
      expect(controller.host, isNull);
      expect(api.channels, isEmpty);
      expect(prefs.ownedHost!.home, original.home);
      expect(hosts.processes, hasLength(1));
      controller.dispose();
      await changeReportedHome(original, original.home);
      expect(await HostLauncher.reusable(original, hosts.executable), isNotNull);
    });

    test('migration keeps explicit manual settings and newer draft conflicts', () async {
      final hosts = await fixture();
      final file = File(p.join(hosts.directory.path, 'preferences.json'));
      for (final address in ['https://localhost:58080', 'http://127.0.0.1:61234']) {
        await file.writeAsString(jsonEncode({'address': address}));
        final custom = await DesktopPreferences.load(fromFile: file);
        custom.migrateBundledDefaults(hosts.executable);
        expect(custom.automaticHost, isFalse);
        expect(custom.address, address);
      }
      await file.writeAsString(jsonEncode({'address': 'http://127.0.0.1:58080', 'automaticHost': false}));
      final manual = await DesktopPreferences.load(fromFile: file);
      manual.migrateBundledDefaults(hosts.executable);
      expect(manual.automaticHost, isFalse);
      final prefix = DesktopController.unnamedDraftPrefix;
      final oldKey = '$prefix${jsonEncode(['http://localhost:58080', 'workspace'])}';
      final newKey = '$prefix${jsonEncode([DesktopPreferences.automaticAddress, 'workspace'])}';
      await file.writeAsString(jsonEncode({
        'address': 'http://localhost:58080/',
        'drafts': {oldKey: 'old draft', newKey: 'newer draft'},
      }));
      final legacy = await DesktopPreferences.load(fromFile: file);
      legacy.migrateBundledDefaults(hosts.executable);
      expect(legacy.automaticHost, isTrue);
      expect(legacy.drafts[oldKey], 'old draft');
      expect(legacy.drafts[newKey], 'newer draft');
    });

    test('final connection rejects a replacement Host before subscribing', () async {
      final hosts = await fixture();
      final original = await hosts.launch();
      final prefs = DesktopPreferences(automaticHost: true)..ownedHost = original;
      final api = FakeClient()..handleCall = (_, _) async => {
        'processId': original.pid,
        'instanceId': 'a-replacement-host',
        'version': original.hostVersion,
        'home': original.home,
        'cwd': original.home,
      };
      final controller = DesktopController(prefs, clientFactory: (_) => api);
      await expectLater(controller.connect(original.address, desktopOwned: true), throwsFormatException);
      expect(controller.host, isNull);
      expect(controller.client, isNull);
      expect(api.channels, isEmpty);
      controller.dispose();
      expect(await HostLauncher.reusable(original, hosts.executable), isNotNull);
    });

    test('automatic unsent draft scope survives a new assigned port', () {
      final prefs = DesktopPreferences(writer: (_) async {});
      final controller = DesktopController(prefs);
      addTearDown(controller.dispose);
      controller.setDraft('keep this before the Host is ready');
      final first = controller.unnamedDraftKey;
      prefs.address = 'http://127.0.0.1:12345';
      expect(controller.unnamedDraftKey, first);
      prefs.address = 'http://127.0.0.1:54321';
      expect(controller.unnamedDraftKey, first);
      expect(controller.draft, 'keep this before the Host is ready');
    });

  });

  testWidgets('first manual connection from settings keeps its preconnection workspace draft', (tester) async {
    final prefs = DesktopPreferences(writer: (_) async {});
    final api = FakeClient()..handleCall = (_, _) async => {
      'items': [{'workspaceId': 'one', 'path': 'E:/one'}],
      'archivedSessionIds': [],
    };
    final controller = DesktopController(prefs, clientFactory: (_) => api);
    addTearDown(controller.dispose);
    controller.targetWorkspace('one');
    controller.setDraft('typed before opening connection settings');
    final originalKey = controller.draftScopeKey;
    await tester.pumpWidget(ShadApp(home: Scaffold(body: Builder(
      builder: (context) => DshButton(
        onPressed: () => connectionSettings(context, controller),
        child: const Text('open connection'),
      ),
    ))));
    await tester.tap(find.text('open connection'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('automatic-host')));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('manual-host-address')), 'http://127.0.0.1:61234');
    await tester.tap(find.text('连接'));
    await tester.pumpAndSettle();
    expect(controller.client, same(api));
    expect(prefs.automaticHost, isFalse);
    expect(prefs.address, 'http://127.0.0.1:61234');
    controller.targetWorkspace('one');
    expect(controller.draft, 'typed before opening connection settings');
    expect(controller.draftScopeKey, isNot(originalKey));
    expect(prefs.drafts[originalKey], 'typed before opening connection settings');
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('connection settings preserve the explicit address when toggling auto', (tester) async {
    final prefs = DesktopPreferences(address: 'https://localhost:61234/');
    final controller = DesktopController(prefs);
    await tester.pumpWidget(ShadApp(home: Scaffold(body: Builder(
      builder: (context) => DshButton(
        onPressed: () => connectionSettings(context, controller),
        child: const Text('open connection'),
      ),
    ))));
    await tester.tap(find.text('open connection'));
    await tester.pumpAndSettle();
    final input = find.byKey(const Key('manual-host-address'));
    expect(tester.widget<DshField>(input).controller!.text, prefs.address);
    await tester.tap(find.byKey(const Key('automatic-host')));
    await tester.pumpAndSettle();
    expect(input, findsNothing);
    expect(find.text('端口由本机服务自动分配，无需手动设置。'), findsOneWidget);
    await tester.tap(find.byKey(const Key('automatic-host')));
    await tester.pumpAndSettle();
    expect(tester.widget<DshField>(input).controller!.text, prefs.address);
    expect(prefs.automaticHost, isFalse);
    await tester.tap(find.text('取消'));
    await tester.pumpAndSettle();
    await tester.pumpWidget(const SizedBox());
    controller.dispose();
  });
}
