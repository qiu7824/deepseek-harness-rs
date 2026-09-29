import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/features/settings/skill_revisions_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class SkillApi extends DshClient {
  SkillApi() : super('http://127.0.0.1:9');
  int revision = 4;
  bool active = false, withdrawn = false, enabled = true;
  final writes = <Json>[];
  Json candidate() => {
    'id': 'v1',
    'name': 'manual-workflow',
    'description': '项目流程',
    'project': 'D:/fixture',
    'contentHash': 'hash',
    'activationMode': 'manual',
    'active': active,
    'withdrawn': withdrawn,
  };
  Json snapshot() => {
    'revision': revision,
    'enabled': enabled,
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
    if (method.endsWith('Read')) return {...candidate(), 'content': '技能正文'};
    expect(mutation, isTrue);
    expect(payload['expectedRevision'], revision);
    expect(payload.containsKey('samples'), isFalse);
    expect(method.endsWith('Validate'), isFalse);
    writes.add({'method': method, ...payload});
    revision++;
    if (method.endsWith('Create')) {
      return {
        'candidate': {...candidate(), 'id': 'v2'},
        'state': snapshot(),
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
    if (method.endsWith('Toggle')) enabled = payload['enabled'] == true;
    return snapshot();
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async => throw StateError('Unexpected control endpoint: $path');
}

void main() {
  testWidgets(
    'manual skill revisions preserve content and require explicit activation',
    (tester) async {
      tester.view.physicalSize = const Size(1400, 2600);
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
      expect(button('启用版本').onPressed, isNotNull);
      await tester.tap(find.text('查看版本'));
      await tester.pumpAndSettle();
      expect(find.textContaining('验收'), findsNothing);
      await tester.ensureVisible(find.text('编辑为新版本'));
      await tester.tap(find.text('编辑为新版本'));
      await tester.pumpAndSettle();
      final fields = find.byType(TextField);
      expect(fields, findsNWidgets(4));
      expect(
        tester.widget<TextField>(fields.at(2)).controller!.text,
        'D:/fixture',
      );
      await tester.enterText(fields.at(3), '技能正文：保留换行\n新增内容。');
      await tester.ensureVisible(find.text('保存新版本'));
      await tester.tap(find.text('保存新版本'));
      await tester.pumpAndSettle();
      expect(api.writes.single['content'], '技能正文：保留换行\n新增内容。');
      expect(api.active, isFalse);
      await tester.ensureVisible(find.text('启用版本'));
      await tester.tap(find.text('启用版本'));
      await tester.pumpAndSettle();
      expect(api.active, isTrue);
      await tester.tap(find.text('撤回版本'));
      await tester.pumpAndSettle();
      expect(api.withdrawn, isTrue);
      await tester.tap(find.text('恢复版本'));
      await tester.pumpAndSettle();
      expect(api.active, isTrue);
      expect(api.writes.map((row) => row['expectedRevision']), [4, 5, 6, 7]);
      await tester.tap(find.byType(Switch));
      await tester.pumpAndSettle();
      expect(api.enabled, isFalse);
      expect(button('创建技能版本').onPressed, isNull);
      expect(tester.takeException(), isNull);
    },
  );
}
