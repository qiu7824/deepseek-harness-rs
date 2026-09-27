import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

Json impactJson() => {
  'provider': 'openai-codex',
  'accountScope': 'account-a',
  'loginGeneration': 'generation-a',
  'taskCount': 2,
  'reason': 'account-signed-out',
};

class FakeAuthClient extends DshClient {
  FakeAuthClient() : super('http://127.0.0.1');
  final calls = <({String path, Json body, bool mutation})>[];
  Json impact = impactJson();
  DshException? failure;
  Json? result;

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    calls.add((path: path, body: {...?body}, mutation: mutation));
    if (failure != null) throw failure!;
    if (path == '/provider-auth/logout-impact') return impact;
    return result ??
        {
          'status': 'signedOut',
          'provider': body!['provider'],
          'removedAccountScope': body['accountScope'],
          'loginGeneration': body['loginGeneration'],
          'cancelledTaskCount': 2,
          'reason': 'account-signed-out',
        };
  }
}

void main() {
  test(
    'logout submits the immutable confirmed account and generation',
    () async {
      final client = FakeAuthClient(), api = ProviderAuthApi(client);
      final impact = await api.logoutImpact(
        'openai-codex',
        accountScope: 'account-a',
      );
      client.impact['accountScope'] = 'account-b';
      client.impact['loginGeneration'] = 'generation-b';
      impact.toLogoutBody()['accountScope'] = 'account-c';
      final result = await api.logout(impact);
      expect(client.calls.first.mutation, isFalse);
      expect(client.calls.last.mutation, isTrue);
      expect(client.calls.last.body, {
        'provider': 'openai-codex',
        'accountScope': 'account-a',
        'loginGeneration': 'generation-a',
      });
      expect(result.removedAccountScope, 'account-a');
      expect(result.cancelledTaskCount, 2);
      expect(result.reason, 'account-signed-out');
    },
  );

  test('invalid impact never becomes a zero-task confirmation', () async {
    for (final change in <Json>[
      {'taskCount': null},
      {'taskCount': -1},
      {'taskCount': 1.5},
      {'taskCount': '0'},
      {'accountScope': ''},
      {'accountScope': 'account-b'},
      {'provider': 'other'},
      {'loginGeneration': ''},
      {'loginGeneration': null},
      {'reason': 'unknown'},
    ]) {
      final client = FakeAuthClient()..impact = {...impactJson(), ...change};
      await expectLater(
        ProviderAuthApi(client)
            .logoutImpact('openai-codex', accountScope: 'account-a'),
        throwsA(isA<DshException>()),
      );
      expect(client.calls.map((call) => call.path), [
        '/provider-auth/logout-impact',
      ]);
    }
  });

  test('zero tasks is valid only with a complete identity', () async {
    final client = FakeAuthClient()..impact['taskCount'] = 0;
    final impact = await ProviderAuthApi(client).logoutImpact('openai-codex');
    expect(impact.taskCount, 0);
    expect(impact.accountScope, 'account-a');
  });

  test(
    'impact failures and logout conflicts never fall back or retry',
    () async {
      final client = FakeAuthClient(), api = ProviderAuthApi(client);
      client.failure = DshException('http-404', 'not supported');
      await expectLater(
        api.logoutImpact('openai-codex'),
        throwsA(isA<DshException>()),
      );
      expect(client.calls.length, 1);
      client.failure = null;
      final impact = await api.logoutImpact('openai-codex');
      client.failure = DshException('http-409', 'account changed');
      await expectLater(api.logout(impact), throwsA(isA<DshException>()));
      expect(client.calls.length, 3);
      expect(client.calls.last.body, impact.toLogoutBody());
    },
  );

  test(
    'success warning is preserved and mismatched success is unknown',
    () async {
      final client = FakeAuthClient(), api = ProviderAuthApi(client);
      final impact = await api.logoutImpact('openai-codex');
      client.result = {
        'status': 'signedOut',
        'provider': 'openai-codex',
        'removedAccountScope': 'account-a',
        'loginGeneration': 'generation-a',
        'cancelledTaskCount': 1,
        'reason': 'account-signed-out',
        'warning': 'Credential cleanup incomplete',
      };
      expect(
        (await api.logout(impact)).warning,
        'Credential cleanup incomplete',
      );
      client.result!['removedAccountScope'] = 'account-b';
      await expectLater(
        api.logout(impact),
        throwsA(
          isA<DshException>().having(
            (error) => error.outcomeUnknown,
            'outcomeUnknown',
            isTrue,
          ),
        ),
      );
      client.result!['removedAccountScope'] = 'account-a';
      client.result!['provider'] = 'other-provider';
      await expectLater(api.logout(impact), throwsA(isA<DshException>()));
    },
  );
}
