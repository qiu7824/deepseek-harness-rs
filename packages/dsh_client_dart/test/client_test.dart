import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  late HttpServer server;
  late DshClient client;
  setUp(() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    client = DshClient(
      'http://127.0.0.1:${server.port}',
      timeout: const Duration(milliseconds: 500),
    );
  });
  tearDown(() async {
    await client.close();
    await server.close(force: true);
    expect(DshClient.resourceCounts.values, everyElement(0));
  });

  test('validates local endpoints before any network request', () {
    for (final uri in [
      'https://example.com',
      'file:///a',
      'http://127.0.0.1@evil.test',
      'http://127.0.0.1/api',
      'http://127.0.0.1?x=y',
    ]) {
      expect(() => localHostUri(uri), throwsFormatException);
    }
    expect(localHostUri('http://localhost:58080').port, 58080);
  });
  test('uses Host envelope and rejects correlation mismatch', () async {
    server.listen((request) async {
      final body = object(jsonDecode(await utf8.decoder.bind(request).join()));
      expect(request.uri.path, '/api/host.describe');
      expect(body['type'], 'client-request');
      request.response.write(
        jsonEncode({
          'type': 'server-response',
          'rpcId': 'wrong',
          'result': {'ok': true, 'value': {}},
        }),
      );
      await request.response.close();
    });
    await expectLater(
      client.describe(),
      throwsA(isA<DshException>().having((e) => e.code, 'code', 'protocol')),
    );
  });
  test('preserves explicit server errors', () async {
    server.listen((request) async {
      final body = object(jsonDecode(await utf8.decoder.bind(request).join()));
      request.response.write(
        jsonEncode({
          'type': 'server-response',
          'rpcId': body['rpcId'],
          'result': {
            'ok': false,
            'error': {'code': 'agent-busy', 'message': 'busy'},
          },
        }),
      );
      await request.response.close();
    });
    await expectLater(
      client.cancel('s'),
      throwsA(
        isA<DshException>()
            .having((e) => e.outcomeUnknown, 'known result', false)
            .having((e) => e.code, 'code', 'agent-busy'),
      ),
    );
  });
  test('interrupted mutation is outcome unknown and is not retried', () async {
    var requests = 0;
    server.listen((request) async {
      requests++;
      await utf8.decoder.bind(request).join();
      request.response.write('{');
      await request.response.close();
    });
    await expectLater(
      client.prompt('s', 'hello', requestId: 'stable-prompt-id'),
      throwsA(
        isA<DshException>().having((e) => e.outcomeUnknown, 'unknown', true),
      ),
    );
    expect(requests, 1);
  });
  test('approval response echoes server rpcId and validates receipt', () async {
    server.listen((request) async {
      final body = object(jsonDecode(await utf8.decoder.bind(request).join()));
      expect(request.uri.path, '/api/respond');
      expect(body, {
        'type': 'client-response',
        'rpcId': 'approval-rpc',
        'result': {
          'ok': true,
          'value': {
            'sessionId': 's',
            'approvalId': 'a',
            'outcome': 'allowed-once',
          },
        },
      });
      request.response.write('{"accepted":true}');
      await request.response.close();
    });
    final frame = HostFrame.fromJson({
      'type': 'server-request',
      'rpcId': 'approval-rpc',
      'payload': {'type': 'approval/requested', 'sessionId': 's'},
    });
    expect(
      await client.respond(frame, {
        'sessionId': 's',
        'approvalId': 'a',
        'outcome': 'allowed-once',
      }),
      isTrue,
    );
  });
  test('question cancellation uses the carrier error result', () async {
    server.listen((request) async {
      final body = object(jsonDecode(await utf8.decoder.bind(request).join()));
      expect(body, {
        'type': 'client-response',
        'rpcId': 'question-rpc',
        'result': {
          'ok': false,
          'error': {
            'code': 'cancelled',
            'message': 'the user closed this question request',
            'details': <String, dynamic>{},
          },
        },
      });
      request.response.write('{"accepted":true}');
      await request.response.close();
    });
    expect(
      await client.cancelQuestion(
        HostFrame.fromJson({
          'type': 'server-request',
          'rpcId': 'question-rpc',
          'payload': {'type': 'question/requested', 'sessionId': 's'},
        }),
      ),
      isTrue,
    );
  });
  test('WebSocket reconnects, replays frames and closes its sockets', () async {
    var connections = 0;
    final sockets = <WebSocket>[];
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      sockets.add(socket);
      connections++;
      socket.add(
        jsonEncode({
          'type': 'server-request',
          'rpcId': 'pending-id',
          'payload': {
            'type': 'question/requested',
            'sessionId': 's',
            'questions': [],
          },
        }),
      );
      if (connections == 1) await socket.close();
    });
    final channel = client.events('mux');
    final frames = <HostFrame>[];
    final replayed = Completer<void>();
    final subscription = channel.frames.listen((f) {
      frames.add(f);
      if (frames.length == 2) replayed.complete();
    });
    channel.start();
    await replayed.future.timeout(const Duration(seconds: 5));
    expect(frames.map((e) => e.rpcId), ['pending-id', 'pending-id']);
    await subscription.cancel();
    await channel.close();
    for (final socket in sockets) {
      await socket.close();
    }
    expect(connections, 2);
  });
}
