import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/settings/task_models_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class TaskApi extends DshClient {
  TaskApi({this.unsupported = false}) : super('http://127.0.0.1:9');
  final bool unsupported;
  final saves = <Json>[];
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    if (path.endsWith('/save')) {
      saves.add(body!);
      expect(mutation, isTrue);
    }
    return {
      'revision': saves.length + 4,
      'routes': {
        'image': {'provider': 'subscription', 'model': 'image-model'},
      },
      'providers': [
        {
          'id': 'subscription',
          'name': '订阅连接',
          'nativeTools': {
            'image': unsupported ? 'unsupported' : 'unknown',
            'search': 'unknown',
          },
        },
      ],
    };
  }
}

void main() {
  testWidgets('unsupported image routes disable save and show the reason', (
    tester,
  ) async {
    final api = TaskApi(unsupported: true);
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: TaskModelsPage(api: api)),
      ),
    );
    await tester.pumpAndSettle();
    await tester.scrollUntilVisible(
      find.text('保存任务模型'),
      300,
      scrollable: find.byType(Scrollable).first,
    );
    final button = tester.widget<DshButton>(
      find.ancestor(of: find.text('保存任务模型'), matching: find.byType(DshButton)),
    );
    expect(button.onPressed, isNull);
    expect(api.saves, isEmpty);
  });
  testWidgets(
    'unknown capabilities are not asserted compatible and repeated saves refresh revisions',
    (tester) async {
      final api = TaskApi();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: TaskModelsPage(api: api)),
        ),
      );
      await tester.pumpAndSettle();
      await tester.scrollUntilVisible(
        find.text('保存任务模型'),
        300,
        scrollable: find.byType(Scrollable).first,
      );
      await tester.tap(find.text('保存任务模型'));
      await tester.pumpAndSettle();
      await tester.ensureVisible(find.text('保存任务模型'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('保存任务模型'));
      await tester.pumpAndSettle();
      expect(api.saves.map((s) => s['revision']), [4, 5]);
      expect(
        object(object(api.saves.first['routes'])['image'])['model'],
        'image-model',
      );
      expect(tester.takeException(), isNull);
    },
  );
}
