import 'dart:async';
import 'dart:io';
import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  test('weighted LRU bounds bytes and entry count and disposes evictions', () {
    final evicted = <String>[];
    final cache = ResourceCache<String, String>(
      maxBytes: 12,
      maxEntries: 2,
      sizeOf: (v) => v.length * 2,
      onEvict: evicted.add,
    );
    expect(cache.put('a', 'aaa'), isTrue);
    expect(cache.put('b', 'bb'), isTrue);
    expect(cache.get('a'), 'aaa');
    cache.put('c', 'ccc');
    expect(cache.get('b'), isNull);
    expect(evicted, ['bb']);
    expect(cache.bytes, 12);
    expect(cache.put('huge', 'x' * 100), isFalse);
    expect(cache.length, 2);
    for (var i = 0; i < 10000; i++) {
      cache.put('$i', 'abc');
      expect(cache.bytes, lessThanOrEqualTo(12));
      expect(cache.length, lessThanOrEqualTo(2));
    }
    cache.clear();
    expect(cache.bytes, 0);
    expect(cache.length, 0);
  });
  test('scope cancels every registered owner exactly once', () {
    final scope = RequestScope();
    var closed = 0;
    scope.register(() => closed++);
    final remove = scope.register(() => closed += 10);
    remove();
    scope.cancel();
    scope.cancel();
    scope.register(() => closed += 100);
    expect(closed, 101);
  });
  test(
    'cancel stops a response stream and releases request accounting',
    () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final started = Completer<void>();
      Timer? timer;
      server.listen((request) async {
        request.response.headers.contentType = ContentType.binary;
        request.response.add(List.filled(4096, 65));
        await request.response.flush();
        started.complete();
        timer = Timer.periodic(const Duration(milliseconds: 20), (_) {
          try {
            request.response.add(List.filled(4096, 66));
          } catch (_) {
            timer?.cancel();
          }
        });
      });
      final client = DshClient('http://127.0.0.1:${server.port}');
      final scope = RequestScope();
      final response = client.bytes('/large', scope: scope);
      final expected = expectLater(
        response,
        throwsA(isA<DshException>().having((e) => e.code, 'code', 'cancelled')),
      );
      await started.future;
      expect(client.activeRequests, 1);
      scope.cancel();
      await expected.timeout(const Duration(seconds: 2));
      expect(client.activeRequests, 0);
      timer?.cancel();
      await client.close();
      await server.close(force: true);
    },
  );
  test(
    'oversized binary response is rejected before full accumulation',
    () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      server.listen((request) async {
        request.response.add(List.filled(8192, 1));
        await request.response.close();
      });
      final client = DshClient('http://127.0.0.1:${server.port}');
      await expectLater(
        client.bytes('/file', maxBytes: 4096),
        throwsA(
          isA<DshException>().having((e) => e.code, 'code', 'response-limit'),
        ),
      );
      expect(client.activeRequests, 0);
      await client.close();
      await server.close(force: true);
    },
  );
}
