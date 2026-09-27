import 'client.dart';
import 'models.dart';
import 'resources.dart';

Never _invalidLogout(String field) =>
    throw DshException('protocol', '账号退出影响信息无效：$field；请重新查询影响并确认。');

String _requiredText(Json value, String key) {
  final text = value[key];
  if (text is! String || text.trim().isEmpty) _invalidLogout(key);
  return text;
}

int _taskCount(Json value, String key) {
  final count = value[key];
  if (count is! int || count < 0) _invalidLogout(key);
  return count;
}

/// The exact account and login instance presented for sign-out confirmation.
class ProviderLogoutImpact {
  ProviderLogoutImpact._({
    required this.provider,
    required this.accountScope,
    required this.loginGeneration,
    required this.taskCount,
  });

  factory ProviderLogoutImpact.fromJson(Json value) {
    if (value['reason'] != 'account-signed-out') _invalidLogout('reason');
    return ProviderLogoutImpact._(
      provider: _requiredText(value, 'provider'),
      accountScope: _requiredText(value, 'accountScope'),
      loginGeneration: _requiredText(value, 'loginGeneration'),
      taskCount: _taskCount(value, 'taskCount'),
    );
  }

  final String provider, accountScope, loginGeneration;
  final int taskCount;
  String get reason => 'account-signed-out';

  Json toLogoutBody() => {
    'provider': provider,
    'accountScope': accountScope,
    'loginGeneration': loginGeneration,
  };
}

class ProviderLogoutResult {
  ProviderLogoutResult._({
    required this.removedAccountScope,
    required this.loginGeneration,
    required this.cancelledTaskCount,
    this.warning,
  });

  factory ProviderLogoutResult.fromJson(
    Json value,
    ProviderLogoutImpact confirmed,
  ) {
    try {
      if (value['status'] != 'signedOut' ||
          (value.containsKey('provider') &&
              value['provider'] != confirmed.provider) ||
          value['reason'] != 'account-signed-out' ||
          value['removedAccountScope'] != confirmed.accountScope ||
          value['loginGeneration'] != confirmed.loginGeneration) {
        _invalidLogout('account');
      }
      if (value['warning'] != null && value['warning'] is! String) {
        _invalidLogout('warning');
      }
      return ProviderLogoutResult._(
        removedAccountScope: confirmed.accountScope,
        loginGeneration: confirmed.loginGeneration,
        cancelledTaskCount: _taskCount(value, 'cancelledTaskCount'),
        warning: value['warning'] as String?,
      );
    } on DshException catch (error) {
      throw DshException(error.code, error.message, outcomeUnknown: true);
    }
  }

  final String removedAccountScope, loginGeneration;
  final int cancelledTaskCount;
  final String? warning;
  String get reason => 'account-signed-out';
}

class ProviderAuthApi {
  ProviderAuthApi(this.client);
  final DshClient client;

  Future<ProviderLogoutImpact> logoutImpact(
    String provider, {
    String? accountScope,
    RequestScope? scope,
  }) async {
    if (provider.trim().isEmpty ||
        (accountScope != null && accountScope.trim().isEmpty)) {
      throw ArgumentError('A valid provider and account scope are required');
    }
    final value = ProviderLogoutImpact.fromJson(
      await client.request(
        '/provider-auth/logout-impact',
        body: {
          'provider': provider,
          if (accountScope != null) 'accountScope': accountScope,
        },
        scope: scope,
      ),
    );
    if (value.provider != provider ||
        (accountScope != null && value.accountScope != accountScope)) {
      _invalidLogout('account');
    }
    return value;
  }

  Future<ProviderLogoutResult> logout(
    ProviderLogoutImpact confirmed, {
    RequestScope? scope,
  }) async => ProviderLogoutResult.fromJson(
    await client.request(
      '/provider-auth/logout',
      body: confirmed.toLogoutBody(),
      mutation: true,
      scope: scope,
    ),
    confirmed,
  );
}
