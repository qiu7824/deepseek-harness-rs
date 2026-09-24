import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/foundation.dart';

const feedbackCategories = {
  'task-result': '任务结果',
  'instruction-following': '指令遵循',
  'product-interaction': '交互体验',
  'service-stability': '服务稳定性',
  'resource-cost': '资源与费用',
  'security-privacy-permission': '安全、隐私与权限',
  'other': '其他',
};

/// The RPC carrier has already been decoded by DshClient. Feedback methods
/// return another Result; an HTTP success is not a persisted feedback receipt.
Json feedbackValue(Json result) {
  if (result['ok'] == false) {
    final error = object(result['error']);
    final code = '${error['code'] ?? 'feedback-failed'}';
    throw DshException(code, switch (code) {
      'version-conflict' => '这条反馈已在别处改动，已显示最新状态；请核对后再保存。',
      'target-not-found' => '此消息无法评价，请刷新会话记录。',
      'session-not-found' => '会话已不存在或尚未加载。',
      'note-too-large' => '反馈说明过长，请缩短后重试。',
      _ => '${error['message'] ?? '反馈保存失败，请重试。'}',
    });
  }
  if (result['ok'] != true || result['value'] is! Map) {
    throw DshException('protocol', '服务未返回有效的反馈确认。');
  }
  return object(result['value']);
}

class MessageFeedbackController extends ChangeNotifier {
  MessageFeedbackController(this.api, this.sessionId);
  final DshClient api;
  final String sessionId;
  final scope = RequestScope();
  final Map<String, Json> _items = {};
  Set<String> _targets = {};
  Future<bool>? _load;
  bool ready = false, busy = false, _disposed = false;
  String? error;
  Json? item(String id) => _items[id];
  int get retainedCount => _items.length;

  void retainTargets(Iterable<String> ids) {
    final next = ids.take(8192).toSet();
    if (!setEquals(next, _targets)) {
      _targets = next;
      _items.removeWhere((id, _) => !next.contains(id));
      ready = false;
    }
  }

  void _emit() {
    if (!_disposed) notifyListeners();
  }

  Future<bool> ensure({bool refresh = false}) {
    if (_disposed) return Future.value(false);
    if (_load != null) return _load!;
    if (ready && !refresh) return Future.value(true);
    final request = _read();
    _load = request;
    return request.whenComplete(() {
      if (identical(_load, request)) _load = null;
    });
  }

  Future<bool> _read() async {
    try {
      final value = feedbackValue(
        await api.rpc(
          'messageFeedback.list',
          payload: {'sessionId': sessionId},
          scope: scope,
        ),
      );
      if (_disposed) return false;
      if (value['items'] is! List) throw DshException('protocol', '反馈列表格式无效。');
      _items.clear();
      var bytes = 0;
      for (final item in objects(value['items'])) {
        final id = item['messageId'];
        if (id is! String || !_targets.contains(id)) continue;
        _validate(item, id);
        bytes += utf8.encode(jsonEncode(item)).length;
        if (bytes > 2 * 1024 * 1024) {
          throw DshException('feedback-budget', '反馈内容过多，请缩小历史范围。');
        }
        _items[id] = item;
      }
      ready = true;
      error = null;
      _emit();
      return true;
    } catch (e) {
      if (!_disposed) {
        ready = false;
        error = '$e';
        _emit();
      }
      return false;
    }
  }

  void _validate(Json value, String id) {
    if (value['messageId'] != id ||
        value['version'] is! String ||
        !['positive', 'negative'].contains(value['rating'])) {
      throw DshException('protocol', '服务未返回有效的消息评价。');
    }
  }

  Future<bool> save(
    String id,
    String? rating, {
    required String? ifVersion,
    String? note,
    String? category,
  }) async {
    if (_disposed || busy || !_targets.contains(id)) return false;
    if (!ready && !await ensure()) return false;
    if (_disposed || busy) return false;
    busy = true;
    error = null;
    _emit();
    try {
      if (rating == null && ifVersion == null) {
        throw DshException('protocol', '缺少评价版本。');
      }
      final result = await api.rpc(
        rating == null ? 'messageFeedback.delete' : 'messageFeedback.put',
        payload: {
          'sessionId': sessionId,
          'messageId': id,
          'ifVersion': ifVersion,
          'rating': ?rating,
          if (note != null && note.trim().isNotEmpty) 'note': note.trim(),
          if (category != null && category.isNotEmpty) 'category': category,
        },
        mutation: true,
        scope: scope,
      );
      if (_disposed) return false;
      if (result['ok'] == false &&
          object(result['error'])['code'] == 'version-conflict') {
        final current = object(result['error'])['current'];
        if (current == null) {
          _items.remove(id);
        } else {
          final row = object(current);
          _validate(row, id);
          _items[id] = row;
        }
      }
      final value = feedbackValue(result);
      if (rating == null) {
        if (value['absent'] != true) {
          throw DshException('protocol', '服务未确认撤销评价。');
        }
        _items.remove(id);
      } else {
        _validate(value, id);
        _items[id] = value;
      }
      return true;
    } catch (e) {
      if (!_disposed) {
        error = '$e';
        if (e is DshException && e.outcomeUnknown) ready = false;
      }
      return false;
    } finally {
      busy = false;
      _emit();
    }
  }

  @override
  void dispose() {
    _disposed = true;
    scope.cancel();
    _items.clear();
    _targets.clear();
    super.dispose();
  }
}
