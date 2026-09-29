import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

void main() {
  test('new conversation creates one selected blank session; a queued prompt uses it', () async {
    final api = FakeClient();
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    final workspace = {
      'workspaceId': 'w',
      'path': r'E:\project',
      'title': 'project',
      'sessionIds': <String>[],
    };
    c.workspaces = [workspace];
    c.workspaceId = 'w';
    final creating = Completer<Json>();
    var creates = 0, prompts = 0;
    api.handleCall = (method, payload) async {
      if (method == 'session.create') {
        creates++;
        return creating.future;
      }
      if (method == 'session.prompt') {
        prompts++;
        expect(payload['sessionId'], 'created');
        return {'accepted': true};
      }
      if (method == 'workspace.list') {
        return {
          'items': [workspace],
          'archivedSessionIds': [],
        };
      }
      return {'items': [], 'archivedSessionIds': []};
    };
    final starting = c.startConversation();
    final duplicate = c.startConversation();
    expect(creates, 1);
    final sending = c.sendParts('请处理', []);
    api.liveSessions = [
      SessionSummary.fromJson({
        'sessionId': 'created',
        'cwd': r'E:\project',
        'blank': true,
      }),
    ];
    api.histories['created'] = Future.value(
      HistoryPage.fromJson({'events': []}),
    );
    creating.complete({'sessionId': 'created'});
    await Future.wait([starting, duplicate]);
    expect(c.selectedId, 'created');
    expect(c.blankConversation, isTrue);
    expect(await sending, 'created');
    expect(prompts, 1);
    c.dispose();
  });

  test('late blank creation cannot replace another selected task', () async {
    final api = FakeClient();
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    c.workspaces = [
      {'workspaceId': 'w', 'path': r'E:\project'},
    ];
    c.workspaceId = 'w';
    final creating = Completer<Json>();
    api.handleCall = (method, payload) async {
      if (method == 'session.create') return creating.future;
      return {'items': [], 'archivedSessionIds': []};
    };
    final starting = c.startConversation();
    await c.select('other');
    creating.complete({'sessionId': 'stale'});
    await starting;
    expect(c.selectedId, 'other');
    c.dispose();
  });

  test(
    'new conversation starts in the Workspace of the selected folder',
    () async {
      final api = FakeClient();
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await Future<void>.delayed(Duration.zero);
      final workspaces = [
        {
          'workspaceId': 'first',
          'path': r'E:\first',
          'title': 'first',
          'sessionIds': <String>[],
        },
        {
          'workspaceId': 'road',
          'path': r'E:\资料\交通\龙江路',
          'title': '龙江路',
          'sessionIds': <String>[],
        },
      ];
      Json? created;
      api.handleCall = (method, payload) async {
        if (method == 'session.create') {
          created = payload;
          return {'sessionId': 'next'};
        }
        return {'items': workspaces, 'archivedSessionIds': []};
      };
      c.workspaces = workspaces;
      c.workspaceId = 'first';
      // The Host reports this session's folder with the extended-length prefix
      // and without Workspace membership; the sidebar still groups it there.
      c.sessions = [
        SessionSummary.fromJson({
          'sessionId': 'task',
          'cwd': r'\\?\E:\资料\交通\龙江路',
        }),
      ];
      await c.select('task');
      expect(c.workspaceId, 'road');
      await c.startConversation();
      expect(created?['cwd'], r'E:\资料\交通\龙江路');
      expect(created?['workspaceId'], 'road');
      c.dispose();
    },
  );

  test('refresh keeps the chosen Workspace, else resumes the latest', () async {
    final api = FakeClient();
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    final items = [
      {
        'workspaceId': 'first',
        'path': r'E:\first',
        'sessionIds': ['old'],
      },
      {'workspaceId': 'latest', 'path': r'E:\latest', 'sessionIds': []},
    ];
    api.liveSessions = [
      SessionSummary.fromJson({
        'sessionId': 'old',
        'cwd': r'E:\first',
        'updatedAt': 1,
      }),
      SessionSummary.fromJson({
        'sessionId': 'new',
        'cwd': r'\\?\E:\latest',
        'updatedAt': 2,
      }),
    ];
    api.handleCall = (method, payload) async => {
      'items': items,
      'archivedSessionIds': [],
    };
    c.workspaceId = null;
    await c.refreshSessions();
    expect(c.workspaceId, 'latest');
    c.targetWorkspace('first');
    await c.refreshSessions();
    expect(c.workspaceId, 'first');
    c.dispose();
  });

  testWidgets(
    'selected blank session keeps the hero and hides active conversation chrome',
    (tester) async {
      final c = DesktopController(MemoryPreferences())
        ..selectedId = 'blank'
        ..sessions = [
          SessionSummary.fromJson({
            'sessionId': 'blank',
            'cwd': r'E:\project',
            'blank': true,
          }),
        ];
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('composer-card')), findsOneWidget);
      expect(find.text('轨迹'), findsNothing);
      c.sessions.single.blank = false;
      c.emit();
      await tester.pumpAndSettle();
      expect(find.text('轨迹'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
}
