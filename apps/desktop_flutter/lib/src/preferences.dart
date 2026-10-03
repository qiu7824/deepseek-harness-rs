import '../l10n/runtime_zh.dart';

import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:path/path.dart' as p;

import 'desktop_paths.dart';

class DesktopPreferences {
  DesktopPreferences({
    this.address = 'http://127.0.0.1:58080',
    this.executable = '',
    this.sessionId,
    this.dark = false,
    Future<void> Function(String content)? writer,
  }) : _writer = writer ?? _writeFile;
  final Future<void> Function(String content) _writer;
  String address, executable;
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
    final prefs = DesktopPreferences(writer: writer);
    if (await source.exists()) {
      final data = object(jsonDecode(await source.readAsString()));
      prefs.address = data['address'] as String? ?? prefs.address;
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

  Future<void> _saving = Future.value();
  Future<void> _enqueue(Future<void> Function() action) {
    _saving = _saving.catchError((Object _) {}).then((_) => action());
    return _saving;
  }

  String _snapshot(Json savedLayout) => jsonEncode({
    'address': address,
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

  static Future<void> _writeFile(String content) async {
    await file.parent.create(recursive: true);
    final temporary = File('${file.path}.${newRequestId()}.tmp');
    await temporary.writeAsString(content, flush: true);
    await temporary.rename(file.path);
  }
}

class HostLauncher {
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

  static Future<int> start(String executable, String address) async {
    final uri = localHostUri(address);
    if (uri.port == 0) {
      throw const FormatException(DshRuntimeZh.fixedPortRequired);
    }
    if (uri.scheme != 'http' || uri.host == '::1') {
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
    final portProbe = await ServerSocket.bind(
      InternetAddress.loopbackIPv4,
      uri.port,
    );
    await portProbe.close();
    final process = await Process.start(
      file.path,
      ['web', '--host', '127.0.0.1', '--port', '${uri.port}'],
      workingDirectory: file.parent.path,
      mode: ProcessStartMode.detached,
      runInShell: false,
    );
    // A first start prepares the plugin profile before listening, which can
    // take several seconds on slow disks; poll quickly within one deadline.
    final probe = _probe(address);
    final deadline = DateTime.now().add(const Duration(seconds: 30));
    try {
      while (true) {
        try {
          await probe.describe();
          return process.pid;
        } catch (_) {
          if (DateTime.now().isAfter(deadline)) break;
          await Future<void>.delayed(const Duration(milliseconds: 150));
        }
      }
      throw StateError(DshRuntimeZh.hostNotReady(processId: process.pid));
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
