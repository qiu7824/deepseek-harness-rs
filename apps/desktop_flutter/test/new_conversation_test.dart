import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/features/workspace_tree_row.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

void main() {
  testWidgets(
    'selecting a sidebar workspace is inherited by New Conversation',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1280, 800));
      final api = FakeClient();
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await tester.pump();
      final workspaces = [
        {
          'workspaceId': 'first',
          'path': r'E:\first',
          'title': '工作区一',
          'sessionIds': ['old'],
        },
        {
          'workspaceId': 'second',
          'path': r'E:\second',
          'title': '工作区二',
          'sessionIds': <String>[],
        },
      ];
      c.workspaces = workspaces;
      c.workspaceId = 'first';
      c.selectedId = 'old';
      c.sessions = [
        SessionSummary.fromJson({'sessionId': 'old', 'cwd': r'E:\first'}),
      ];
      Json? createdWith;
      api.handleCall = (method, payload) async {
        if (method == 'workspace.list') return {'items': workspaces};
        if (method == 'session.create') {
          createdWith = payload;
          api.liveSessions = [
            SessionSummary.fromJson({
              'sessionId': 'new',
              'cwd': payload['cwd'],
              'blank': true,
            }),
          ];
          api.histories['new'] = Future.value(
            HistoryPage.fromJson({'events': []}),
          );
          return {'sessionId': 'new'};
        }
        return {'items': []};
      };
      addTearDown(() async {
        await tester.pumpWidget(const SizedBox());
        c.dispose();
        await tester.binding.setSurfaceSize(null);
      });
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      await tester.tap(find.text('工作区二'));
      await tester.pumpAndSettle();
      expect(c.workspaceId, 'second');
      expect(c.selectedId, 'old');
      expect(createdWith, isNull);
      expect(
        tester
            .widgetList<WorkspaceTreeRow>(find.byType(WorkspaceTreeRow))
            .where((w) => w.active)
            .single
            .title,
        '工作区二',
      );
      await tester.tap(find.byKey(const Key('new-task')));
      await tester.pumpAndSettle();
      expect(createdWith?['workspaceId'], 'second');
      expect(createdWith?['cwd'], r'E:\second');
      expect(c.selectedId, 'new');
      expect(tester.takeException(), isNull);
    },
  );

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

  for (final retargetWhileRestoring in [false, true]) {
    test(
      retargetWhileRestoring
          ? 'cold restoration preserves an explicit Workspace chosen while loading'
          : 'cold restoration adopts the saved session folder without membership',
      () async {
        final api = FakeClient();
        final preferences = MemoryPreferences()..sessionId = 'saved';
        final listed = Completer<Json>();
        final requested = Completer<void>();
        final workspaces = [
          {
            'workspaceId': 'saved-folder',
            'path': r'E:\资料\保存的任务',
            'sessionIds': <String>[],
          },
          {
            'workspaceId': 'latest-folder',
            'path': r'E:\latest',
            'sessionIds': <String>[],
          },
        ];
        api.liveSessions = [
          SessionSummary.fromJson({
            'sessionId': 'saved',
            'cwd': r'\\?\E:\资料\保存的任务',
            'updatedAt': 1,
          }),
          SessionSummary.fromJson({
            'sessionId': 'latest',
            'cwd': r'E:\latest',
            'updatedAt': 2,
          }),
        ];
        Json? created;
        api.handleCall = (method, payload) async {
          if (method == 'workspace.list') {
            if (!requested.isCompleted) requested.complete();
            return listed.future;
          }
          if (method == 'session.create') {
            created = payload;
            api.liveSessions.add(
              SessionSummary.fromJson({
                'sessionId': 'created',
                'cwd': payload['cwd'],
                'blank': true,
              }),
            );
            return {'sessionId': 'created'};
          }
          return {'items': []};
        };
        final c = DesktopController(preferences, clientFactory: (_) => api);
        addTearDown(c.dispose);
        await c.connect('http://127.0.0.1');
        await requested.future.timeout(const Duration(seconds: 2));
        expect(c.selectedId, 'saved');
        if (retargetWhileRestoring) c.targetWorkspace('latest-folder');
        listed.complete({'items': workspaces, 'archivedSessionIds': []});
        await Future<void>.delayed(Duration.zero);
        expect(c.selectedId, 'saved');
        expect(c.selected?.cwd, r'\\?\E:\资料\保存的任务');
        final workspace = retargetWhileRestoring
            ? 'latest-folder'
            : 'saved-folder';
        expect(c.workspaceId, workspace);
        await c.startConversation();
        expect(created?['workspaceId'], workspace);
        expect(
          created?['cwd'],
          retargetWhileRestoring ? r'E:\latest' : r'E:\资料\保存的任务',
        );
      },
    );
  }

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
