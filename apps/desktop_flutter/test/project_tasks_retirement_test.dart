import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:flutter/material.dart' hide TabController;
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'workbench_tabs_test.dart' show TabApi, TabController;

void main() {
  testWidgets(
    'project task retirement preserves the other five conversation tabs',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1440, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final api = TabApi();
      final controller = TabController(api)..menuSettings = {'tasks': true};
      await tester.pumpWidget(DesktopApp(controller: controller));
      await tester.pumpAndSettle();
      for (final key in [
        'conversation',
        'trajectory',
        'artifacts',
        'code-graph',
        'context',
      ]) {
        expect(find.byKey(ValueKey('conversation-view-$key')), findsOneWidget);
      }
      expect(find.text('项目任务'), findsNothing);
      expect(
        find.byKey(const ValueKey('conversation-view-project-tasks')),
        findsNothing,
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
      await api.close();
    },
  );

  testWidgets(
    'a stale project task tab restores files and cannot be reopened',
    (tester) async {
      final api = TabApi();
      final controller = TabController(api);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: WorkbenchPanel(
              controller: controller,
              initialTab: 'project-tasks',
              onClose: () {},
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        api.requests.any((request) => request.operation == 'list'),
        isTrue,
      );
      await tester.tap(find.byKey(const Key('workbench-add-tab')));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('workbench-open-project-tasks')),
        findsNothing,
      );
      for (final key in [
        'files',
        'git',
        'terminal',
        'tasks',
        'team',
        'computer-use',
      ]) {
        expect(find.byKey(ValueKey('workbench-open-$key')), findsOneWidget);
      }
      expect(
        api.requests.every((request) => !request.operation.contains('tasks')),
        isTrue,
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
      await api.close();
    },
  );

  testWidgets(
    'legacy Host menu settings cannot recreate the retired project switch',
    (tester) async {
      final edits = <List<String>>[];
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: NamespaceForm(
              namespace: {
                'ns': 'mini-menu',
                'value': {
                  'tasks': true,
                  'trajectory': true,
                  'artifacts': true,
                  'code-graph': true,
                  'context': true,
                },
              },
              pending: const [],
              onChange: (path, _) => edits.add(path),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('settings-field-mini-menu-tasks')),
        findsNothing,
      );
      for (final key in ['trajectory', 'artifacts', 'code-graph', 'context']) {
        expect(
          find.byKey(ValueKey('settings-field-mini-menu-$key')),
          findsOneWidget,
        );
      }
      expect(edits, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );
}
