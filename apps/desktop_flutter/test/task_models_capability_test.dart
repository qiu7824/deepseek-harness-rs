import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/settings/task_models_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class TaskApi extends DshClient {
  TaskApi({this.unsupported = false, this.evidence = false})
    : super('http://127.0.0.1:9');
  final bool unsupported;
  final bool evidence;
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
          if (evidence)
            'nativeCapabilities': {
              'image': {
                'registered': true,
                'authorization': 'present',
                'state': 'unverified',
                'observations': [
                  {
                    'model': 'image-model',
                    'driverModel': 'driver',
                    'operation': 'generate',
                    'state': 'ready',
                    'expiresAt':
                        DateTime.now().millisecondsSinceEpoch ~/ 1000 + 3600,
                  },
                  {
                    'model': 'old-model',
                    'operation': 'edit',
                    'state': 'ready',
                    'expiresAt': 1,
                    'expired': true,
                  },
                ],
              },
            },
        },
      ],
    };
  }
}

void main() {
  testWidgets(
    'observed capabilities retain model scope and expire without making a paid probe',
    (tester) async {
      tester.view.physicalSize = const Size(1200, 1800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final api = TaskApi(evidence: true);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: TaskModelsPage(api: api)),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('工具已注册 · 凭据已配置 · 尚未验证'), findsOneWidget);
      expect(
        find.textContaining('image-model / generate · 驱动 driver · 已验证可用'),
        findsOneWidget,
      );
      expect(find.textContaining('old-model / edit · 尚未验证'), findsOneWidget);
      await tester.tap(find.text('刷新能力记录'));
      await tester.pumpAndSettle();
      expect(api.saves, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );
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
      find.text('保存辅助模型'),
      300,
      scrollable: find.byType(Scrollable).first,
    );
    final button = tester.widget<DshButton>(
      find.ancestor(of: find.text('保存辅助模型'), matching: find.byType(DshButton)),
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
        find.text('保存辅助模型'),
        300,
        scrollable: find.byType(Scrollable).first,
      );
      await Scrollable.ensureVisible(
        tester.element(
          find.ancestor(
            of: find.text('保存辅助模型'),
            matching: find.byType(DshButton),
          ),
        ),
        alignment: 0.5,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('保存辅助模型'));
      await tester.pumpAndSettle();
      await Scrollable.ensureVisible(
        tester.element(
          find.ancestor(
            of: find.text('保存辅助模型'),
            matching: find.byType(DshButton),
          ),
        ),
        alignment: 0.5,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('保存辅助模型'));
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
