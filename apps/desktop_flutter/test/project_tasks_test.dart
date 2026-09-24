import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/project_tasks.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class BoardClient extends DshClient {
  BoardClient() : super('http://127.0.0.1');
  Json? saved;
  bool conflict = false;
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    if (path.endsWith('/save')) {
      saved = body;
      if (conflict) throw DshException('conflict', '任务文件已被其他窗口更新，请刷新后合并修改');
      return {'revision': 'r2', 'tasks': body!['tasks']};
    }
    return {
      'revision': 'r1',
      'tasks': [
        {
          'id': 'existing',
          'title': '保留的任务',
          'priority': 1,
          'status': 'todo',
          'detail': '原始说明',
        },
      ],
    };
  }
}

void main() {
  testWidgets(
    'project task saves preserve other rows and use the fetched revision',
    (tester) async {
      final api = BoardClient();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ProjectTasks(api: api, session: 'parent'),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('新增任务'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find
            .descendant(
              of: find.byType(TaskEditor),
              matching: find.byType(TextField),
            )
            .first,
        '核对图标',
      );
      await tester.tap(find.text('保存'));
      await tester.pumpAndSettle();
      expect(api.saved!['revision'], 'r1');
      expect(api.saved!['sessionId'], 'parent');
      final tasks = objects(api.saved!['tasks']);
      expect(tasks.length, 2);
      expect(tasks.first['detail'], '原始说明');
      expect(tasks.last['title'], '核对图标');
      expect(tasks.last['id'], matches(RegExp(r'^[a-zA-Z0-9-]+$')));
      expect(find.text('核对图标'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      await api.close();
    },
  );
  testWidgets('project task conflict preserves the editor draft', (
    tester,
  ) async {
    final api = BoardClient()..conflict = true;
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: ProjectTasks(api: api, session: 'parent'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('编辑'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find
          .descendant(
            of: find.byType(TaskEditor),
            matching: find.byType(TextField),
          )
          .first,
      '继续核对尺寸',
    );
    await tester.tap(find.text('保存'));
    await tester.pumpAndSettle();
    expect(find.byType(TaskEditor), findsOneWidget);
    expect(find.text('继续核对尺寸'), findsOneWidget);
    expect(find.textContaining('任务文件已被其他窗口更新'), findsOneWidget);
    expect(api.saved!['revision'], 'r1');
    await tester.pumpWidget(const SizedBox());
    await api.close();
  });
}
