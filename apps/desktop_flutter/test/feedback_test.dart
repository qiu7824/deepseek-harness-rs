import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/conversation/feedback_controller.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late HttpServer server;
  late DshClient api;
  late List<Json> calls;
  late FutureOr<Json> Function(Json call) handle;
  setUp(() async {
    calls = [];
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    api = DshClient('http://127.0.0.1:${server.port}');
    handle = (_) => {
      'ok': true,
      'value': {'items': []},
    };
    server.listen((request) async {
      final call = object(jsonDecode(await utf8.decoder.bind(request).join()));
      calls.add(call);
      final value = await handle(call);
      request.response.headers.contentType = ContentType.json;
      request.response.write(
        jsonEncode({
          'type': 'server-response',
          'rpcId': call['rpcId'],
          'result': {'ok': true, 'value': value},
        }),
      );
      await request.response.close();
    });
  });
  tearDown(() async {
    await api.close();
    await server.close(force: true);
  });
  Json row(String version, [String rating = 'positive']) => {
    'messageId': 'm',
    'rating': rating,
    'version': version,
  };

  test('carrier success cannot hide business rejection; conflict reconciles and delete uses CAS', () async {
    final c = MessageFeedbackController(api, 's')..retainTargets(['m']);
    addTearDown(c.dispose);
    await Future.wait(List.generate(20, (_) => c.ensure()));
    expect(calls, hasLength(1));
    handle = (_) => {
      'ok': false,
      'error': {'code': 'target-not-found'},
    };
    expect(await c.save('m', 'positive', ifVersion: null), isFalse);
    expect(c.item('m'), isNull);
    expect(c.error, contains('target-not-found'));
    expect(object(calls.last['payload']).containsKey('ifVersion'), isTrue);
    expect(object(calls.last['payload'])['ifVersion'], isNull);
    handle = (_) => {'ok': true, 'value': row('v1')};
    expect(await c.save('m', 'positive', ifVersion: null), isTrue);
    expect(c.item('m')?['version'], 'v1');
    handle = (_) => {
      'ok': false,
      'error': {'code': 'version-conflict', 'current': row('v2', 'negative')},
    };
    expect(await c.save('m', null, ifVersion: 'v1'), isFalse);
    expect(c.item('m')?['rating'], 'negative');
    expect(c.error, contains('version-conflict'));
    handle = (_) => {
      'ok': true,
      'value': {'absent': true},
    };
    expect(await c.save('m', null, ifVersion: 'v2'), isTrue);
    expect(object(calls.last['payload'])['ifVersion'], 'v2');
    expect(c.item('m'), isNull);
  });

  test('cold click waits for shared list, failed reads block writes, disposal ignores late reads', () async {
    final c = MessageFeedbackController(api, 's')..retainTargets(['m']);
    handle = (_) => {
      'ok': false,
      'error': {'code': 'session-not-found'},
    };
    expect(await c.save('m', 'positive', ifVersion: null), isFalse);
    expect(calls.map((v) => v['method']), ['messageFeedback.list']);
    final pending = Completer<Json>();
    handle = (_) => pending.future;
    final future = c.ensure(refresh: true);
    await Future<void>.delayed(const Duration(milliseconds: 20));
    c.dispose();
    pending.complete({
      'ok': true,
      'value': {
        'items': [row('late')],
      },
    });
    expect(await future, isFalse);
    expect(c.retainedCount, 0);
  });
}
