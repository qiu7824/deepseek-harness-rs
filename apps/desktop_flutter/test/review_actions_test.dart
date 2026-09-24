import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/features/conversation/permission_control.dart';
import 'package:dsh_desktop/features/conversation/attachment_view.dart';
import 'package:dsh_desktop/features/conversation/artifacts_view.dart';
import 'package:dsh_desktop/features/workspace_source_dialog.dart';
import 'package:dsh_desktop/features/shell.dart';
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'workbench_test.dart' show TestController;

const modes = [
  {'value': 'workspace-write', 'name': 'workspace-write'},
  {'value': 'read-only', 'name': 'read-only'},
  {'value': 'danger-full-access', 'name': 'danger-full-access'},
];

class ReviewApi extends DshClient {
  ReviewApi() : super('http://127.0.0.1');
  final calls = <({String method, Json data})>[];
  Completer<Json>? command;
  RequestScope? lastScope;
  bool rejectCreate = false;
  String mode = 'workspace-write';
  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    calls.add((method: method, data: payload));
    lastScope = scope;
    if (method == 'commands.execute') {
      mode = '${object(payload['args'])['line']}'.split(' ').last;
      return command?.future ??
          {
            'result': {'kind': 'success'},
          };
    }
    if (method == 'session.history') {
      return {
        'projections': {
          'asOfSeq': 99,
          'values': {
            'permissions': {'currentValue': mode, 'options': modes},
          },
        },
      };
    }
    if (method == 'workspace.create') {
      if (rejectCreate) throw StateError('克隆失败');
      return {
        'workspace': {'workspaceId': 'new-workspace'},
      };
    }
    if (method == 'session.attachment') {
      return {
        'data': 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==',
      };
    }
    return {};
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    calls.add((method: path, data: body ?? {}));
    lastScope = scope;
    if (path == '/__dsh-artifacts/list') {
      return {
        'entries': [
          {'path': r'\\?\C:\产物\报告.pdf', 'change': 'created', 'size': 12},
        ],
      };
    }
    return {'items': []};
  }
}

class PermissionTestController extends TestController {
  PermissionTestController(this.api) {
    selectedId = 's';
  }
  final ReviewApi api;
  @override
  DshClient get client => api;
}

void main() {
  testWidgets('images and artifact rows expose secondary-click menus', (
    tester,
  ) async {
    final api = ReviewApi();
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: AttachmentView(
            client: api,
            sessionId: 's',
            attachment: const {'attachmentId': 'image', 'name': '图片.png'},
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(
      find.byType(AttachmentView),
      buttons: kSecondaryMouseButton,
    );
    await tester.pumpAndSettle();
    expect(find.text('查看原图'), findsOneWidget);
    expect(find.text('保存图片'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: ArtifactsView(api: api, session: 's'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(
      find.text(r'C:\产物\报告.pdf'),
      buttons: kSecondaryMouseButton,
    );
    await tester.pumpAndSettle();
    expect(find.text('复制路径'), findsOneWidget);
    expect(find.text('保存原文件副本'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    await tester.pumpWidget(const SizedBox());
    final count = api.calls.length;
    await tester.pump(const Duration(seconds: 10));
    expect(api.calls.length, count);
    await api.close();
  });
  testWidgets(
    'permission menu checks current state and refreshes only projections',
    (tester) async {
      final api = ReviewApi(), c = PermissionTestController(ReviewApi());
      final source = c.api;
      c.transcript = [
        TranscriptItem(id: 'history', kind: 'user', text: '正在阅读旧消息'),
      ];
      c.projectionWindow.apply('permissions', {
        'currentValue': 'workspace-write',
        'options': modes,
      }, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(child: PermissionControl(controller: c)),
          ),
        ),
      );
      await tester.tap(find.byType(ComposerAction));
      await tester.pumpAndSettle();
      expect(find.byType(PopupMenuItem<String>), findsNWidgets(3));
      await tester.tap(find.text('工作区内修改'));
      await tester.pumpAndSettle();
      expect(source.calls, isEmpty);
      await tester.tap(find.byType(ComposerAction));
      await tester.pumpAndSettle();
      await tester.tap(find.text('只读'));
      await tester.pumpAndSettle();
      expect(
        object(source.calls.first.data['args'])['line'],
        '/permission read-only',
      );
      expect(object(c.projections['permissions'])['currentValue'], 'read-only');
      expect(c.transcript.single.text, '正在阅读旧消息');
      expect(
        tester.widget<ComposerAction>(find.byType(ComposerAction)).label,
        contains('只读'),
      );
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await source.close();
      await api.close();
    },
  );

  testWidgets(
    'unmounted permission request is cancelled and cannot update another session',
    (tester) async {
      final api = ReviewApi()..command = Completer<Json>();
      final c = PermissionTestController(api);
      c.projectionWindow.apply('permissions', {
        'currentValue': 'workspace-write',
        'options': modes,
      }, 1);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: Center(child: PermissionControl(controller: c)),
          ),
        ),
      );
      await tester.tap(find.byType(ComposerAction));
      await tester.pumpAndSettle();
      await tester.tap(find.text('只读'));
      await tester.pumpAndSettle();
      final scope = api.lastScope!;
      c.selectedId = 'another';
      await tester.pumpWidget(const SizedBox());
      expect(scope.cancelled, isTrue);
      api.command!.complete({
        'result': {'kind': 'success'},
      });
      await tester.pump();
      expect(api.calls.length, 1);
      expect(
        object(c.projections['permissions'])['currentValue'],
        'workspace-write',
      );
      c.dispose();
      await api.close();
    },
  );

  testWidgets(
    'secondary click on actual Markdown link and file card copies its target',
    (tester) async {
      String? copied;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied = (call.arguments as Map)['text'];
          }
          return null;
        },
      );
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SizedBox(
              width: 500,
              child: MessageCard(
                item: TranscriptItem(
                  id: 'link',
                  kind: 'assistant',
                  text: '[网页地址](https://example.org/page)',
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final text = find.byType(SelectableText).first;
      await tester.tapAt(
        tester.getTopLeft(text) + const Offset(12, 12),
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      expect(find.text('复制链接地址'), findsOneWidget);
      await tester.tap(find.text('复制链接地址'));
      await tester.pumpAndSettle();
      expect(copied, 'https://example.org/page');
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: UploadedFileCard(
              file: const UploadedFileReceipt('a.txt', r'\\?\C:\项目\a.txt', 12),
            ),
          ),
        ),
      );
      await tester.tap(
        find.byType(UploadedFileCard),
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('复制文件路径'));
      await tester.pumpAndSettle();
      expect(copied, r'C:\项目\a.txt');
      await tester.pumpWidget(const SizedBox());
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      );
    },
  );

  testWidgets(
    'workspace form retains failures and does not apply advanced settings before submit',
    (tester) async {
      final submissions = <String>[];
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: NewTaskDialog(
              initialPath: r'\\?\C:\项目',
              onCreate: (path, scratch) async {
                submissions.add('$path|$scratch');
                throw StateError('目录不可写');
              },
            ),
          ),
        ),
      );
      expect(find.text(r'C:\项目'), findsOneWidget);
      expect(submissions, isEmpty);
      await tester.tap(find.text('高级设置'));
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField), r'D:\缓存');
      expect(submissions, isEmpty);
      await tester.tap(find.byKey(const Key('create-task')));
      await tester.pumpAndSettle();
      expect(submissions.single, r'C:\项目|D:\缓存');
      expect(find.textContaining('目录不可写'), findsOneWidget);
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.text,
        r'D:\缓存',
      );
    },
  );

  testWidgets(
    'clone sends selected kind and retains failed form; SSH validates ports and releases polling',
    (tester) async {
      final api = ReviewApi()..rejectCreate = true;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: WorkspaceSourceDialog(api: api, kind: 'cloud'),
          ),
        ),
      );
      Future<void> enter(String key, String text) => tester.enterText(
        find.descendant(
          of: find.byKey(ValueKey('workspace-source-$key')),
          matching: find.byType(TextField),
        ),
        text,
      );
      await enter('source', 'https://example.org/repo.git');
      await enter('path', r'C:\repo');
      await enter('branch', 'main');
      await tester.tap(find.text('克隆并添加工作区'));
      await tester.pumpAndSettle();
      expect(api.calls.single.data['kind'], 'cloud');
      expect(api.calls.single.data['branch'], 'main');
      expect(api.calls.single.data['operationId'], isNotEmpty);
      expect(find.textContaining('克隆失败'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: WorkspaceSourceDialog(api: api, kind: 'ssh'),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await enter('host', 'host');
      await enter('path', '/project');
      await enter('port', '70000');
      await tester.tap(find.text('连接远端工作目录'));
      await tester.pumpAndSettle();
      expect(api.calls.where((v) => v.method.endsWith('/connect')), isEmpty);
      await tester.pumpWidget(const SizedBox());
      final count = api.calls.length;
      await tester.pump(const Duration(seconds: 10));
      expect(api.calls.length, count);
      await api.close();
    },
  );
}
