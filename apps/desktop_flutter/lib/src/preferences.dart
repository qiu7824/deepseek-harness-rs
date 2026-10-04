import '../l10n/runtime_zh.dart';

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/foundation.dart' show visibleForTesting;
import 'package:path/path.dart' as p;

import 'desktop_paths.dart';

class DesktopPreferences {
  DesktopPreferences({
    String? address,
    bool? automaticHost,
    this.executable = '',
    this.sessionId,
    this.dark = false,
    Future<void> Function(String content)? writer,
  }) : address = address ?? automaticAddress,
       automaticHost = automaticHost ?? (address == null),
       _writer = writer ?? debugDefaultWriter ?? _writeFile;

  static const automaticAddress = 'http://127.0.0.1:0';
  static const unnamedDraftPrefix = '__dsh_unnamed_draft_v1__:';

  /// Replaces the default destination; the test runner points it away from
  /// the real per-user preferences file.
  @visibleForTesting
  static Future<void> Function(String content)? debugDefaultWriter;
  final Future<void> Function(String content) _writer;
  String address, executable;
  bool automaticHost;
  LocalHostProcess? ownedHost;
  bool _legacyHostConfiguration = false;
  String? sessionId;
  bool dark;
  final Map<String, String> drafts = {};
  Json layout = {};

  static File get file {
    return File(
      DesktopPaths.preferences(Platform.operatingSystem, Platform.environment),
    );
  }

  static Future<DesktopPreferences> load({
    File? fromFile,
    Future<void> Function(String content)? writer,
  }) async {
    final source = fromFile ?? file;
    // Preferences read from an explicit file are saved back to that file.
    final prefs = DesktopPreferences(
      writer:
          writer ??
          (fromFile == null ? null : (content) => _writeTo(fromFile, content)),
    );
    if (await source.exists()) {
      final data = object(jsonDecode(await source.readAsString()));
      prefs.address = data['address'] as String? ?? prefs.address;
      // Defer the legacy default migration until bundled Host discovery. A
      // custom address must still be usable without any bundled executable.
      prefs._legacyHostConfiguration = !data.containsKey('automaticHost');
      prefs.automaticHost = data['automaticHost'] is bool
          ? data['automaticHost'] as bool
          : !data.containsKey('address');
      prefs.ownedHost = LocalHostProcess.fromSaved(data['ownedHost']);
      prefs.executable = data['executable'] as String? ?? '';
      prefs.sessionId = data['sessionId'] as String?;
      prefs.dark = data['dark'] == true;
      prefs.layout = object(data['layout']);
      for (final entry in object(data['drafts']).entries) {
        if (entry.value is String) {
          prefs.drafts[entry.key] = entry.value as String;
        }
      }
    }
    return prefs;
  }

  void migrateBundledDefaults(String? bundledExecutable) {
    if (!_legacyHostConfiguration || bundledExecutable == null) return;
    _legacyHostConfiguration = false;
    final uri = Uri.tryParse(address);
    if (uri == null || uri.scheme != 'http' ||
        !['127.0.0.1', 'localhost'].contains(uri.host) || uri.port != 58080 ||
        uri.userInfo.isNotEmpty || uri.hasQuery || uri.hasFragment ||
        (uri.path.isNotEmpty && uri.path != '/')) {
      return;
    }
    // Legacy initialize always auto-started the bundled Host at this default
    // address. Preserve that behavior when upgrading a normal installation.
    automaticHost = true;
    copyUnnamedDrafts(uri.origin, automaticAddress);
  }

  /// Preserve existing target drafts and keep the source slots available.
  void copyUnnamedDrafts(String sourceOrigin, String targetOrigin) {
    for (final entry in drafts.entries.toList()) {
      if (!entry.key.startsWith(unnamedDraftPrefix)) continue;
      try {
        final scope = jsonDecode(entry.key.substring(unnamedDraftPrefix.length));
        if (scope is! List || scope.length != 2 || scope[0] != sourceOrigin) {
          continue;
        }
        final next = '$unnamedDraftPrefix${jsonEncode([targetOrigin, scope[1]])}';
        drafts.putIfAbsent(next, () => entry.value);
      } catch (_) {
        // Keep unrelated or older draft formats verbatim.
      }
    }
  }

  Future<void> _saving = Future.value();
  Future<void> _enqueue(Future<void> Function() action) {
    _saving = _saving.catchError((Object _) {}).then((_) => action());
    return _saving;
  }

  String _snapshot(Json savedLayout) => jsonEncode({
    'address': address,
    'automaticHost': automaticHost,
    if (ownedHost != null) 'ownedHost': ownedHost!.toJson(),
    'executable': executable,
    'sessionId': sessionId,
    'dark': dark,
    'drafts': drafts,
    'layout': savedLayout,
  });

  Future<void> save() => _enqueue(() => _writer(_snapshot(layout)));

  /// Commits one layout entry after persistence without exposing a draft.
  Future<void> saveLayoutValue(String key, Object? value) {
    final captured = jsonDecode(jsonEncode(value));
    return _enqueue(() async {
      await _writer(_snapshot({...layout, key: captured}));
      layout[key] = captured;
    });
  }

  static Future<void> _writeFile(String content) => _writeTo(file, content);

  static Future<void> _writeTo(File target, String content) async {
    await target.parent.create(recursive: true);
    final temporary = File('${target.path}.${newRequestId()}.tmp');
    await temporary.writeAsString(content, flush: true);
    await temporary.rename(target.path);
  }
}

/// Identity of a Host started by this desktop, never an arbitrary live port.
class LocalHostProcess {
  const LocalHostProcess({
    required this.pid,
    required this.instanceId,
    required this.address,
    required this.executable,
    required this.home,
    required this.hostVersion,
    this.logFile,
    this.process,
  });

  final int pid;
  final String instanceId, address, executable, home, hostVersion;
  final String? logFile;
  /// The original attached child handle; saved ownership records omit it.
  final Process? process;

  LocalHostProcess withReportedHome(String actualHome) {
    if (!p.isAbsolute(actualHome)) {
      throw const FormatException(DshRuntimeZh.hostReadinessInvalid);
    }
    return LocalHostProcess(
      pid: pid,
      instanceId: instanceId,
      address: address,
      executable: executable,
      home: HostLauncher._identityPath(actualHome),
      hostVersion: hostVersion,
      logFile: logFile,
      process: process,
    );
  }

  /// Only fresh starts retain an OS child handle; saved records never grant
  /// permission to stop a process by its possibly reused PID.
  Future<void> stopStartedProcess() async {
    final child = process;
    if (child == null) throw StateError('No original child process handle.');
    child.kill();
    await child.exitCode.timeout(const Duration(seconds: 5));
  }

  Json toJson() => {
    'pid': pid,
    'instanceId': instanceId,
    'address': address,
    'executable': executable,
    'home': home,
    'hostVersion': hostVersion,
    if (logFile != null) 'logFile': logFile,
  };

  static LocalHostProcess? fromSaved(Object? value) {
    try {
      final data = object(value);
      final pid = data['pid'];
      final instanceId = data['instanceId'];
      final address = data['address'];
      final executable = data['executable'];
      final home = data['home'];
      final version = data['hostVersion'];
      if (pid is! int || pid <= 0 ||
          instanceId is! String || instanceId.isEmpty || instanceId.length > 128 ||
          address is! String || executable is! String || home is! String ||
          version is! String || version.isEmpty ||
          !p.isAbsolute(executable) || !p.isAbsolute(home)) {
        return null;
      }
      HostLauncher.readyUri(address);
      return LocalHostProcess(
        pid: pid, instanceId: instanceId, address: address, executable: executable,
        home: home, hostVersion: version,
        logFile: data['logFile'] is String && p.isAbsolute(data['logFile'] as String)
            ? data['logFile'] as String : null,
      );
    } catch (_) {
      return null;
    }
  }
}

typedef HostProcessStarter = Future<Process> Function(
  String executable, List<String> arguments, String workingDirectory,
);

/// Retains the original startup failure and its bounded local evidence.
class HostStartupException implements Exception {
  const HostStartupException({
    required this.message,
    required this.cause,
    required this.logFile,
    required this.diagnostics,
    this.exitCode,
  });

  final String message, logFile, diagnostics;
  final Object cause;
  final int? exitCode;

  @override
  String toString() => jsonEncode({
    'kind': 'error',
    'error': {
      'code': 'host-startup',
      'message': message,
      'details': {
        'logFile': logFile,
        'diagnostics': diagnostics,
        'exitCode': exitCode,
      },
    },
  });
}

/// Fixed-size byte ring; stream chunks can be large or split UTF-8 characters.
class _StartupByteTail {
  _StartupByteTail(int capacity) : _bytes = Uint8List(capacity);
  final Uint8List _bytes;
  int _start = 0, _length = 0;

  void add(List<int> chunk) {
    final capacity = _bytes.length;
    if (chunk.length >= capacity) {
      _bytes.setRange(0, capacity, chunk, chunk.length - capacity);
      _start = 0;
      _length = capacity;
      return;
    }
    final end = (_start + _length) % capacity;
    final first = chunk.length < capacity - end
        ? chunk.length : capacity - end;
    _bytes.setRange(end, end + first, chunk);
    _bytes.setRange(0, chunk.length - first, chunk, first);
    final nextLength = _length + chunk.length;
    if (nextLength > capacity) {
      _start = (_start + nextLength - capacity) % capacity;
      _length = capacity;
    } else {
      _length = nextLength;
    }
  }

  String get text {
    final ordered = Uint8List(_length);
    final first = _length < _bytes.length - _start
        ? _length : _bytes.length - _start;
    ordered.setRange(0, first, _bytes, _start);
    ordered.setRange(first, _length, _bytes);
    // A byte bound may cut off the beginning of one UTF-8 code point.
    var offset = 0;
    while (offset < ordered.length && (ordered[offset] & 0xc0) == 0x80) {
      offset++;
    }
    return utf8.decode(ordered.sublist(offset), allowMalformed: true);
  }
}

class _StartupStreamCapture {
  _StartupStreamCapture(Stream<List<int>> stream, int limit)
      : _tail = _StartupByteTail(limit) {
    stream.listen(
      (chunk) => _tail?.add(chunk),
      onError: (Object _) => _complete(),
      onDone: _complete,
    );
  }
  _StartupByteTail? _tail;
  final _completed = Completer<void>();
  Future<void> get done => _completed.future;
  String get text => _tail?.text ?? '';
  void discard() => _tail = null;
  void _complete() {
    if (!_completed.isCompleted) _completed.complete();
  }
}

class HostLauncher {
  @visibleForTesting
  static const startupDiagnosticByteLimit = 8192;

  static Future<String> _logTail(String path) async {
    RandomAccessFile? reader;
    try {
      reader = await File(path).open();
      final length = await reader.length();
      await reader.setPosition(length > startupDiagnosticByteLimit
          ? length - startupDiagnosticByteLimit : 0);
      final tail = _StartupByteTail(startupDiagnosticByteLimit)
        ..add(await reader.read(startupDiagnosticByteLimit));
      return tail.text.replaceAll('dsh: desktop Host starting\n', '').trim();
    } catch (_) {
      return '';
    } finally {
      try { await reader?.close(); } catch (_) {}
    }
  }

  static String _startupDiagnostics(String log, String stderr, String stdout) {
    var remaining = startupDiagnosticByteLimit;
    final sections = <String>[];
    for (final entry in [
      (DshRuntimeZh.hostStartupLogDetails, log),
      (DshRuntimeZh.hostStartupStderrDetails, stderr),
      (DshRuntimeZh.hostStartupStdoutDetails, stdout),
    ]) {
      final content = entry.$2.trim();
      if (content.isEmpty) continue;
      final header = '${sections.isEmpty ? '' : '\n\n'}${entry.$1}\n';
      final headerBytes = utf8.encode(header).length;
      if (remaining <= headerBytes) break;
      final tail = _StartupByteTail(remaining - headerBytes)
        ..add(utf8.encode(content));
      final section = header + tail.text;
      sections.add(section);
      remaining -= utf8.encode(section).length;
    }
    return sections.join();
  }

  static String _startupMessage(Object error, String log, String stderr) {
    // Only exact Host/native failure signatures from its log or stderr may
    // become the summary. Ordinary stdout is retained solely as diagnostics.
    final lines = [...log.split('\n'), ...stderr.split('\n')]
        .map((line) => line.trim());
    if (lines.any((line) => line == '该数据目录正在使用，请先关闭其它 Harness 实例')) {
      return DshRuntimeZh.hostStartupHomeInUse;
    }
    final denied = RegExp(
      r'^(?:(?:dsh: )?(?:desktop stdio redirection failed: |readiness publication failed: ).+|.*(?:settings\.json|\.runtime-paths\.json|\.dsh-home-redirect\.json): .+|Access is denied\.|Permission denied|拒绝访问。|权限被拒绝) \(os error (?:5|13)\)$',
    );
    if (lines.any(denied.hasMatch)) {
      return DshRuntimeZh.hostStartupPermissionDenied;
    }
    if (lines.any((line) => line.startsWith('dsh: readiness publication failed: ') ||
        line.startsWith('dsh: --ready-file '))) {
      return DshRuntimeZh.hostStartupReadinessFailed;
    }
    return error is StateError ? error.message.toString()
        : error is FormatException ? error.message.toString()
        : DshRuntimeZh.hostStartupFailed(error);
  }

  static String? bundledAt(String executableDir) {
    for (final host in DesktopPaths.bundledHostRoots(
      executableDir,
      Platform.operatingSystem,
    )) {
      final executable = File(
        p.join(host, DesktopPaths.hostName(Platform.operatingSystem)),
      );
      if (executable.existsSync() &&
          File(p.join(host, 'PACKAGE.json')).existsSync() &&
          File(p.join(host, 'web', 'dist', 'index.html')).existsSync() &&
          File(
            p.join(
              host,
              'runtime',
              'node',
              DesktopPaths.nodeName(Platform.operatingSystem),
            ),
          ).existsSync()) {
        return executable.path;
      }
    }
    return null;
  }

  static String? bundled() =>
      bundledAt(File(Platform.resolvedExecutable).parent.path);

  static String discover() {
    final executableDir = File(Platform.resolvedExecutable).parent.path;
    final included = bundledAt(executableDir);
    if (included != null) return included;
    final candidates = <String>[
      p.join(executableDir, DesktopPaths.hostName(Platform.operatingSystem)),
      if (Platform.isWindows)
        for (final drive in ['D:', 'C:'])
          '$drive\\Program Files (x86)\\DeepSeek Harness-rs\\core\\deepseek-harness-rs.exe',
      if (!Platform.isWindows)
        for (final root in ['/opt', '/usr/local/lib'])
          p.join(root, 'deepseek-harness-rs', 'core', 'deepseek-harness-rs'),
    ];
    return candidates.where((path) => File(path).existsSync()).firstOrNull ??
        '';
  }

  static Uri readyUri(String address) {
    final uri = localHostUri(address);
    if (uri.scheme != 'http' || uri.host != '127.0.0.1' ||
        !uri.hasPort || uri.port <= 0 || uri.port > 65535) {
      throw const FormatException(DshRuntimeZh.hostReadinessInvalid);
    }
    return uri;
  }

  static String _identityPath(String path) {
    var result = path;
    try {
      result = File(path).resolveSymbolicLinksSync();
    } catch (_) {
      // A retired process may leave an inventory pointing at a removed file.
    }
    if (Platform.isWindows) {
      if (result.startsWith(r'\\?\UNC\')) {
        result = r'\\' + result.substring(8);
      } else if (result.startsWith(r'\\?\')) {
        result = result.substring(4);
      }
      result = result.toLowerCase();
    }
    return p.normalize(result);
  }

  /// [processStarter] and [logDirectory] are caller-supplied startup
  /// dependencies, also used to isolate real-process regression fixtures.
  static Future<LocalHostProcess> start(
    String executable,
    String address, {
    Duration readinessTimeout = const Duration(seconds: 30),
    HostProcessStarter? processStarter,
    Directory? logDirectory,
  }) async {
    final uri = localHostUri(address);
    if (uri.scheme != 'http' || uri.host == '::1' || uri.port > 65535) {
      throw const FormatException(DshRuntimeZh.localServiceAddressRequired);
    }
    final file = File(executable);
    if (!file.isAbsolute ||
        !await file.exists() ||
        (Platform.isWindows && !executable.toLowerCase().endsWith('.exe'))) {
      throw FormatException(
        DshRuntimeZh.hostExecutableRequired(
          name: DesktopPaths.hostName(Platform.operatingSystem),
        ),
      );
    }
    if (!Platform.isWindows && ((await file.stat()).mode & 0x49) == 0) {
      throw const FormatException(DshRuntimeZh.hostNotExecutable);
    }
    final logs = (logDirectory ?? Directory(
      p.join(DesktopPreferences.file.parent.path, 'host-logs'),
    )).absolute;
    await logs.create(recursive: true);
    final logFile = p.join(logs.path, '${newRequestId()}.log');
    final directory = await Directory.systemTemp.createTemp('dsh-host-ready-');
    final ready = File(p.join(directory.path, 'ready.json'));
    Process? process;
    Future<int>? exited;
    int? exitCode;
    _StartupStreamCapture? stdout, stderr;
    // A first start prepares the plugin profile before listening, which can
    // take several seconds on slow disks; poll quickly within one deadline.
    try {
      final arguments = [
        'web', '--host', '127.0.0.1', '--port', '${uri.port}',
        '--ready-file', ready.path,
        '--stdio-log', logFile,
      ];
      process = processStarter == null
          ? await Process.start(
              file.path, arguments,
              workingDirectory: file.parent.path,
              mode: ProcessStartMode.normal,
              runInShell: false,
            )
          : await processStarter(file.path, arguments, file.parent.path);
      // Attached startup keeps the original OS process handle. A detached
      // PID alone could refer to another process after an early child exit.
      exited = process.exitCode.then((value) {
        exitCode = value;
        return value;
      });
      stdout = _StartupStreamCapture(process.stdout, startupDiagnosticByteLimit);
      stderr = _StartupStreamCapture(process.stderr, startupDiagnosticByteLimit);
      final deadline = DateTime.now().add(readinessTimeout);
      while (DateTime.now().isBefore(deadline)) {
        if (exitCode != null) {
          throw StateError(DshRuntimeZh.hostExitedBeforeReady(exitCode!));
        }
        if (await ready.exists()) {
          // The CLI publishes this file by rename only after binding. A
          // malformed publication is an error, not permission to probe a
          // fixed port or to read another launcher's readiness file.
          if (await ready.length() > 4096) {
            throw const FormatException(DshRuntimeZh.hostReadinessInvalid);
          }
          final bytes = await ready.readAsBytes();
          if (bytes.length > 4096) {
            throw const FormatException(DshRuntimeZh.hostReadinessInvalid);
          }
          final data = object(jsonDecode(utf8.decode(bytes)));
          final readyAddress = data['url'];
          final instanceId = data['instanceId'];
          final readyExecutable = data['executable'];
          final readyHome = data['home'];
          if (data['version'] is! int || data['version'] != 1 ||
              data['pid'] is! int || data['pid'] != process.pid ||
              instanceId is! String || instanceId.isEmpty || instanceId.length > 128 ||
              readyAddress is! String || readyExecutable is! String ||
              readyHome is! String || !p.isAbsolute(readyExecutable) ||
              !p.isAbsolute(readyHome) ||
              _identityPath(readyExecutable) != _identityPath(file.path)) {
            throw const FormatException(DshRuntimeZh.hostReadinessInvalid);
          }
          final bound = readyUri(readyAddress);
          if (uri.port != 0 && bound.port != uri.port) {
            throw const FormatException(DshRuntimeZh.hostReadinessInvalid);
          }
          final probe = _probe(bound.origin);
          try {
            final remaining = deadline.difference(DateTime.now());
            if (remaining <= Duration.zero) break;
            final description = await probe.call('host.describe').timeout(remaining);
            final host = HostInfo.fromJson(description);
            if (description['processId'] is! int ||
                description['processId'] != process.pid ||
                description['instanceId'] != instanceId ||
                _identityPath(host.home) != _identityPath(readyHome)) {
              throw const FormatException(DshRuntimeZh.hostReadinessInvalid);
            }
            final expected = packagedVersion(executable);
            if (expected != null && host.version != expected) {
              throw StateError(DshRuntimeZh.hostVersionMismatch(
                address: bound.origin, running: host.version, expected: expected,
              ));
            }
            return LocalHostProcess(
              pid: process.pid, instanceId: instanceId, address: bound.origin,
              executable: readyExecutable, home: readyHome,
              hostVersion: host.version, process: process, logFile: logFile,
            );
          } finally {
            await probe.close();
          }
        }
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
      throw StateError(DshRuntimeZh.hostNotReady(processId: process.pid));
    } catch (error, stack) {
      // Use the original attached child's handle, never a decoded or saved PID.
      try {
        if (exitCode == null) process?.kill();
        await exited?.timeout(const Duration(seconds: 5));
      } catch (_) {}
      // exitCode can arrive before the final pipe data. Descendants can also
      // retain inherited pipes, so never wait for EOF without a short bound.
      try {
        await Future.wait([
          if (stdout != null) stdout.done,
          if (stderr != null) stderr.done,
        ]).timeout(const Duration(milliseconds: 200));
      } catch (_) {}
      final logTail = await _logTail(logFile)
          .timeout(const Duration(milliseconds: 250), onTimeout: () => '');
      final diagnostics = _startupDiagnostics(
        logTail, stderr?.text ?? '', stdout?.text ?? '',
      );
      final message = _startupMessage(error, logTail, stderr?.text ?? '');
      Error.throwWithStackTrace(HostStartupException(
        message: message, cause: error, logFile: logFile,
        diagnostics: diagnostics, exitCode: exitCode,
      ), stack);
    } finally {
      // Continue consuming both pipes after readiness/failure, without storing
      // normal runtime output or output from a descendant retaining a pipe.
      stdout?.discard();
      stderr?.discard();
      await directory.delete(recursive: true).catchError((Object _) => directory);
    }
  }

  /// Reuse only the saved desktop-owned process, with the selected inventory.
  static bool matchesOwned(LocalHostProcess owned, Json description) {
    try {
      final host = HostInfo.fromJson(description);
      return description['processId'] is int &&
          description['processId'] == owned.pid &&
          description['instanceId'] == owned.instanceId &&
          host.version == owned.hostVersion &&
          // Storage migration can change the home within this same process.
          // The PID + random instance identity, not an old directory, owns it.
          p.isAbsolute(host.home);
    } catch (_) {
      return false;
    }
  }

  static Future<HostInfo?> reusable(
    LocalHostProcess? owned, String executable,
  ) async {
    if (owned == null ||
        _identityPath(owned.executable) != _identityPath(executable)) {
      return null;
    }
    final expected = packagedVersion(executable);
    if (expected != null && expected != owned.hostVersion) return null;
    final probe = _probe(owned.address);
    try {
      final data = await probe.call('host.describe');
      final host = HostInfo.fromJson(data);
      return matchesOwned(owned, data) ? host : null;
    } catch (_) {
      return null;
    } finally {
      await probe.close();
    }
  }

  static DshClient _probe(String address) => DshClient(
    address,
    timeout: const Duration(seconds: 2),
    connectTimeout: const Duration(milliseconds: 300),
  );

  /// The Host already answering at [address], or null when none does.
  static Future<HostInfo?> live(String address) async {
    final probe = _probe(address);
    try {
      return await probe.describe();
    } catch (_) {
      return null;
    } finally {
      await probe.close();
    }
  }

  /// Version recorded in the package inventory next to [executable].
  static String? packagedVersion(String executable) {
    try {
      final inventory = File(
        p.join(File(executable).parent.path, 'PACKAGE.json'),
      );
      final version = jsonDecode(inventory.readAsStringSync())['version'];
      return version is String && version.isNotEmpty ? version : null;
    } catch (_) {
      return null;
    }
  }
}
