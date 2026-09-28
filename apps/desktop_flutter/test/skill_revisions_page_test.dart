import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/settings/skill_revisions_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class SkillApi extends DshClient {
  SkillApi() : super('http://127.0.0.1:9');
  int revision = 4;
  bool validated = false, active = false, withdrawn = false;
  final writes = <Json>[];
  Json candidate() => {
    'id': 'v1',
    'name': '可验证流程',
    'description': '检验流程',
    'project': 'D:/fixture',
    'contentHash': 'hash',
    'validation': validated ? {} : null,
    'active': active,
    'withdrawn': withdrawn,
  };
  Json snapshot() => {
    'revision': revision,
    'enabled': true,
    'candidates': [candidate()],
  };
  @override
  Future<dynamic> callValue(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    if (method.endsWith('List')) return snapshot();
    if (method.endsWith('Read')) {
      return {
        ...candidate(),
        'ownerSessionId': 'owner',
        'content': '技能正文',
        'samples': <Json>[],
      };
    }
    expect(mutation, isTrue);
    expect(payload['expectedRevision'], revision);
    writes.add({'method': method, ...payload});
    revision++;
    if (method.endsWith('Validate')) {
      validated = true;
      return {
        'state': snapshot(),
        'evidence': {'allMatched': true},
      };
    }
    if (method.endsWith('Activate') || method.endsWith('Restore')) {
      active = true;
      withdrawn = false;
    }
    if (method.endsWith('Withdraw')) {
      active = false;
      withdrawn = true;
    }
    return snapshot();
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    expect(path, '/__dsh-task-execution');
    expect(body!['summaryOnly'], isTrue);
    return {
      'tasks': [
        for (final row in [
          ('positive', 'success', 'hash'),
          ('negative', 'failure', 'hash'),
          ('foreign', 'success', 'other'),
        ])
          {
            'taskId': row.$1,
            'revision': 31,
            'state': 'completed',
            'spec': {
              'objective': row.$1,
              'validationSubject': {
                'kind': 'skill',
                'identity': row.$3,
                'expectedOutcome': row.$2,
              },
            },
          },
      ],
    };
  }
}

void main() {
  testWidgets(
    'native skill workflow requires both verified sample directions and keeps revisioned controls',
    (tester) async {
      tester.view.physicalSize = const Size(1200, 1800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final api = SkillApi();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(body: SkillRevisionsPage(api: api)),
        ),
      );
      await tester.pumpAndSettle();
      DshButton button(String name) => tester.widget(
        find.ancestor(of: find.text(name), matching: find.byType(DshButton)),
      );
      expect(button('验证并启用').onPressed, isNull);
      await tester.tap(find.text('查看与验证'));
      await tester.pumpAndSettle();
      expect(find.textContaining('foreign'), findsNothing);
      expect(button('核验所选样本').onPressed, isNull);
      await tester.tap(find.text('正向：positive'));
      await tester.pump();
      expect(button('核验所选样本').onPressed, isNull);
      await tester.tap(find.text('反向：negative'));
      await tester.pump();
      await tester.tap(find.text('核验所选样本'));
      await tester.pumpAndSettle();
      expect(
        objects(api.writes.single['samples'])
            .map((row) => row['expectedSuccess']),
        [true, false],
      );
      await tester.tap(find.text('验证并启用'));
      await tester.pumpAndSettle();
      expect(api.active, isTrue);
      await tester.tap(find.text('撤回版本'));
      await tester.pumpAndSettle();
      expect(api.withdrawn, isTrue);
      await tester.tap(find.text('复核并恢复此版本'));
      await tester.pumpAndSettle();
      expect(api.active, isTrue);
      expect(api.writes.map((row) => row['expectedRevision']), [4, 5, 6, 7]);
      expect(tester.takeException(), isNull);
    },
  );
}
