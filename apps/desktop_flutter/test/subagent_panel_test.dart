import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/subagent_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class ChildClient extends DshClient {
  ChildClient() : super('http://127.0.0.1');
  final calls = <({String method, Json payload, bool mutation})>[];
  final scopes = <RequestScope>[];
  bool failSend = false;
  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    calls.add((method: method, payload: payload, mutation: mutation));
    if (scope != null) scopes.add(scope);
    if (method == 'subagent.list') {
      return {
        'entries': [
          {
            'kind': 'child',
            'id': 'child-1',
            'label': '检查组件',
            'mode': 'continuable',
            'activity': 'running',
            'hasChildren': false,
          },
          {
            'kind': 'child',
            'id': 'child-2',
            'label': '一次性审查',
            'mode': 'one-shot',
            'activity': 'inactive',
            'hasChildren': false,
          },
        ],
      };
    }
    if (method == 'subagent.history') {
      return {
        'events': [
          {
            'event': {
              'seq': 1,
              'type': 'assistant/message',
              'data': {
                'message': {
                  'content': [
                    {'type': 'text', 'text': '组件检查已完成'},
                  ],
                },
              },
            },
          },
        ],
        'hasMore': false,
      };
    }
    if (failSend && method == 'subagent.prompt') throw StateError('连接中断');
    return {'accepted': true};
  }
}

void main() {
  testWidgets(
    'subtask browsing and messages address the exact parent and child',
    (tester) async {
      final api = ChildClient();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 470,
              child: SubagentPanel(api: api, parent: 'root'),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(api.calls.first.payload, {'parentSessionId': 'root'});
      expect(api.calls.every((c) => !c.mutation), isTrue);
      await tester.tap(find.text('检查组件'));
      await tester.pumpAndSettle();
      final history = api.calls.last;
      expect(history.method, 'subagent.history');
      expect(find.text('组件检查已完成'), findsOneWidget);
      expect(history.payload, {
        'parentSessionId': 'root',
        'childSessionId': 'child-1',
        'mode': 'continuable',
        'maxMessages': 100,
      });
      await tester.enterText(find.byType(TextField), '检查搜索框尺寸');
      await tester.pump();
      await tester.tap(find.text('发送'));
      await tester.pumpAndSettle();
      final sent = api.calls.singleWhere((c) => c.method == 'subagent.prompt');
      expect(sent.mutation, isTrue);
      expect(sent.payload['parentSessionId'], 'root');
      expect(sent.payload['childSessionId'], 'child-1');
      expect(sent.payload['content'], [
        {'type': 'text', 'text': '检查搜索框尺寸'},
      ]);
      expect(sent.payload['delivery'], 'queue');
      expect(sent.payload['requestId'], isNotEmpty);
      await tester.tap(find.text('中断'));
      await tester.pumpAndSettle();
      expect(
        api.calls.singleWhere((c) => c.method == 'subagent.interrupt').payload,
        {
          'parentSessionId': 'root',
          'childSessionId': 'child-1',
          'mode': 'continuable',
        },
      );
      await tester.pumpWidget(const SizedBox());
      final count = api.calls.length;
      await tester.pump(const Duration(seconds: 12));
      expect(api.calls.length, count);
      expect(api.scopes.every((s) => s.cancelled), isTrue);
      api.close();
    },
  );
  testWidgets(
    'failed continuation keeps editable text and one-shot tasks cannot receive messages',
    (tester) async {
      final api = ChildClient()..failSend = true;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 470,
              child: SubagentPanel(api: api, parent: 'root'),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('检查组件'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), '保留草稿');
      await tester.pump();
      await tester.tap(find.text('发送'));
      await tester.pumpAndSettle();
      expect(find.text('保留草稿'), findsOneWidget);
      expect(find.textContaining('连接中断'), findsOneWidget);
      await tester.tap(find.byTooltip('返回子任务'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('一次性审查'));
      await tester.pumpAndSettle();
      expect(find.byType(TextField), findsNothing);
      expect(find.text('发送'), findsNothing);
      await tester.pumpWidget(const SizedBox());
      api.close();
    },
  );
}
