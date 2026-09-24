import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class SettingsClient extends DshClient {
  SettingsClient() : super('http://127.0.0.1');
  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async => {
    'namespaces': [
      for (final entry in {
        'agent-presets': {'default': 'blank'},
        'permission': {'defaultPreset': 'workspace-write'},
        'ui-conversation': {
          'busyEnter': 'queue',
          'composerTips': 'on',
          'hintDisplay': 'both',
        },
        'locale': <String, Object>{},
      }.entries)
        {'ns': entry.key, 'revision': 1, 'value': entry.value},
    ],
  };
}

class SettingsController extends DesktopController {
  SettingsController() : super(DesktopPreferences());
  final api = SettingsClient();
  @override
  DshClient get client => api;
  @override
  Future<void> refreshSessions() async {}
}

void main() {
  testWidgets('settings receives Escape before any form field has focus', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1280, 900));
    final c = SettingsController();
    await tester.pumpWidget(
      ShadApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () => showDialog<void>(
              context: context,
              barrierDismissible: false,
              builder: (_) => SettingsShell(controller: c),
            ),
            child: const Text('打开设置'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('打开设置'));
    await tester.pumpAndSettle();
    expect(find.byType(SettingsShell), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byType(SettingsShell), findsNothing);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await c.api.close();
    await tester.binding.setSurfaceSize(null);
  });
  testWidgets(
    'general settings use named preset choices in web order, not an ID text box',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1400, 1000));
      final c = SettingsController()
        ..presets = [
          {'id': 'blank', 'name': '空白模式'},
          {'id': 'code', 'name': '代码模式'},
        ];
      await tester.pumpWidget(ShadApp(home: SettingsShell(controller: c)));
      await tester.pumpAndSettle();
      expect(find.byType(TextField), findsNothing);
      expect(find.text('空白模式'), findsOneWidget);
      expect(find.text('中文'), findsOneWidget);
      expect(find.text('preference'), findsNothing);
      expect(
        tester.getTopLeft(find.text('权限')).dy,
        lessThan(tester.getTopLeft(find.text('语言')).dy),
      );
      expect(
        tester.getTopLeft(find.text('语言')).dy,
        lessThan(tester.getTopLeft(find.text('外观')).dy),
      );
      await tester.tap(find.text('空白模式'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('代码模式'));
      await tester.pumpAndSettle();
      expect(find.text('代码模式'), findsOneWidget);
      expect(find.text('有未保存的修改'), findsOneWidget);
      await tester.scrollUntilVisible(
        find.text('输入提示'),
        100,
        scrollable: find.byType(Scrollable).last,
      );
      expect(
        tester.getTopLeft(find.text('回复提示显示')).dy,
        lessThan(tester.getTopLeft(find.text('输入提示')).dy),
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await c.api.close();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets(
    'archive matches cards and text actions without a fabricated search control',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1400, 900));
      final c = SettingsController()
        ..archivedSessions = [
          {
            'sessionId': 'archived',
            'title': '组件审查',
            'cwd': 'C:/project',
            'updatedAt': 1700000000000,
          },
        ];
      await tester.pumpWidget(
        ShadApp(
          home: SettingsShell(controller: c, initialPage: 'archive'),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('归档会话'), findsOneWidget);
      expect(find.text('恢复'), findsOneWidget);
      expect(find.text('删除'), findsOneWidget);
      expect(find.byType(TextField), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await c.api.close();
      await tester.binding.setSurfaceSize(null);
    },
  );
}
