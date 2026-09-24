import 'dart:async';
import 'dart:io';
import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  late Directory temporary;
  late HttpServer server;
  late DshClient client;
  setUp(() async {
    temporary = await Directory.systemTemp.createTemp('dsh-download-');
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    client = DshClient('http://127.0.0.1:${server.port}');
  });
  tearDown(() async {
    await client.close();
    await server.close(force: true);
    expect(
      Directory(await temporary.resolveSymbolicLinks()).parent.path,
      await Directory.systemTemp.resolveSymbolicLinks(),
    );
    await temporary.delete(recursive: true);
  });
  test(
    'streaming save preserves original bytes and replaces only a complete destination',
    () async {
      final expected = List.generate(1024 * 1024, (i) => i % 256),
          target = File('${temporary.path}/original.bin');
      await target.writeAsString('previous');
      server.listen((r) async {
        r.response.contentLength = expected.length;
        for (var i = 0; i < expected.length; i += 8192) {
          r.response.add(expected.sublist(i, i + 8192));
          await r.response.flush();
        }
        await r.response.close();
      });
      final count = await client.downloadTo('/file', target);
      expect(count, expected.length);
      expect(await target.readAsBytes(), expected);
      expect(client.activeRequests, 0);
      expect(await temporary.list().length, 1);
    },
  );
  test(
    'canceling download preserves existing file and removes staged bytes',
    () async {
      final scope = RequestScope(),
          target = File('${temporary.path}/existing.txt');
      await target.writeAsString('keep');
      server.listen((r) async {
        try {
          for (var i = 0; i < 64; i++) {
            r.response.add(List.filled(8192, 42));
            await r.response.flush();
            await Future<void>.delayed(const Duration(milliseconds: 2));
          }
          await r.response.close();
        } catch (_) {}
      });
      await expectLater(
        client.downloadTo(
          '/file',
          target,
          scope: scope,
          onProgress: (_) => scope.cancel(),
        ),
        throwsA(isA<DshException>().having((e) => e.code, 'code', 'cancelled')),
      );
      expect(await target.readAsString(), 'keep');
      expect(await temporary.list().length, 1);
      expect(client.activeRequests, 0);
    },
  );
  test(
    'size limits, HTTP failures and redirects never create the destination',
    () async {
      server.listen((r) async {
        if (r.uri.path == '/redirect') {
          r.response.statusCode = 302;
          r.response.headers.set('location', 'http://example.com/file');
        } else if (r.uri.path == '/missing') {
          r.response.statusCode = 404;
        } else {
          r.response.contentLength = 200;
          r.response.add(List.filled(200, 1));
        }
        await r.response.close();
      });
      final target = File('${temporary.path}/out');
      for (final path in ['/large', '/missing', '/redirect']) {
        await expectLater(
          client.downloadTo(path, target, maxBytes: 100),
          throwsA(isA<DshException>()),
        );
        expect(await target.exists(), false);
        expect(await temporary.list().length, 0);
        expect(client.activeRequests, 0);
      }
    },
  );
  test(
    'large streamed transfers can extend the default request timeout',
    () async {
      await client.close();
      client = DshClient(
        'http://127.0.0.1:${server.port}',
        timeout: const Duration(milliseconds: 15),
      );
      server.listen((request) async {
        await Future<void>.delayed(const Duration(milliseconds: 80));
        request.response.add([1, 2, 3]);
        await request.response.close();
      });
      final target = File('${temporary.path}/session.zip');
      expect(
        await client.downloadTo(
          '/api/session.export',
          target,
          totalTimeout: const Duration(seconds: 2),
        ),
        3,
      );
      expect(await target.readAsBytes(), [1, 2, 3]);
      expect(client.activeRequests, 0);
    },
  );
}
