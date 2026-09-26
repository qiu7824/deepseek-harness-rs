import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/knowledge/knowledge_page.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class KnowledgePreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class FakeKnowledgeApi extends KnowledgeApi {
  FakeKnowledgeApi() : super(DshClient('http://127.0.0.1:9'));
  final bases = <Json>[];
  final documents = <String, List<Json>>{};
  final calls = <(String, Json)>[];

  Json view(Json base) => {
    ...base,
    'documentCount': documents[base['id']]?.length ?? 0,
    'chunkCount': (documents[base['id']] ?? []).fold<int>(
      0,
      (sum, doc) => sum + (doc['chunkCount'] as int),
    ),
  };

  Json doc(String id, String name) => {
    'id': id,
    'name': name,
    'bytes': 2048,
    'chars': 900,
    'chunkCount': 2,
    'createdAt': '2026-09-20T08:00:00.000Z',
  };

  @override
  Future<Json> call(String operation, [Json body = const {}]) async {
    calls.add((operation, body));
    final base = bases.where((row) => row['id'] == body['id']).firstOrNull;
    switch (operation) {
      case 'catalog':
        return {
          'bases': [for (final row in bases) view(row)],
          'extensions': ['pdf', 'docx', 'md'],
        };
      case 'create':
        if (bases.any((row) => row['name'] == body['name'])) {
          throw DshException('http-409', '已有同名知识库');
        }
        final created = {
          'id': 'kb-${bases.length + 1}',
          'name': body['name'],
          'description': body['description'],
          'enabled': true,
          'updatedAt': '1',
        };
        bases.add(created);
        documents[created['id'] as String] = [doc('doc-1', '手册.pdf')];
        return {'base': view(created)};
      case 'update':
        base!.addAll({
          for (final entry in body.entries)
            if (entry.key != 'id') entry.key: entry.value,
          'updatedAt': '${int.parse('${base['updatedAt']}') + 1}',
        });
        return {'base': view(base)};
      case 'delete':
        bases.remove(base);
        return {'deleted': true};
      case 'documents':
        return {'documents': documents[body['id']]};
      case 'importPath':
        if ('${body['path']}'.endsWith('bad')) {
          throw DshException('http-404', '无法访问');
        }
        documents[body['id']]!.add(doc('doc-a', 'a.txt'));
        return {
          'report': {
            'added': [doc('doc-a', 'a.txt')],
            'skipped': [
              {'name': r'D:\docs\empty.txt', 'reason': '没有可索引的文字'},
            ],
            'truncated': false,
          },
        };
      case 'deleteDocument':
        for (final rows in documents.values) {
          rows.removeWhere((row) => row['id'] == body['id']);
        }
        return {'deleted': true};
      case 'search':
        return {
          'results': [
            {
              'documentName': '手册.pdf',
              'chunk': 2,
              'snippet': '…先停止服务，再恢复上一版本…',
            },
          ],
        };
    }
    throw StateError(operation);
  }
}

Future<void> pumpPage(WidgetTester tester, FakeKnowledgeApi api) async {
  tester.view.physicalSize = const Size(1400, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    ShadApp(
      home: Scaffold(
        body: KnowledgePage(
          controller: DesktopController(KnowledgePreferences()),
          api: api,
          onClose: () {},
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Json lastCall(FakeKnowledgeApi api, String operation) =>
    api.calls.lastWhere((call) => call.$1 == operation).$2;

void main() {
  test('byte sizes are readable', () {
    expect(formatKnowledgeBytes(512), '512 B');
    expect(formatKnowledgeBytes(2048), '2.0 KB');
    expect(formatKnowledgeBytes(3 * 1048576), '3.0 MB');
  });

  testWidgets('creates a base, imports, edits, searches and deletes it', (
    tester,
  ) async {
    final api = FakeKnowledgeApi();
    await pumpPage(tester, api);
    expect(find.textContaining('还没有知识库'), findsOneWidget);

    await tester.tap(find.byKey(const Key('knowledge-create')));
    await tester.pumpAndSettle();
    final submit = find.byKey(const Key('knowledge-create-submit'));
    expect(tester.widget<DshButton>(submit).onPressed, isNull);
    await tester.enterText(
      find.descendant(
        of: find.byKey(const Key('knowledge-create-name')),
        matching: find.byType(EditableText),
      ),
      '运维手册',
    );
    await tester.pump();
    await tester.tap(submit);
    await tester.pumpAndSettle();
    expect(lastCall(api, 'create')['name'], '运维手册');
    expect(find.byKey(const Key('knowledge-detail')), findsOneWidget);
    expect(find.text('手册.pdf'), findsOneWidget);
    expect(find.text('文档 1'), findsOneWidget);

    // Imports go by path, one request each, and report skipped files.
    final state = tester.state<KnowledgeBaseDetailState>(
      find.byType(KnowledgeBaseDetail),
    );
    await state.importPaths([r'D:\docs', r'D:\bad']);
    await tester.pumpAndSettle();
    expect(
      api.calls
          .where((call) => call.$1 == 'importPath')
          .map((c) => c.$2['path']),
      [r'D:\docs', r'D:\bad'],
    );
    expect(find.text('已导入 1 个文档 · 跳过 2 个'), findsOneWidget);
    expect(find.textContaining('empty.txt'), findsOneWidget);
    expect(find.textContaining(r'D:\bad：无法访问'), findsOneWidget);
    expect(find.text('a.txt'), findsOneWidget);
    expect(find.text('文档 2'), findsOneWidget);

    // Remove a document.
    await tester.tap(
      find.descendant(
        of: find.byKey(const ValueKey('knowledge-doc-doc-a')),
        matching: find.byType(DshIcon),
      ),
    );
    await tester.pumpAndSettle();
    expect(lastCall(api, 'deleteDocument'), {'id': 'doc-a'});
    expect(find.text('a.txt'), findsNothing);

    // Disable search, then rename.
    await tester.tap(find.byKey(const Key('knowledge-enabled')));
    await tester.pumpAndSettle();
    expect(lastCall(api, 'update'), {'id': 'kb-1', 'enabled': false});
    expect(find.text('已停用'), findsOneWidget);
    await tester.enterText(
      find.descendant(
        of: find.byKey(const Key('knowledge-name')),
        matching: find.byType(EditableText),
      ),
      '运维与发布',
    );
    await tester.pump();
    expect(find.text('有未保存的修改'), findsOneWidget);
    await tester.tap(find.byKey(const Key('knowledge-save')));
    await tester.pumpAndSettle();
    expect(lastCall(api, 'update')['name'], '运维与发布');
    expect(find.text('有未保存的修改'), findsNothing);

    // Try a search scoped to this base.
    await tester.tap(find.byKey(const ValueKey('knowledge-tab-search')));
    await tester.pumpAndSettle();
    expect(find.textContaining('知识库已停用'), findsOneWidget);
    await tester.enterText(
      find.descendant(
        of: find.byKey(const Key('knowledge-query')),
        matching: find.byType(EditableText),
      ),
      '怎么回滚',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('knowledge-search')));
    await tester.pumpAndSettle();
    expect(lastCall(api, 'search'), {
      'query': '怎么回滚',
      'baseIds': ['kb-1'],
      'limit': 10,
    });
    expect(find.text('手册.pdf  #3'), findsOneWidget);
    expect(find.text('…先停止服务，再恢复上一版本…'), findsOneWidget);

    // Delete after confirmation.
    await tester.tap(find.byKey(const Key('knowledge-delete')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('删除').last);
    await tester.pumpAndSettle();
    expect(lastCall(api, 'delete'), {'id': 'kb-1'});
    expect(find.byKey(const Key('knowledge-detail')), findsNothing);
    expect(find.textContaining('还没有知识库'), findsOneWidget);
  });

  testWidgets('a duplicate name keeps the dialog open with the error', (
    tester,
  ) async {
    final api = FakeKnowledgeApi()
      ..bases.add({
        'id': 'kb-1',
        'name': '手册',
        'description': '',
        'enabled': true,
        'updatedAt': '1',
      })
      ..documents['kb-1'] = [];
    await pumpPage(tester, api);
    expect(find.byKey(const Key('knowledge-detail')), findsOneWidget);
    await tester.tap(find.byKey(const Key('knowledge-create')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.descendant(
        of: find.byKey(const Key('knowledge-create-name')),
        matching: find.byType(EditableText),
      ),
      '手册',
    );
    await tester.pump();
    await tester.tap(find.byKey(const Key('knowledge-create-submit')));
    await tester.pumpAndSettle();
    expect(find.text('已有同名知识库'), findsOneWidget);
    expect(find.byKey(const Key('knowledge-create-submit')), findsOneWidget);
  });
}
