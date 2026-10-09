import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path/path.dart' as p;

typedef UpdateRunner = Future<Map<String, dynamic>> Function(
  List<String> arguments,
  void Function(String message) progress,
);

/// One update operation per desktop process. Checks never download a package;
/// preparation and installation require separate explicit actions.
class DesktopUpdateController extends ChangeNotifier {
  DesktopUpdateController({this.runner});

  static final instance = DesktopUpdateController();
  final UpdateRunner? runner;
  Future<void>? _operation;
  bool _disposed = false;
  Map<String, dynamic> state = const {};
  String phase = 'idle', message = '';
  String? error;
  DateTime? checkedAt;

  bool get busy => _operation != null;
  String get currentVersion => state['currentVersion'] as String? ?? '';
  String get offeredVersion => state['version'] as String? ?? '';
  bool get ready => state['phase'] == 'ready';
  bool get available => ready || state['phase'] == 'available';
  bool get restartSupported => state['restartSupported'] == true;

  void _emit() {
    if (!_disposed) notifyListeners();
  }

  Future<void> _work(String nextPhase, Future<void> Function() action) {
    final existing = _operation;
    if (existing != null) return existing;
    if (_disposed) return Future.value();
    final completion = Completer<void>();
    _operation = completion.future;
    phase = nextPhase;
    error = null;
    message = '';
    _emit();
    unawaited(() async {
      try {
        await action();
      } catch (failure) {
        error = '$failure'.replaceFirst(
          RegExp(r'^(?:Exception|StateError): '),
          '',
        );
        phase = state['phase'] as String? ?? 'idle';
      } finally {
        _operation = null;
        _emit();
        completion.complete();
      }
    }());
    return completion.future;
  }

  Future<void> check() => _work('checking', () async {
    Map<String, dynamic> result;
    try {
      result = await _call(['check']);
    } catch (_) {
      // Offline checks still expose an already verified package and the most
      // recent install failure, so a network failure cannot hide recovery.
      try {
        state = await _call(['status']);
      } catch (_) {}
      rethrow;
    }
    state = result;
    phase = result['phase'] as String? ?? 'current';
    checkedAt = DateTime.now();
  });

  Future<void> prepare({bool mirror = false}) => _work('downloading', () async {
    if (!available) throw StateError('请先检查更新');
    final result = await _call(['prepare', if (mirror) 'mirror']);
    state = result;
    phase = result['phase'] as String? ?? 'ready';
    message = '下载和校验完成';
  });

  Future<void> restart({
    required Future<void> Function() save,
    required Future<void> Function() close,
    int? desktopPid,
  }) => _work('restarting', () async {
    if (!ready || !restartSupported) throw StateError('更新尚未准备完成');
    // A failed preference write must never close a client with unsaved drafts.
    await save();
    await _call(['restart', '${desktopPid ?? pid}']);
    phase = 'restarting';
    try {
      await close();
    } catch (failure) {
      throw StateError('客户端未能关闭，更新将在等待超时后取消：$failure');
    }
  });

  Future<String?> packagePath() async {
    if (busy || !ready) return null;
    String? path;
    await _work(phase, () async {
      final result = await _call(['package']);
      path = result['path'] as String?;
    });
    return path;
  }

  Future<Map<String, dynamic>> _call(List<String> arguments) async {
    final execute = runner ?? _run;
    return execute(arguments, (progress) {
      if (_disposed) return;
      message = progress;
      _emit();
    });
  }

  static String? installationRoot() {
    var directory = File(Platform.resolvedExecutable).parent;
    for (var depth = 0; depth < 5; depth++) {
      if (File(p.join(directory.path, 'DESKTOP.json')).existsSync()) {
        return directory.path;
      }
      final parent = directory.parent;
      if (parent.path == directory.path) break;
      directory = parent;
    }
    return null;
  }

  static Future<Map<String, dynamic>> _run(
    List<String> arguments,
    void Function(String) progress,
  ) async {
    final root = installationRoot();
    if (root == null) {
      throw StateError('当前运行的是开发构建，请使用完整桌面发行包检查更新');
    }
    final manifest = jsonDecode(
      await File(p.join(root, 'DESKTOP.json')).readAsString(),
    ) as Map<String, dynamic>;
    if (manifest['updaterProtocol'] != 1) {
      throw StateError('当前发行包未包含桌面更新接口，请从版本说明页下载安装完整发行包');
    }
    final hostRoot = manifest['hostRoot'];
    if (hostRoot is! String || p.isAbsolute(hostRoot)) {
      throw const FormatException('桌面更新器目录无效');
    }
    final canonicalRoot = await Directory(root).resolveSymbolicLinks();
    final launcher = File(
      p.join(
        root,
        hostRoot,
        Platform.isWindows ? 'dsh-launcher.exe' : 'dsh-launcher',
      ),
    );
    final executable = await launcher.resolveSymbolicLinks();
    if (!p.isWithin(canonicalRoot, executable)) {
      throw const FormatException('桌面更新器必须位于当前安装目录内');
    }
    final process = await Process.start(
      executable,
      [
        '--desktop-update',
        arguments.first,
        canonicalRoot,
        ...arguments.skip(1),
      ],
      workingDirectory: canonicalRoot,
      runInShell: false,
    );
    Map<String, dynamic>? response;
    String? protocolError;
    var errorBytes = 0;
    final stderr = StringBuffer();
    final errorSubscription = process.stderr.transform(utf8.decoder).listen((
      chunk,
    ) {
      if (errorBytes < 4096) {
        final remaining = 4096 - errorBytes;
        stderr.write(
          chunk.length > remaining ? chunk.substring(0, remaining) : chunk,
        );
        errorBytes += chunk.length;
      }
    });
    try {
      await process.stdout
          .transform(utf8.decoder)
          .transform(const LineSplitter())
          .forEach((line) {
            if (line.length > 1024 * 1024) {
              protocolError = '更新器响应超过大小限制';
              return;
            }
            try {
              final value = jsonDecode(line);
              if (value is! Map<String, dynamic>) return;
              if (value['type'] == 'progress' && value['message'] is String) {
                progress(value['message'] as String);
              } else if (value['type'] == 'result') {
                if (response != null) protocolError = '更新器重复返回操作结果';
                response = value;
              }
            } on FormatException {
              protocolError = '更新器返回了无效响应';
            }
          })
          .timeout(
            const Duration(minutes: 20),
            onTimeout: () {
              process.kill();
              throw TimeoutException('更新操作超时，下载尚未完成');
            },
          );
      final exitCode = await process.exitCode;
      if (protocolError != null) throw StateError(protocolError!);
      if (exitCode != 0 || response == null) {
        throw StateError(
          '更新器未完成操作（退出码 $exitCode）${stderr.isEmpty ? '' : '：$stderr'}',
        );
      }
      if (response!['ok'] != true) {
        throw StateError(response!['error'] as String? ?? '更新操作失败');
      }
      final state = response!['state'];
      if (state is! Map<String, dynamic>) {
        throw const FormatException('更新状态无效');
      }
      return state;
    } finally {
      await errorSubscription.cancel();
    }
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}
