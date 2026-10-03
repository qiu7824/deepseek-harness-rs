import 'package:dsh_client/dsh_client.dart';

/// One operation belongs to a screen generation, even across nested awaits.
class PageOperation {
  PageOperation(this.scope, this.isCurrent);
  final RequestScope scope;
  final bool Function() isCurrent;

  bool get valid => !scope.cancelled && isCurrent();

  Future<T> request<T>(Future<T> Function() action) async {
    if (!valid) throw DshException('cancelled', '页面已关闭或连接已更改');
    final value = await action();
    if (!valid) throw DshException('cancelled', '页面已关闭或连接已更改');
    return value;
  }
}

/// Whether a failure means the connected Host predates the feature. Older
/// Hosts answer unknown routes with 404 or 405 rather than a structured error.
bool isUnsupportedHost(Object error) =>
    error is DshException &&
    const {'unsupported', 'http-404', 'http-405'}.contains(error.code);

/// Every Host that serves a page answers its catalog; a missing catalog means
/// the client is attached to an older Host, not that a record is missing.
Object unsupportedHostPage(DshException error, String operation, String page) =>
    isUnsupportedHost(error) && operation == 'catalog'
    ? DshException('unsupported', '当前连接的本机服务版本过旧，不支持$page。请结束旧的服务进程后重新打开桌面版。')
    : error;
