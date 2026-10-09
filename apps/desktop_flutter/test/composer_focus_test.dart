import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show FakeClient, MemoryPreferences;

void main() {
  final prompt = find.byKey(const Key('prompt-input'));

  Future<void> clickPrompt(WidgetTester tester, {TestGesture? mouse}) async {
    final input = tester.widget<TextField>(prompt);
    input.focusNode!.unfocus();
    await tester.pump();
    if (mouse == null) {
      final pointer = await tester.startGesture(
        tester.getCenter(prompt),
        kind: PointerDeviceKind.mouse,
      );
      await pointer.up();
    } else {
      await mouse.moveTo(tester.getCenter(prompt));
      await mouse.down(tester.getCenter(prompt));
      await mouse.up();
    }
    await tester.pump();
    expect(input.focusNode!.hasPrimaryFocus, isTrue);
    expect(input.focusNode!.context, isNotNull);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
    expect(tester.takeException(), isNull);
  }

  testWidgets('mouse can focus composer after repeated hero transitions', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences());
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    });
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    await clickPrompt(tester);

    for (var index = 0; index < 4; index++) {
      c.selectedId = 'existing-$index';
      c.transcript = [
        TranscriptItem(id: 'u-$index', kind: 'user', text: '已有会话'),
      ];
      c.emit();
      await tester.pumpAndSettle();
      await clickPrompt(tester);
      c.newConversation();
      await tester.pumpAndSettle();
      await clickPrompt(tester);
      expect(find.byType(EditableText), findsOneWidget);
    }
  });

  testWidgets('an idle composer focus request schedules its own frame', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences());
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    });
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    final focus = tester.widget<TextField>(prompt).focusNode!;
    focus.unfocus();
    await tester.pumpAndSettle();
    expect(tester.binding.hasScheduledFrame, isFalse);
    expect(focus.hasPrimaryFocus, isFalse);
    c.composerFocus.value++;
    expect(tester.binding.hasScheduledFrame, isTrue);
    await tester.pump();
    await tester.pump();
    expect(focus.hasPrimaryFocus, isTrue);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'new conversation button restores composer focus after creation',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1280, 800));
      final api = FakeClient();
      final c = DesktopController(
        MemoryPreferences(),
        clientFactory: (_) => api,
      );
      await c.connect('http://127.0.0.1');
      await tester.pump();
      c.workspaces = [
        {'workspaceId': 'w', 'path': r'E:\project', 'title': 'project'},
      ];
      c.workspaceId = 'w';
      c.selectedId = 'existing';
      c.transcript = [TranscriptItem(id: 'u', kind: 'user', text: '已有会话')];
      final creating = Completer<Json>();
      api.handleCall = (method, payload) async => method == 'session.create'
          ? creating.future
          : {'items': c.workspaces, 'archivedSessionIds': []};
      addTearDown(() async {
        await tester.pumpWidget(const SizedBox());
        c.dispose();
        await tester.binding.setSurfaceSize(null);
      });
      await tester.pumpWidget(DesktopApp(controller: c));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('new-task')));
      await tester.pumpAndSettle();
      expect(
        tester.widget<TextField>(prompt).focusNode!.hasPrimaryFocus,
        isTrue,
      );
      await clickPrompt(tester);
      api.liveSessions = [
        SessionSummary.fromJson({
          'sessionId': 'created',
          'cwd': r'E:\project',
          'blank': true,
        }),
      ];
      api.histories['created'] = Future.value(
        HistoryPage.fromJson({'events': []}),
      );
      creating.complete({'sessionId': 'created'});
      await tester.pumpAndSettle();
      expect(c.selectedId, 'created');
      await clickPrompt(tester);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.windows),
  );

  for (final scenario in ['immediate', 'history-loading', 'rapid-clicks']) {
    testWidgets(
      'Windows hovered new conversation remains editable: $scenario',
      (tester) async {
        await tester.binding.setSurfaceSize(const Size(1280, 800));
        final api = FakeClient();
        final c = DesktopController(
          MemoryPreferences(),
          clientFactory: (_) => api,
        );
        await c.connect('http://127.0.0.1');
        await tester.pump();
        c.workspaces = [
          {'workspaceId': 'w', 'path': r'E:\project', 'title': 'project'},
        ];
        c.workspaceId = 'w';
        c.selectedId = 'existing';
        c.sessions = [
          SessionSummary.fromJson({
            'sessionId': 'existing',
            'cwd': r'E:\project',
          }),
        ];
        c.transcript = [TranscriptItem(id: 'u', kind: 'user', text: '已有会话')];
        final creating = Completer<Json>();
        final history = Completer<HistoryPage>();
        var creates = 0;
        api.handleCall = (method, payload) async {
          if (method != 'session.create') {
            return {'items': c.workspaces, 'archivedSessionIds': []};
          }
          creates++;
          api.liveSessions = [
            SessionSummary.fromJson({
              'sessionId': 'created',
              'cwd': r'E:\project',
              'blank': true,
            }),
          ];
          api.histories['created'] = scenario == 'history-loading'
              ? history.future
              : Future.value(HistoryPage.fromJson({'events': []}));
          return scenario == 'rapid-clicks'
              ? creating.future
              : {'sessionId': 'created'};
        };
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        addTearDown(() async {
          if (!creating.isCompleted) {
            creating.complete({'sessionId': 'created'});
          }
          if (!history.isCompleted) {
            history.complete(HistoryPage.fromJson({'events': []}));
          }
          await mouse.removePointer();
          await tester.pumpWidget(const SizedBox());
          c.dispose();
          await tester.binding.setSurfaceSize(null);
        });
        await tester.pumpWidget(DesktopApp(controller: c));
        await tester.pumpAndSettle();
        final newButton = find.byKey(const Key('new-task'));
        final tooltip = tester.widget<Tooltip>(
          find.ancestor(of: newButton, matching: find.byType(Tooltip)).first,
        );
        await mouse.addPointer(location: Offset.zero);
        await mouse.moveTo(tester.getCenter(newButton));
        await tester.pump(const Duration(milliseconds: 700));
        await tester.pump(const Duration(milliseconds: 200));
        expect(find.text(tooltip.message!), findsOneWidget);
        await mouse.down(tester.getCenter(newButton));
        await mouse.up();
        for (var frame = 0; frame < 5; frame++) {
          await tester.pump(const Duration(milliseconds: 50));
        }
        await clickPrompt(tester, mouse: mouse);
        if (scenario == 'rapid-clicks') {
          for (var click = 0; click < 3; click++) {
            await mouse.moveTo(tester.getCenter(newButton));
            await mouse.down(tester.getCenter(newButton));
            await mouse.up();
            await tester.pump();
          }
          expect(creates, 1);
          await clickPrompt(tester, mouse: mouse);
          creating.complete({'sessionId': 'created'});
        }
        if (scenario == 'history-loading') {
          expect(c.selectedId, 'created');
          expect(c.loading, isTrue);
          history.complete(HistoryPage.fromJson({'events': []}));
        }
        await tester.pumpAndSettle();
        expect(c.selectedId, 'created');
        expect(c.loading, isFalse);
        expect(creates, 1);
        await clickPrompt(tester, mouse: mouse);
        expect(find.byType(EditableText), findsOneWidget);
      },
      variant: TargetPlatformVariant.only(TargetPlatform.windows),
    );
  }

  for (final panel in ['plugins', 'knowledge']) {
    testWidgets(
      'Windows new conversation focuses the editor after closing $panel',
      (tester) async {
        await tester.binding.setSurfaceSize(const Size(1280, 800));
        final api = FakeClient();
        final c = DesktopController(
          MemoryPreferences(),
          clientFactory: (_) => api,
        );
        await c.connect('http://127.0.0.1');
        await tester.pump();
        c.workspaces = [
          {'workspaceId': 'w', 'path': r'E:\project', 'title': 'project'},
        ];
        c.workspaceId = 'w';
        final creating = Completer<Json>();
        api.handleCall = (method, payload) async => method == 'session.create'
            ? creating.future
            : {'items': c.workspaces, 'archivedSessionIds': []};
        final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
        addTearDown(() async {
          if (!creating.isCompleted) {
            creating.complete({'sessionId': 'created'});
          }
          await mouse.removePointer();
          await tester.pumpWidget(const SizedBox());
          c.dispose();
          await tester.binding.setSurfaceSize(null);
        });
        await tester.pumpWidget(DesktopApp(controller: c));
        await tester.pumpAndSettle();
        await tester.tap(find.byKey(Key('open-$panel')));
        await tester.pumpAndSettle();
        expect(prompt, findsNothing);
        final newButton = find.byKey(const Key('new-task'));
        await mouse.addPointer(location: Offset.zero);
        await mouse.moveTo(tester.getCenter(newButton));
        await mouse.down(tester.getCenter(newButton));
        await mouse.up();
        await tester.pumpAndSettle();
        expect(prompt, findsOneWidget);
        expect(
          tester.widget<TextField>(prompt).focusNode!.hasPrimaryFocus,
          isTrue,
        );
        await clickPrompt(tester, mouse: mouse);
        api.liveSessions = [
          SessionSummary.fromJson({
            'sessionId': 'created',
            'cwd': r'E:\project',
            'blank': true,
          }),
        ];
        api.histories['created'] = Future.value(
          HistoryPage.fromJson({'events': []}),
        );
        creating.complete({'sessionId': 'created'});
        await tester.pumpAndSettle();
        expect(c.selectedId, 'created');
        await clickPrompt(tester, mouse: mouse);
      },
      variant: TargetPlatformVariant.only(TargetPlatform.windows),
    );
  }

  testWidgets('input request waits for a hidden composer to become focusable', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences());
    var hidden = true;
    late StateSetter setParent;
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    });
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              setParent = setState;
              return Offstage(
                offstage: hidden,
                child: ExcludeFocus(
                  excluding: hidden,
                  child: Conversation(controller: c),
                ),
              );
            },
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    setParent(() => hidden = false);
    c.composerFocus.value++;
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(prompt).focusNode!.hasPrimaryFocus, isTrue);
    await clickPrompt(tester);
  });

  testWidgets('loading transitions preserve active text input connection', (
    tester,
  ) async {
    final c = DesktopController(MemoryPreferences());
    addTearDown(() async {
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    });
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: c)),
      ),
    );
    await tester.pumpAndSettle();
    await tester.enterText(prompt, '输入草稿');
    final input = tester.widget<TextField>(prompt);
    input.controller!.value = const TextEditingValue(
      text: '输入草稿',
      selection: TextSelection.collapsed(offset: 3),
      composing: TextRange(start: 2, end: 4),
    );
    for (final loading in [true, false, true, false]) {
      c.loading = loading;
      c.emit();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 200));
      expect(input.focusNode!.hasPrimaryFocus, isTrue);
      expect(tester.testTextInput.isRegistered, isTrue);
      expect(input.controller!.selection.baseOffset, 3);
      expect(
        input.controller!.value.composing,
        const TextRange(start: 2, end: 4),
      );
      expect(tester.takeException(), isNull);
    }
  });
}
