abstract final class DshAccountMenuZh {
  static const subscription = '订阅账号';
  static const signIn = '登录订阅账号';
  static const signInHint = '登录订阅账号，或配置 API 连接';
  static const manageAccounts = '添加或管理订阅账号';
  static const apiModels = 'API 连接与模型';
  static const connected = '已连接';
  static const expired = '需重新登录';
  static const relogin = '重新登录';
  static String providerDetails(String name) => '查看 $name 的账号';
  static String reloginProvider(String name) => '重新登录 $name';

  /// One short line for the trigger tooltip; account names stay in the panel.
  static String summary(int linked, int attention) => [
    if (linked - attention > 0) '${linked - attention} 个已连接',
    if (attention > 0) '$attention 个需重新登录',
  ].join(' · ');
}
