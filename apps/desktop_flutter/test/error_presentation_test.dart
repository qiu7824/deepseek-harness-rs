import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/error.dart';
import 'package:dsh_desktop/l10n/zh.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

const providerMessage =
    'The third-party model provider is experiencing issues and is currently '
    'not available. Please try this model again later. '
    '(trace ID: 5a7e4b578018918955c988bb308beb7a)';
const providerSummary =
    '模型提供方返回暂不可用，本次请求未完成。请稍后再试，或切换其他模型。';

Json providerReason({int status = 400, String code = 'INVALID_REQUEST'}) => {
  'kind': 'error',
  'error': {'message': providerMessage, 'code': code, 'status': status},
};

void main() {
  test('live and reopened turn errors share the friendly presentation', () {
    for (final live in [true, false]) {
      final reason = providerReason();
      final item = projectTranscript([
        HistoryEvent.fromJson({
          'seq': 3,
          'type': 'turn/end',
          'data': {'turn': 1, 'reason': reason},
        }),
      ], live: live).single;
      expect(item.kind, 'error');
      // The history/export source remains intact; only its presentation changes.
      expect(jsonDecode(item.clipboardText), reason);
      final description = DshError.describe(item.text);
      expect(description.message, providerSummary);
      expect(description.code, 'INVALID_REQUEST');
      expect(description.status, 400);
      expect(description.traceId, '5a7e4b578018918955c988bb308beb7a');
      expect(description.retryable, isFalse);
      expect(description.details, contains(providerMessage));
    }
  });

  test('unavailable 503 preserves its real status and manual recovery', () {
    final description = DshError.describe(
      jsonEncode(providerReason(status: 503, code: 'SERVER')),
    );
    expect(description.message, providerSummary);
    expect(description.code, 'SERVER');
    expect(description.status, 503);
    expect(description.retryable, isTrue);
    expect(description.details, contains('status: 503'));
  });

  test('INVALID_REQUEST alone does not claim the user supplied bad parameters', () {
    final description = DshError.describe(jsonEncode({
      'kind': 'error',
      'error': {'code': 'INVALID_REQUEST', 'message': 'provider rejected this request'},
    }));
    expect(description.message, 'provider rejected this request');
    expect(description.status, isNull);
    expect(description.details, contains('INVALID_REQUEST'));
  });

  test('business JSON, cancellation and unknown outcomes retain their semantics', () {
    final business = jsonEncode({'message': 'ordinary result', 'code': 'row', 'status': 400});
    expect(DshError.describe(business).message, business);
    expect(DshError.describe(business).status, isNull);
    final providerBusiness = jsonEncode({'message': providerMessage, 'ok': true});
    expect(DshError.describe(providerBusiness).message, providerBusiness);
    final arrayBusiness = jsonEncode([{'message': providerMessage}]);
    expect(DshError.describe(arrayBusiness).message, arrayBusiness);
    expect(DshError.describe(DshException('cancelled', 'stopped')).cancelled, isTrue);
    final unknown = DshError.describe(DshException(
      'transport',
      jsonEncode(providerReason()),
      outcomeUnknown: true,
    ));
    expect(unknown.message, DshZh.outcomeUnknown);
    expect(unknown.retryable, isFalse);
  });

  test('ordinary diagnostic metadata cannot replace a trusted primary error', () {
    for (final details in <Json>[
      {'message': 'ordinary detail', 'code': 'row', 'status': 504},
      {'error': {'message': providerMessage, 'code': 'SERVER'}, 'status': 504},
    ]) {
      final description = DshError.describe(DshException(
        'TIMEOUT',
        'deadline exceeded',
        details: details,
      ));
      expect(description.code, 'TIMEOUT');
      expect(description.message, DshZh.requestTimeout);
      expect(description.status, 504);
      expect(description.details, contains('deadline exceeded'));
    }
  });

  test('a real HTTP 400 remains non-retryable when its body claims 503', () {
    final description = DshError.describe(DshException(
      'http-400',
      providerMessage,
      details: {...providerReason(status: 503, code: 'SERVER'), 'httpStatus': 400},
    ));
    expect(description.message, providerSummary);
    expect(description.code, 'http-400');
    expect(description.status, 400);
    expect(description.retryable, isFalse);
    expect(description.details, contains('503'));
  });

  test('native CANCELLED and TIMEOUT codes keep cancellation and timeout semantics', () {
    final cancelled = DshError.describe(jsonEncode({
      'kind': 'error',
      'error': {'message': 'request canceled [canceled]', 'code': 'CANCELLED', 'status': 499},
    }));
    expect(cancelled.message, DshZh.operationCancelled);
    expect(cancelled.cancelled, isTrue);
    expect(cancelled.retryable, isFalse);
    final timeout = DshError.describe(jsonEncode({
      'kind': 'error',
      'error': {'message': 'deadline exceeded [deadline_exceeded]', 'code': 'TIMEOUT', 'status': 504},
    }));
    expect(timeout.message, DshZh.requestTimeout);
    expect(timeout.status, 504);
    expect(timeout.cancelled, isFalse);
  });

  testWidgets('the reported envelope shows a summary and folded diagnostic details', (tester) async {
    var retries = 0;
    await tester.pumpWidget(ShadApp(home: Scaffold(body: DshErrorView(
      error: jsonEncode(providerReason()),
      onRetry: () => retries++,
    ))));
    expect(find.text(providerSummary), findsOneWidget);
    expect(find.textContaining('{"kind"'), findsNothing);
    expect(find.textContaining('INVALID_REQUEST'), findsNothing);
    expect(find.byType(SelectableText), findsNothing);
    expect(find.text(DshZh.retry), findsNothing);
    expect(retries, 0);

    await tester.tap(find.text(DshZh.details));
    await tester.pumpAndSettle();
    final details = tester.widget<SelectableText>(find.byType(SelectableText)).data!;
    expect(details, contains('code: INVALID_REQUEST'));
    expect(details, contains('status: 400'));
    expect(details, contains('trace ID: 5a7e4b578018918955c988bb308beb7a'));
    expect(retries, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('RPC and HTTP diagnostics remain redacted when expanded', (tester) async {
    final error = DshException(
      'http-400',
      providerMessage,
      details: {
        ...providerReason(),
        'httpStatus': 400,
        'api_key': 'private-api-key',
        'Authorization': 'Bearer private-auth-token',
      },
    );
    await tester.pumpWidget(ShadApp(home: Scaffold(body: DshErrorView(error: error))));
    expect(find.text(providerSummary), findsOneWidget);
    await tester.tap(find.text(DshZh.details));
    await tester.pumpAndSettle();
    final details = tester.widget<SelectableText>(find.byType(SelectableText)).data!;
    expect(details, contains('INVALID_REQUEST'));
    expect(details, contains('400'));
    expect(details, contains('5a7e4b578018918955c988bb308beb7a'));
    expect(details, isNot(contains('private-api-key')));
    expect(details, isNot(contains('private-auth-token')));
    expect(tester.takeException(), isNull);
  });
}
