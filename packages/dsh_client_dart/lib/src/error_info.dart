import 'dart:convert';

import 'models.dart';

/// A declared failure, separate from ordinary JSON returned by a tool or RPC.
class DshErrorInfo {
  const DshErrorInfo({
    required this.message,
    required this.envelope,
    required this.details,
    this.code,
    this.status,
    this.traceId,
  });

  final String message;
  final String? code, traceId;
  final int? status;
  final Json envelope, details;

  /// Direct error objects are accepted only at an already failed HTTP/RPC
  /// boundary. A business result containing message/code/status is not one.
  static DshErrorInfo? tryParse(
    Object? value, {
    bool errorObject = false,
  }) {
    if (value is String) {
      if (value.length > 65536) return null;
      try {
        value = jsonDecode(value);
      } on FormatException {
        return null;
      }
    }
    if (value is! Map || value.keys.any((key) => key is! String)) {
      return null;
    }
    final envelope = _object(value);
    var failure = envelope;
    if (envelope['type'] == 'server-response') {
      final result = _object(envelope['result']);
      if (result['ok'] != false) return null;
      failure = _object(result['error']);
    } else if (envelope['kind'] == 'error' || envelope['ok'] == false) {
      failure = _object(envelope['error']);
    } else if (envelope['ok'] == true || envelope['kind'] == 'success') {
      return null;
    } else if (errorObject && envelope['error'] is Map) {
      failure = _object(envelope['error']);
    } else if (!errorObject) {
      return null;
    }
    final message = _text(failure['message']);
    if (message == null) return null;
    final nested = _object(failure['details']);
    final details = <String, dynamic>{
      ...nested,
      for (final entry in failure.entries)
        if (!const {'code', 'message', 'details'}.contains(entry.key))
          entry.key: entry.value,
    };
    final status =
        statusFrom(failure['status']) ??
        statusFrom(failure['statusCode']) ??
        statusFrom(nested['status']) ??
        statusFrom(nested['httpStatus']) ??
        statusFrom(envelope['httpStatus']) ??
        statusFrom(envelope['status']);
    final traceId =
        _text(failure['traceId']) ??
        _text(failure['trace_id']) ??
        _text(nested['traceId']) ??
        _text(nested['trace_id']) ??
        RegExp(
          r'\btrace[ _-]?id\s*[:=]\s*([a-zA-Z0-9_.:-]+)',
          caseSensitive: false,
        ).firstMatch(message)?.group(1);
    return DshErrorInfo(
      message: message,
      code: _text(failure['code']),
      status: status,
      traceId: traceId,
      envelope: envelope,
      details: details,
    );
  }

  static int? statusFrom(Object? value) {
    final status = value is int
        ? value
        : value is String
        ? int.tryParse(value)
        : null;
    return status != null && status >= 100 && status <= 599 ? status : null;
  }

  static String? _text(Object? value) =>
      value is String && value.trim().isNotEmpty ? value : null;

  static Json _object(Object? value) =>
      value is Map && value.keys.every((key) => key is String)
      ? object(value)
      : const {};
}
