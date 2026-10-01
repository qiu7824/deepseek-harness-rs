import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/primitives.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;

void main() {
  testWidgets(
    'workspace callback replacement is reflected without other state',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1144, 862));
      final c = DesktopController(MemoryPreferences());
      var first = 0, second = 0;
      final workspaceKey = GlobalKey();
      late StateSetter setParent;
      VoidCallback callback = () {
        first++;
      };
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: StatefulBuilder(
              builder: (context, setState) {
                setParent = setState;
                return Conversation(
                  controller: c,
                  workspaceAnchor: workspaceKey,
                  onSelectWorkspace: callback,
                );
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      tester.widget<DshButton>(find.byKey(workspaceKey)).onPressed!();
      expect(first, 1);
      setParent(() {
        callback = () {
          second++;
        };
      });
      await tester.pumpAndSettle();
      tester.widget<DshButton>(find.byKey(workspaceKey)).onPressed!();
      expect(second, 1);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );

  test(
    'moving selected session between workspaces invalidates conversation',
    () {
      final c = DesktopController(MemoryPreferences())..selectedId = 's';
      c.sessions = [
        SessionSummary.fromJson({'sessionId': 's', 'cwd': '/outside'}),
      ];
      c.workspaceId = 'a';
      c.workspaces = [
        {
          'workspaceId': 'a',
          'title': 'A',
          'path': '/a',
          'sessionIds': ['s'],
        },
        {
          'workspaceId': 'b',
          'title': 'B',
          'path': '/b',
          'sessionIds': <String>[],
        },
      ];
      c.emit();
      final revision = c.conversationChanges.value;
      c.workspaces = [
        {
          'workspaceId': 'a',
          'title': 'A',
          'path': '/a',
          'sessionIds': <String>[],
        },
        {
          'workspaceId': 'b',
          'title': 'B',
          'path': '/b',
          'sessionIds': ['s'],
        },
      ];
      expect(c.workspaceOf(c.selected!)!['workspaceId'], 'b');
      c.emit();
      expect(c.conversationChanges.value, greaterThan(revision));
      c.dispose();
    },
  );
}
