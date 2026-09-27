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
