import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/foundation.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

const feedbackCategories = {
  'task-result': DshConversationZh.feedbackTaskResult,
  'instruction-following': DshConversationZh.feedbackInstructions,
  'product-interaction': DshConversationZh.feedbackInteraction,
  'service-stability': DshConversationZh.feedbackReliability,
  'resource-cost': DshConversationZh.feedbackResources,
  'security-privacy-permission': DshConversationZh.feedbackSecurity,
  'other': DshConversationZh.feedbackOther,
};

/// The RPC carrier has already been decoded by DshClient. Feedback methods
/// return another Result; an HTTP success is not a persisted feedback receipt.
Json feedbackValue(Json result) {
  if (result['ok'] == false) {
    final error = object(result['error']);
    final code = '${error['code'] ?? 'feedback-failed'}';
    throw DshException(code, switch (code) {
      'version-conflict' => DshConversationZh.feedbackConflict,
      'target-not-found' => DshConversationZh.feedbackMessageUnavailable,
      'session-not-found' => DshConversationZh.sessionUnavailable,
      'note-too-large' => DshConversationZh.feedbackNoteTooLong,
      _ => '${error['message'] ?? DshConversationZh.feedbackSaveFailed}',
    });
  }
  if (result['ok'] != true || result['value'] is! Map) {
    throw DshException(
      'protocol',
      DshConversationZh.feedbackConfirmationInvalid,
    );
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
  Object? error;
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
      if (value['items'] is! List) {
        throw DshException('protocol', DshConversationZh.feedbackListInvalid);
      }
      _items.clear();
      var bytes = 0;
      for (final item in objects(value['items'])) {
        final id = item['messageId'];
        if (id is! String || !_targets.contains(id)) continue;
        _validate(item, id);
        bytes += utf8.encode(jsonEncode(item)).length;
        if (bytes > 2 * 1024 * 1024) {
          throw DshException(
            'feedback-budget',
            DshConversationZh.feedbackHistoryTooLarge,
          );
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
        error = e;
        _emit();
      }
      return false;
    }
  }

  void _validate(Json value, String id) {
    if (value['messageId'] != id ||
        value['version'] is! String ||
        !['positive', 'negative'].contains(value['rating'])) {
      throw DshException('protocol', DshConversationZh.feedbackResponseInvalid);
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
        throw DshException(
          'protocol',
          DshConversationZh.feedbackRevisionMissing,
        );
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
          throw DshException(
            'protocol',
            DshConversationZh.feedbackRemovalUnconfirmed,
          );
        }
        _items.remove(id);
      } else {
        _validate(value, id);
        _items[id] = value;
      }
      return true;
    } catch (e) {
      if (!_disposed) {
        error = e;
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
