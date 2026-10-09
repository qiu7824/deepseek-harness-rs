import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/account_usage_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'account_usage_panel_test.dart' as fixture;

class DeferredUsageApi extends fixture.UsageApi {
  final replies = <Completer<Json>>[];
  final scopes = <RequestScope?>[];

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) {
    requests.add(body ?? {});
    scopes.add(scope);
    final reply = Completer<Json>();
    replies.add(reply);
    return reply.future;
  }
}

void main() {
  testWidgets(
    'disclosure waits for click, cancels on collapse, ignores late replies',
    (tester) async {
      final api = DeferredUsageApi();
      final c = fixture.UsageController(api)..account('a');
      var cancelled = false;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: AccountUsageDisclosure(
              controller: c,
              api: api,
              provider: 'devin',
              accountScope: 'a',
              visible: true,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(api.requests, isEmpty);
      await tester.tap(find.text('查看额度'));
      await tester.pump();
      expect(api.requests, hasLength(1));
      api.scopes.single?.register(() => cancelled = true);
      await tester.tap(find.text('收起额度'));
      await tester.pumpAndSettle();
      expect(cancelled, isTrue);
      api.replies.single.complete(fixture.response());
      await tester.pumpAndSettle();
      expect(find.byType(LinearProgressIndicator), findsNothing);
      await tester.tap(find.text('查看额度'));
      await tester.pump();
      expect(api.requests, hasLength(2));
      c.account('b');
      await tester.pumpAndSettle();
      api.replies.last.complete(fixture.response());
      await tester.pumpAndSettle();
      expect(api.requests, hasLength(2));
      expect(find.text('查看额度'), findsOneWidget);
      expect(find.text('已用 37% · 剩余 63%'), findsNothing);
      await fixture.cleanup(tester, c, api);
    },
  );

  testWidgets(
    'hiding and reopening account details requires another usage click',
    (tester) async {
      final api = fixture.UsageApi();
      final c = fixture.UsageController(api)..account('a');
      Future<void> mount(bool visible) async {
        await tester.pumpWidget(
          ShadApp(
            home: Scaffold(
              body: AccountUsageDisclosure(
                controller: c,
                api: api,
                provider: 'devin',
                accountScope: 'a',
                visible: visible,
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
      }

      await mount(true);
      expect(api.requests, isEmpty);
      await tester.tap(find.text('查看额度'));
      await tester.pumpAndSettle();
      expect(api.requests, hasLength(1));
      await mount(false);
      await mount(true);
      expect(api.requests, hasLength(1));
      expect(find.text('查看额度'), findsOneWidget);
      expect(find.byType(LinearProgressIndicator), findsNothing);
      await fixture.cleanup(tester, c, api);
    },
  );
}
