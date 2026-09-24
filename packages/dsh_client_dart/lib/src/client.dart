import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'models.dart';
import 'resources.dart';
import 'display_path.dart';

class DshException implements Exception {
  DshException(this.code, this.message, {this.outcomeUnknown = false});
  final String code, message;
  final bool outcomeUnknown;
  @override
  String toString() => displayPathText(
    '$message ($code)${outcomeUnknown ? '；操作结果尚未确认，请核对任务状态后再试。' : ''}',
  );
}

String newRequestId() {
  final random = Random.secure();
  return List.generate(
    16,
    (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
  ).join();
}

Uri localHostUri(String address) {
  final uri = Uri.parse(address.trim());
  if (!['http', 'https'].contains(uri.scheme) ||
      !['127.0.0.1', 'localhost', '::1'].contains(uri.host) ||
      uri.userInfo.isNotEmpty ||
      uri.hasQuery ||
      uri.hasFragment ||
      (uri.path.isNotEmpty && uri.path != '/')) {
    throw const FormatException('请输入本机服务地址，例如 http://127.0.0.1:58080');
  }
  return uri;
}

class DshClient {
  static int _globalActiveRequests = 0;
  static Map<String, int> get resourceCounts => {
    'httpRequests': _globalActiveRequests,
    'eventChannels': EventChannel._liveChannels,
    'eventSockets': EventChannel._liveSockets,
  };
  DshClient(String address, {this.timeout = const Duration(seconds: 30)})
    : baseUri = localHostUri(address);
  final Uri baseUri;
  final Duration timeout;
  final Set<HttpClient> _requests = {};
  bool _closed = false;
  final List<EventChannel> _channels = [];
  int get activeRequests => _requests.length;

  /// Save original bytes with disk backpressure and commit only a complete file.
  Future<int> downloadTo(
    String path,
    File destination, {
    RequestScope? scope,
    int maxBytes = 64 * 1024 * 1024,
    Duration? totalTimeout,
    void Function(int bytes)? onProgress,
  }) async {
    final uri = baseUri.resolve(path);
    if (uri.origin != baseUri.origin || uri.userInfo.isNotEmpty) {
      throw ArgumentError('Host requests must remain on the connected origin');
    }
    final http = HttpClient()..connectionTimeout = const Duration(seconds: 5);
    final temporary = File(
      '${destination.absolute.path}.dsh-${newRequestId()}.part',
    );
    RandomAccessFile? output;
    var received = 0, expired = false;
    void check() {
      if (_closed || scope?.cancelled == true) {
        throw DshException('cancelled', '保存已取消');
      }
      if (expired) throw DshException('timeout', '保存超时');
    }

    check();
    _requests.add(http);
    _globalActiveRequests++;
    final unregister = scope?.register(() => http.close(force: true));
    final timer = Timer(totalTimeout ?? timeout, () {
      expired = true;
      http.close(force: true);
    });
    try {
      final request = await http.getUrl(uri);
      request.followRedirects = false;
      final response = await request.close();
      check();
      if (response.statusCode != 200) {
        throw DshException(
          'http-${response.statusCode}',
          '无法读取原文件（HTTP ${response.statusCode}）',
        );
      }
      if (response.contentLength > maxBytes) {
        throw DshException('response-limit', '原文件超过保存上限');
      }
      output = await temporary.open(mode: FileMode.writeOnly);
      await for (final part in response) {
        check();
        received += part.length;
        if (received > maxBytes) {
          throw DshException('response-limit', '原文件超过保存上限');
        }
        await output.writeFrom(part);
        onProgress?.call(received);
      }
      check();
      if (response.contentLength >= 0 && received != response.contentLength) {
        throw DshException('incomplete', '文件未完整接收');
      }
      await output.flush();
      await output.close();
      output = null;
      check();
      await temporary.rename(destination.absolute.path);
      return received;
    } on DshException {
      rethrow;
    } catch (error) {
      check();
      throw DshException('download', '保存失败：$error');
    } finally {
      timer.cancel();
      unregister?.call();
      http.close(force: true);
      if (_requests.remove(http)) _globalActiveRequests--;
      if (output != null) await output.close();
      if (await temporary.exists()) await temporary.delete();
    }
  }

  Future<Uint8List> bytes(
    String path, {
    Json? body,
    RequestScope? scope,
    int maxBytes = 16 * 1024 * 1024,
    bool mutation = false,
  }) async {
    final uri = baseUri.resolve(path);
    if (uri.origin != baseUri.origin || uri.userInfo.isNotEmpty) {
      throw ArgumentError('Host requests must remain on the connected origin');
    }
    if (_closed || scope?.cancelled == true) {
      throw DshException('cancelled', '读取已取消');
    }
    final http = HttpClient()..connectionTimeout = const Duration(seconds: 5);
    _requests.add(http);
    _globalActiveRequests++;
    final unregister = scope?.register(() => http.close(force: true));
    HttpClientRequest? request;
    var dispatched = false;
    try {
      return await (() async {
        request = await http.openUrl(body == null ? 'GET' : 'POST', uri);
        request!.followRedirects = false;
        request!.headers.contentType = ContentType.json;
        if (body != null) request!.write(jsonEncode(body));
        dispatched = true;
        final response = await request!.close();
        final bytes = BytesBuilder(copy: false);
        await for (final part in response) {
          if (scope?.cancelled == true) {
            throw DshException('cancelled', '读取已取消');
          }
          if (bytes.length + part.length > maxBytes) {
            throw DshException(
              'response-limit',
              '响应超过读取上限 ${(maxBytes / 1048576).round()} MiB',
            );
          }
          bytes.add(part);
        }
        if (response.statusCode != 200) {
          final errorBody = utf8.decode(
            bytes.takeBytes(),
            allowMalformed: true,
          );
          String message = '服务请求失败';
          try {
            final decoded = object(jsonDecode(errorBody));
            message = '${decoded['message'] ?? decoded['error'] ?? message}';
          } catch (_) {}
          throw DshException(
            'http-${response.statusCode}',
            message,
            outcomeUnknown: mutation,
          );
        }
        return bytes.takeBytes();
      })().timeout(
        timeout,
        onTimeout: () {
          request?.abort();
          throw TimeoutException('Host request timed out');
        },
      );
    } on DshException {
      rethrow;
    } catch (error) {
      request?.abort();
      if (scope?.cancelled == true || _closed) {
        throw DshException('cancelled', '读取已取消');
      }
      throw DshException(
        'transport',
        '无法完成服务请求：$error',
        outcomeUnknown: mutation && dispatched,
      );
    } finally {
      unregister?.call();
      http.close(force: true);
      if (_requests.remove(http)) _globalActiveRequests--;
    }
  }

  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    final data = await bytes(
      path,
      body: body,
      scope: scope,
      mutation: mutation,
      maxBytes: maxBytes,
    );
    try {
      final decoded = jsonDecode(utf8.decode(data));
      if (decoded is! Map) {
        throw const FormatException('Invalid response object');
      }
      return object(decoded);
    } catch (e) {
      throw DshException('protocol', '响应无法解析：$e', outcomeUnknown: mutation);
    }
  }

  Future<Json> _post(
    String path,
    Json body, {
    bool mutation = false,
    RequestScope? scope,
  }) => request(path, body: body, mutation: mutation, scope: scope);

  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    return object(await _call(method, payload, mutation, scope));
  }

  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async {
    return object(await _call(method, payload, mutation, null));
  }

  Future<dynamic> callValue(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) => _call(method, payload, mutation, scope);
  Future<dynamic> _call(
    String method,
    Json payload,
    bool mutation,
    RequestScope? scope,
  ) async {
    final rpcId = newRequestId();
    final response = await _post(
      '/api/$method',
      {
        'type': 'client-request',
        'rpcId': rpcId,
        'method': method,
        'payload': payload,
      },
      mutation: mutation,
      scope: scope,
    );
    if (response['type'] != 'server-response' || response['rpcId'] != rpcId) {
      throw DshException('protocol', '服务响应与请求不匹配', outcomeUnknown: mutation);
    }
    final result = object(response['result']);
    if (result['ok'] == false) {
      final error = object(result['error']);
      throw DshException(
        error['code'] as String? ?? 'unknown',
        error['message'] as String? ?? '服务拒绝了请求',
      );
    }
    if (result['ok'] != true) {
      throw DshException('protocol', '无效的服务响应', outcomeUnknown: mutation);
    }
    return result['value'];
  }

  Future<HostInfo> describe() async =>
      HostInfo.fromJson(await call('host.describe'));
  Future<List<Json>> availableCommands(String sessionId) async => objects(
    await callValue(
      'commands.list',
      payload: {
        'args': {'agentId': sessionId},
      },
    ),
  );
  Future<List<SessionSummary>> sessions() async => objects(
    (await call('session.list'))['items'],
  ).map(SessionSummary.fromJson).toList();
  Future<String> createSession(String cwd) async =>
      (await call('session.create', {'cwd': cwd}, true))['sessionId'] as String;
  Future<HistoryPage> history(
    String id, {
    int? before,
    int? after,
    RequestScope? scope,
  }) async => HistoryPage.fromJson(
    await rpc(
      'session.history',
      payload: {
        'sessionId': id,
        'maxMessages': 80,
        'beforeSeq': ?before,
        'afterSeq': ?after,
      },
      scope: scope,
    ),
  );
  Future<ModelCatalog> models(String id) async =>
      ModelCatalog.fromJson(await call('session.models', {'sessionId': id}));
  Future<void> selectModel(String id, ModelChoice model) => call(
    'session.selectModel',
    {'sessionId': id, 'provider': model.provider, 'model': model.id},
    true,
  );
  Future<Json> prompt(String id, String text, {required String requestId}) =>
      call('session.prompt', {
        'sessionId': id,
        'mode': 'queue',
        'content': [
          {'type': 'text', 'text': text},
        ],
        'requestId': requestId,
      }, true);
  Future<void> cancel(String id) =>
      call('session.cancel', {'sessionId': id}, true);

  Future<bool> respond(HostFrame frame, Json value) async {
    return _respondResult(frame, {'ok': true, 'value': value});
  }

  Future<bool> cancelQuestion(HostFrame frame) {
    if (frame.type != 'question/requested')
      throw ArgumentError('Expected a question request');
    return _respondResult(frame, {
      'ok': false,
      'error': {
        'code': 'cancelled',
        'message': 'the user closed this question request',
        'details': <String, dynamic>{},
      },
    });
  }

  Future<bool> _respondResult(HostFrame frame, Json result) async {
    final receipt = await _post('/api/respond', {
      'type': 'client-response',
      'rpcId': frame.rpcId,
      'result': result,
    }, mutation: true);
    if (receipt['accepted'] == true) return true;
    if (receipt['accepted'] == false && receipt['reason'] == 'not-pending') {
      return false;
    }
    throw DshException('bad-response', '服务未接受此回答');
  }

  EventChannel events(String name) {
    if (name != 'mux' && name != 'host') throw ArgumentError.value(name);
    final channel = EventChannel(
      baseUri
          .resolve('/api/events.$name')
          .replace(scheme: baseUri.scheme == 'https' ? 'wss' : 'ws'),
    );
    _channels.add(channel);
    return channel;
  }

  Future<void> close() async {
    _closed = true;
    for (final http in _requests.toList()) {
      http.close(force: true);
    }
    await Future.wait(_channels.map((channel) => channel.close()));
    _channels.clear();
  }
}

class EventChannel {
  static int _liveChannels = 0, _liveSockets = 0;
  EventChannel(this.uri);
  final Uri uri;
  final _frames = StreamController<HostFrame>.broadcast();
  final _states = StreamController<bool>.broadcast();
  Stream<HostFrame> get frames => _frames.stream;
  Stream<bool> get states => _states.stream;
  WebSocket? _socket;
  bool _closed = false, _started = false;
  final _stop = Completer<void>();
  Future<void>? _task;

  void start() {
    if (_started || _closed) return;
    _started = true;
    _liveChannels++;
    _task = _run();
  }

  Future<void> _run() async {
    var attempt = 0;
    while (!_closed) {
      var countedSocket = false;
      try {
        final pending = WebSocket.connect(uri.toString());
        // A connection that completes after close/timeout must not leak a socket.
        var expired = false;
        pending.then((socket) {
          if (_closed || expired) unawaited(socket.close());
        }, onError: (Object _) {});
        try {
          _socket = await pending.timeout(const Duration(seconds: 8));
        } on TimeoutException {
          expired = true;
          rethrow;
        }
        if (_closed) {
          await _socket!.close();
          break;
        }
        _socket!.pingInterval = const Duration(seconds: 20);
        _liveSockets++;
        countedSocket = true;
        attempt = 0;
        _states.add(true);
        await for (final message in _socket!) {
          if (_closed) break;
          if (message is! String) {
            throw const FormatException('Expected JSON event');
          }
          _frames.add(HostFrame.fromJson(object(jsonDecode(message))));
        }
      } catch (error, stack) {
        if (!_closed) _frames.addError(error, stack);
      } finally {
        if (countedSocket) _liveSockets--;
        final socket = _socket;
        _socket = null;
        if (socket != null) unawaited(socket.close());
      }
      if (_closed) break;
      _states.add(false);
      final delay = Duration(
        milliseconds: min(10000, 500 * (1 << min(attempt++, 5))),
      );
      await Future.any([Future<void>.delayed(delay), _stop.future]);
    }
  }

  Future<void> close() async {
    if (_closed) return;
    _closed = true;
    _stop.complete();
    unawaited(_socket?.close());
    await _task;
    await _frames.close();
    await _states.close();
    if (_started) _liveChannels--;
  }
}
