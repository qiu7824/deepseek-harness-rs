import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

const providerMessage =
    'The third-party model provider is experiencing issues and is currently '
    'not available. Please try this model again later. '
    '(trace ID: 5a7e4b578018918955c988bb308beb7a)';

void main() {
  test('the reported turn-end envelope retains provider diagnostics', () {
    final reason = {
      'kind': 'error',
      'error': {
        'message': providerMessage,
        'code': 'INVALID_REQUEST',
        'status': 400,
      },
    };
    for (final input in [reason, jsonEncode(reason)]) {
      final failure = DshErrorInfo.tryParse(input)!;
      expect(failure.message, providerMessage);
      expect(failure.code, 'INVALID_REQUEST');
      expect(failure.status, 400);
      expect(failure.traceId, '5a7e4b578018918955c988bb308beb7a');
      expect(failure.envelope, reason);
    }
  });

  test('RPC failure metadata survives without inventing an HTTP status', () {
    final failure = DshErrorInfo.tryParse({
      'type': 'server-response',
      'result': {
        'ok': false,
        'error': {
          'message': providerMessage,
          'code': 'UNAVAILABLE',
          'status': 503,
          'traceId': 'explicit-trace',
          'providerRetryAfterMs': 2500,
          'requestId': 'provider-request',
          'details': {'reason': 'provider-unavailable'},
        },
      },
    })!;
    expect(failure.status, 503);
    expect(failure.traceId, 'explicit-trace');
    expect(failure.details['reason'], 'provider-unavailable');
    expect(failure.details['providerRetryAfterMs'], 2500);
    expect(failure.details['requestId'], 'provider-request');
    expect(
      DshErrorInfo.tryParse({
        'message': 'provider error',
        'code': 'INVALID_REQUEST',
      }, errorObject: true)!.status,
      isNull,
    );
  });

  test('business JSON and successful carriers do not become errors', () {
    final business = {
      'message': 'normal tool result',
      'code': 'INVALID_REQUEST',
      'status': 400,
      'rows': [1, 2],
    };
    expect(DshErrorInfo.tryParse(business), isNull);
    expect(DshErrorInfo.tryParse(jsonEncode(business)), isNull);
    expect(DshErrorInfo.tryParse({'error': {'message': 'ordinary row'}}), isNull);
    expect(
      DshErrorInfo.tryParse({'ok': true, 'error': {'message': 'ordinary row'}}, errorObject: true),
      isNull,
    );
    expect(
      DshErrorInfo.tryParse({
        'type': 'server-response',
        'result': {'ok': true, 'value': business},
      }),
      isNull,
    );
  });

  test('malformed and oversized errors remain unparsed without throwing', () {
    for (final input in <Object?>[
      null,
      '{broken',
      '[]',
      {'kind': 'error', 'error': 5},
      {'kind': 'error', 'error': {'message': 5}},
      {'kind': 'error', 'error': {1: 'invalid key'}},
      jsonEncode({'kind': 'error', 'error': {'message': ''.padRight(65536, 'x')}}),
    ]) {
      expect(DshErrorInfo.tryParse(input), isNull);
    }
  });
}
