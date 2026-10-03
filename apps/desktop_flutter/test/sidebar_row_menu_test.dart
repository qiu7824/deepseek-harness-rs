import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'sidebar_interaction_repair_test.dart' show mountRepair;

void main() {
  testWidgets('row menus are reachable without a right click', (tester) async {
    await mountRepair(tester);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);

    final session = find.byKey(const ValueKey('session-a-session'));
    final sessionMore = find.byKey(const ValueKey('session-more-a-session'));
    expect(sessionMore, findsNothing);
    await mouse.moveTo(tester.getCenter(session));
    await tester.pumpAndSettle();
    expect(sessionMore, findsOneWidget);
    await tester.tap(sessionMore);
    await tester.pumpAndSettle();
    for (final item in ['重命名', '创建分支', '复制会话 ID', '归档']) {
      expect(find.text(item), findsOneWidget);
    }
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.text('创建分支'), findsNothing);

    await mouse.moveTo(tester.getCenter(find.text('工作区 A').first));
    await tester.pumpAndSettle();
    expect(sessionMore, findsNothing);
    final workspaceMore = find.byKey(const ValueKey('workspace-more-a'));
    expect(workspaceMore, findsOneWidget);
    await tester.tap(workspaceMore);
    await tester.pumpAndSettle();
    expect(find.text('重命名工作区'), findsOneWidget);
    final delete = tester.widget<Text>(find.text('删除工作区'));
    final rename = tester.widget<Text>(find.text('重命名工作区'));
    expect(delete.style?.color, isNot(rename.style?.color));
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });
}
