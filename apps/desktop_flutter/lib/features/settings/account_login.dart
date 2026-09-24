import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/foundation.dart';

bool retryableLoginError(Object error) {
  if (error is! DshException) return false;
  if (error.code == 'transport' || error.code == 'timeout') return true;
  if (error.code == 'http-408' || error.code == 'http-429') return true;
  final status = int.tryParse(error.code.replaceFirst('http-', ''));
  return error.code.startsWith('http-') && status != null && status >= 500;
}

class AccountLogin extends ChangeNotifier {
  AccountLogin(this.api, this.provider);
  final DshClient api;
  final String provider;
  final scope = RequestScope();
  Json? attempt;
  String? error, notice;
  bool starting = false,
      polling = false,
      complete = false,
      expired = false,
      _disposed = false;
  Timer? _timer;
  int interval = 3;
  final _cancelled = <String>{};
  bool get cli => attempt?['mode'] == 'cli';
  bool get attemptExpired {
    final expiry = (attempt?['expiresAt'] as num?)?.toInt();
    if (expiry == null) return false;
    final millis = expiry < 100000000000 ? expiry * 1000 : expiry;
    return DateTime.now().millisecondsSinceEpoch >= millis;
  }

  Uri? get authorizationUri {
    final uri = Uri.tryParse('${attempt?['verificationUri'] ?? ''}');
    return uri != null &&
            uri.scheme == 'https' &&
            uri.host.isNotEmpty &&
            uri.userInfo.isEmpty
        ? uri
        : null;
  }

  void emit() {
    if (!_disposed) notifyListeners();
  }

  Future<void> cancelId(Object? value) async {
    if (value is! String || !_cancelled.add(value)) return;
    try {
      await api.request(
        '/provider-auth/cancel',
        body: {'attempt': value},
        mutation: true,
      );
    } catch (_) {}
  }

  Future<void> start() async {
    if (starting || polling || _disposed) return;
    starting = true;
    _timer?.cancel();
    error = null;
    notice = null;
    expired = false;
    complete = false;
    emit();
    await cancelId(attempt?['attempt']);
    attempt = null;
    try {
      final value = await api.request(
        '/provider-auth/start',
        body: {'provider': provider},
        mutation: true,
        scope: scope,
      );
      if (_disposed) {
        await cancelId(value['attempt']);
        return;
      }
      if (value['attempt'] is! String) throw StateError('服务未返回有效的登录请求');
      attempt = value;
      interval = ((value['interval'] as num?)?.toInt() ?? 3).clamp(3, 60);
      if (!cli && authorizationUri == null) {
        throw StateError('服务未返回有效的 HTTPS 授权地址');
      }
      schedule();
    } catch (e) {
      unawaited(cancelId(attempt?['attempt']));
      attempt = null;
      if (!_disposed) error = '$e';
    } finally {
      starting = false;
      if (_disposed) {
        scope.cancel();
      } else {
        emit();
      }
    }
  }

  void schedule() {
    _timer?.cancel();
    if (!_disposed && attempt != null && !complete && !expired) {
      _timer = Timer(Duration(seconds: interval), poll);
    }
  }

  Future<void> poll() async {
    if (polling || attempt == null || _disposed || complete || expired) return;
    if (attemptExpired) {
      expired = true;
      error = '授权已过期，请重新登录。';
      _timer?.cancel();
      unawaited(cancelId(attempt!['attempt']));
      attempt = null;
      emit();
      return;
    }
    polling = true;
    _timer?.cancel();
    emit();
    try {
      final value = await api.request(
        '/provider-auth/poll',
        body: {'attempt': attempt!['attempt']},
        scope: scope,
      );
      if (_disposed) return;
      switch (value['status']) {
        case 'complete':
          complete = true;
          attempt = null;
          error = null;
          notice = null;
        case 'pending':
          interval = ((value['interval'] as num?)?.toInt() ?? interval).clamp(
            3,
            60,
          );
          notice = value['retryable'] == true
              ? '连接暂时中断，正在重试当前登录请求。'
              : value['message'] as String?;
          error = null;
          schedule();
        default:
          expired = true;
          error = '${value['message'] ?? '登录已取消或结束，请重新登录。'}';
          final old = attempt?['attempt'];
          attempt = null;
          unawaited(cancelId(old));
      }
    } catch (e) {
      if (!_disposed && retryableLoginError(e) && !attemptExpired) {
        error = null;
        notice = '连接暂时中断，正在重试当前登录请求。';
        interval = interval < 5 ? 5 : interval;
        schedule();
      } else if (!_disposed) {
        expired = true;
        error = attemptExpired ? '授权已过期，请重新登录。' : '$e';
        notice = null;
        final old = attempt?['attempt'];
        attempt = null;
        unawaited(cancelId(old));
      }
    } finally {
      polling = false;
      emit();
    }
  }

  @override
  void dispose() {
    _disposed = true;
    _timer?.cancel();
    unawaited(cancelId(attempt?['attempt']));
    if (!starting) scope.cancel();
    super.dispose();
  }
}
