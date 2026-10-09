import 'package:dsh_desktop/features/settings/account_usage.dart';
import 'package:flutter_test/flutter_test.dart';

Map<String, dynamic> usage(Map<String, dynamic> window) => {
  'provider': 'devin',
  'accountScope': 'account-a',
  'status': 'fresh',
  'plan': 'Pro',
  'updatedAt': 1791172800,
  'windows': [window],
};

void main() {
  test('official remaining percentage gives the matching used percentage', () {
    final snapshot = AccountUsageSnapshot.fromJson(
      usage({'id': 'daily', 'label': '日额度', 'remainingPercent': 63}),
      expectedProvider: 'devin',
      expectedScope: 'account-a',
    );
    expect(snapshot.windows.single.usedPercent, 37);
    expect(snapshot.windows.single.remainingPercent, 63);
    expect(snapshot.updatedAt, 1791172800);
  });

  test(
    'missing, malformed and contradictory percentages never become zero',
    () {
      for (final window in <Map<String, dynamic>>[
        {},
        {'usedPercent': -1},
        {'usedPercent': 101},
        {'usedPercent': double.nan},
        {'remainingPercent': double.infinity},
        {'usedPercent': '0'},
        {'usedPercent': 20, 'remainingPercent': 90},
        {'usedPercent': 20, 'remainingPercent': '80'},
      ]) {
        final parsed = AccountUsageWindow.fromJson(window);
        expect(parsed.usedPercent, isNull, reason: '$window');
        expect(parsed.remainingPercent, isNull, reason: '$window');
      }
      expect(AccountUsageWindow.fromJson({'usedPercent': 0}).usedPercent, 0);
      expect(
        AccountUsageWindow.fromJson({'usedPercent': 100}).remainingPercent,
        0,
      );
    },
  );

  test('both provider and account identity must match the request', () {
    for (final change in <Map<String, dynamic>>[
      {'provider': 'openai-codex'},
      {'accountScope': 'account-b'},
      {'accountScope': null},
    ]) {
      expect(
        () => AccountUsageSnapshot.fromJson(
          {...usage({}), ...change},
          expectedProvider: 'devin',
          expectedScope: 'account-a',
        ),
        throwsA(isA<AccountUsageIdentityException>()),
      );
    }
  });

  test(
    'invalid time and duration remain unavailable, Unix seconds stay seconds',
    () {
      final window = AccountUsageWindow.fromJson({
        'resetsAt': 1791273600,
        'windowDurationMins': 1440,
      });
      expect(window.resetsAt, 1791273600);
      expect(window.windowDurationMins, 1440);
      for (final value in [
        null,
        -1,
        0,
        1.5,
        '1791273600',
        double.infinity,
        9e15,
      ]) {
        expect(accountUsageUnixSeconds(value), isNull);
      }
      expect(
        AccountUsageWindow.fromJson({'windowDurationMins': -1})
            .windowDurationMins,
        isNull,
      );
    },
  );

  test('unknown status and malformed window collections are rejected', () {
    for (final change in <Map<String, dynamic>>[
      {'status': 'ok'},
      {'windows': {}},
      {
        'windows': [null],
      },
    ]) {
      expect(
        () => AccountUsageSnapshot.fromJson(
          {...usage({}), ...change},
          expectedProvider: 'devin',
          expectedScope: 'account-a',
        ),
        throwsFormatException,
      );
    }
  });

  test(
    'credit amounts retain their unit without deriving a quota percentage',
    () {
      final snapshot = AccountUsageSnapshot.fromJson(
        {
          ...usage({
            'remaining': 23.5,
            'limit': 20,
            'unit': 'credits',
            'periodEndsAt': 1791273600,
          }),
          'message': '额度服务暂不可用',
          'clearSnapshot': true,
        },
        expectedProvider: 'devin',
        expectedScope: 'account-a',
      );
      final window = snapshot.windows.single;
      expect(window.remaining, 23.5);
      expect(window.limit, 20);
      expect(window.unit, 'credits');
      expect(window.usedPercent, isNull);
      expect(window.resetsAt, isNull);
      expect(window.periodEndsAt, 1791273600);
      expect(snapshot.message, '额度服务暂不可用');
      expect(snapshot.clearSnapshot, isTrue);
      expect(AccountUsageWindow.fromJson({'remaining': -1}).remaining, isNull);
    },
  );

  test('oversized service messages are not exposed', () {
    final snapshot = AccountUsageSnapshot.fromJson(
      {...usage({}), 'message': List.filled(201, '字').join()},
      expectedProvider: 'devin',
      expectedScope: 'account-a',
    );
    expect(snapshot.message, isNull);
  });
}
