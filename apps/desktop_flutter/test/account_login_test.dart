import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/account_login.dart';
import 'package:flutter_test/flutter_test.dart';

class AuthClient extends DshClient {
  AuthClient() : super('http://127.0.0.1');
  final calls = <String>[];
  Completer<Json>? delayed;
  Json started = {
    'attempt': 'auth-1',
    'verificationUri': 'https://example.com/authorize',
    'userCode': 'TEST',
    'interval': 2,
  };
  Json polled = {'status': 'pending', 'interval': 10};
  Object? pollFailure;
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    calls.add(path);
    if (path.endsWith('/start')) {
      return delayed != null ? delayed!.future : started;
    }
    if (path.endsWith('/poll')) {
      if (pollFailure != null) throw pollFailure!;
      return polled;
    }
    return {'status': 'cancelled'};
  }
}

void main() {
  testWidgets(
    'device login honors server backoff and completion stops polling',
    (tester) async {
      final api = AuthClient();
      final auth = AccountLogin(api, 'openai-codex');
      await auth.start();
      expect(auth.authorizationUri!.scheme, 'https');
      await tester.pump(const Duration(seconds: 3));
      expect(auth.interval, 10);
      final count = api.calls.length;
      await tester.pump(const Duration(seconds: 9));
      expect(api.calls.length, count);
      api.polled = {'status': 'complete'};
      await tester.pump(const Duration(seconds: 1));
      expect(auth.complete, true);
      await tester.pump(const Duration(seconds: 30));
      expect(api.calls.where((p) => p.endsWith('/poll')).length, 2);
      auth.dispose();
      expect(api.calls.where((p) => p.endsWith('/cancel')), isEmpty);
      await api.close();
    },
  );
  testWidgets(
    'closing during start cancels the late server attempt exactly once',
    (tester) async {
      final api = AuthClient()..delayed = Completer<Json>();
      final auth = AccountLogin(api, 'devin');
      final pending = auth.start();
      await tester.pump();
      auth.dispose();
      api.delayed!.complete(api.started);
      await pending;
      await tester.pump();
      expect(api.calls.where((p) => p.endsWith('/cancel')).length, 1);
      expect(auth.scope.cancelled, true);
      await api.close();
    },
  );
  testWidgets(
    'expired login is cancelled and CLI login needs no fabricated URL',
    (tester) async {
      final api = AuthClient()
        ..started = {
          'attempt': 'cli-1',
          'mode': 'cli',
          'interval': 2,
          'expiresAt': 1,
        };
      final auth = AccountLogin(api, 'claude-code');
      await auth.start();
      expect(auth.cli, true);
      expect(auth.authorizationUri, isNull);
      expect(auth.error, isNull);
      await tester.pump(const Duration(seconds: 3));
      expect(auth.expired, true);
      expect(api.calls.where((p) => p.endsWith('/poll')), isEmpty);
      auth.dispose();
      expect(api.calls.where((p) => p.endsWith('/cancel')).length, 1);
      await api.close();
    },
  );
  testWidgets('transient poll errors back off and resume the same login', (
    tester,
  ) async {
    final api = AuthClient()
      ..pollFailure = DshException('transport', 'temporary disconnect');
    final auth = AccountLogin(api, 'openai-codex');
    await auth.start();
    await tester.pump(const Duration(seconds: 3));
    expect(auth.expired, false);
    expect(auth.attempt?['attempt'], 'auth-1');
    expect(auth.notice, '连接暂时中断，正在重试当前登录请求。');
    expect(api.calls.where((p) => p.endsWith('/poll')).length, 1);
    api.pollFailure = null;
    api.polled = {'status': 'complete'};
    await tester.pump(const Duration(seconds: 4));
    expect(api.calls.where((p) => p.endsWith('/poll')).length, 1);
    await tester.pump(const Duration(seconds: 1));
    expect(auth.complete, true);
    expect(auth.attempt, isNull);
    expect(api.calls.where((p) => p.endsWith('/cancel')), isEmpty);
    auth.dispose();
    await api.close();
  });
  testWidgets('terminal poll errors cancel the server attempt once', (
    tester,
  ) async {
    final api = AuthClient()
      ..pollFailure = DshException('http-401', 'unauthorized');
    final auth = AccountLogin(api, 'openai-codex');
    await auth.start();
    await tester.pump(const Duration(seconds: 3));
    expect(auth.expired, true);
    expect(auth.attempt, isNull);
    expect(api.calls.where((p) => p.endsWith('/cancel')).length, 1);
    await tester.pump(const Duration(seconds: 30));
    expect(api.calls.where((p) => p.endsWith('/poll')).length, 1);
    auth.dispose();
    await api.close();
  });
  testWidgets('invalid authorization URLs release their server attempt', (
    tester,
  ) async {
    final api = AuthClient()
      ..started = {
        'attempt': 'auth-1',
        'verificationUri': 'http://invalid.example/authorize',
      };
    final auth = AccountLogin(api, 'openai-codex');
    await auth.start();
    expect(auth.error, contains('HTTPS'));
    expect(auth.attempt, isNull);
    expect(api.calls.where((p) => p.endsWith('/cancel')).length, 1);
    auth.dispose();
    await api.close();
  });
}
