import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/design/loading.dart';
import 'package:dsh_desktop/design/rich_content.dart';
import 'package:dsh_desktop/features/knowledge/knowledge_page.dart';
import 'package:dsh_desktop/features/schedule/schedule_page.dart';
import 'package:dsh_desktop/features/settings/models_page.dart';
import 'package:dsh_desktop/features/settings/resource_page.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';
import 'package:dsh_desktop/l10n/zh.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import 'controller_test.dart' show MemoryPreferences;
import 'knowledge_page_test.dart' show FakeKnowledgeApi;
import 'schedule_page_test.dart' show FakeScheduleApi;

class LoadingApi extends DshClient {
  LoadingApi() : super('http://127.0.0.1:1');
  final pending = <String, Completer<Json>>{};
  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) {
    final previous = pending[method];
    if (previous == null || previous.isCompleted) {
      pending[method] = Completer<Json>();
    }
    return pending[method]!.future;
  }
}

class LoadingController extends DesktopController {
  LoadingController(this.api) : super(MemoryPreferences());
  final DshClient api;
  bool firstList = false;
  @override
  DshClient get client => api;
  @override
  bool get connected => true;
  @override
  bool get loadingSessions => firstList;
}

class SlowKnowledgeApi extends FakeKnowledgeApi {
  final first = Completer<Json>();
  @override
  Future<Json> call(
    String operation, [
    Json body = const {},
    RequestScope? scope,
  ]) => operation == 'catalog'
      ? first.future
      : super.call(operation, body, scope);
}

class SlowScheduleApi extends FakeScheduleApi {
  SlowScheduleApi() : super([]);
  final first = Completer<Json>();
  @override
  Future<Json> call(
    String operation, [
    Json body = const {},
    RequestScope? scope,
  ]) => operation == 'catalog'
      ? first.future
      : super.call(operation, body, scope);
}

void main() {
  testWidgets(
    'skeleton announces loading once and does not steal focus or animate',
    (tester) async {
      final before = FocusNode(), after = FocusNode();
      addTearDown(before.dispose);
      addTearDown(after.dispose);
      final semantics = tester.ensureSemantics();
      try {
        await tester.pumpWidget(
          MaterialApp(
            home: MediaQuery(
              data: const MediaQueryData(
                disableAnimations: true,
                textScaler: TextScaler.linear(2),
              ),
              child: Scaffold(
                body: Column(
                  children: [
                    TextButton(
                      focusNode: before,
                      onPressed: () {},
                      child: const Text('before'),
                    ),
                    const Expanded(
                      child: DshListSkeleton(label: 'Loading fixture', rows: 4),
                    ),
                    TextButton(
                      focusNode: after,
                      onPressed: () {},
                      child: const Text('after'),
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(
          tester.getSemantics(find.byType(DshListSkeleton)).label,
          'Loading fixture',
        );
        before.requestFocus();
        await tester.pump();
        await tester.sendKeyEvent(LogicalKeyboardKey.tab);
        await tester.pumpAndSettle();
        expect(after.hasFocus, isTrue);
        expect(tester.binding.transientCallbackCount, 0);
        expect(tester.takeException(), isNull);
      } finally {
        semantics.dispose();
      }
    },
  );

  testWidgets('session skeleton only replaces an empty initial list', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1280, 800));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final api = LoadingApi();
    final c = LoadingController(api)..firstList = true;
    await tester.pumpWidget(DesktopApp(controller: c));
    await tester.pumpAndSettle();
    expect(find.byType(DshListSkeleton), findsOneWidget);
    c.sessions = [
      SessionSummary.fromJson({
        'sessionId': 'loaded',
        'displayTitle': 'Loaded session',
      }),
    ];
    c.firstList = false;
    c.emit();
    await tester.pumpAndSettle();
    expect(find.byType(DshListSkeleton), findsNothing);
    expect(find.byKey(const ValueKey('session-loaded')), findsOneWidget);
    c.firstList = true;
    c.emit();
    await tester.pumpAndSettle();
    expect(find.byType(DshListSkeleton), findsNothing);
    expect(find.byKey(const ValueKey('session-loaded')), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await api.close();
  });

  testWidgets(
    'models and capabilities show a skeleton while their first response is pending',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1100, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final api = LoadingApi(), c = LoadingController(LoadingApi());
      final modelApi = c.api as LoadingApi;
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: ModelsPage(controller: c, onSettingsChanged: () async {}),
          ),
        ),
      );
      await tester.pump();
      expect(find.byType(DshListSkeleton), findsOneWidget);
      modelApi.pending['llm.providers']!.complete({'providers': <Json>[]});
      modelApi.pending['settings.describe']!.complete({'namespaces': <Json>[]});
      await tester.pumpAndSettle();
      expect(find.byType(DshListSkeleton), findsNothing);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SettingsResourcePage(controller: c, page: 'skills'),
          ),
        ),
      );
      await tester.pump();
      expect(find.byType(DshListSkeleton), findsOneWidget);
      modelApi.pending['capabilities.list']!.complete({
        'revision': 1,
        'skills': [
          {'name': 'Existing skill', 'enabled': true},
        ],
        'servers': <Json>[],
      });
      await tester.pumpAndSettle();
      expect(find.byType(DshListSkeleton), findsNothing);
      expect(find.text('Existing skill'), findsOneWidget);
      await tester.tap(find.byTooltip(DshSettingsZh.refresh).first);
      await tester.pump();
      expect(find.byType(DshListSkeleton), findsNothing);
      expect(find.text('Existing skill'), findsOneWidget);
      modelApi.pending['capabilities.list']!.complete({
        'revision': 2,
        'skills': [
          {'name': 'Updated skill', 'enabled': true},
        ],
        'servers': <Json>[],
      });
      await tester.pumpAndSettle();
      expect(find.text('Updated skill'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await c.api.close();
      await api.close();
    },
  );

  testWidgets(
    'knowledge and scheduled tasks distinguish first loading from an empty result',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1100, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final c = LoadingController(LoadingApi());
      final knowledge = SlowKnowledgeApi(), schedule = SlowScheduleApi();
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: KnowledgePage(controller: c, onClose: () {}, api: knowledge),
          ),
        ),
      );
      await tester.pump();
      expect(find.byType(DshListSkeleton), findsOneWidget);
      knowledge.first.complete({'bases': <Json>[], 'extensions': <String>[]});
      await tester.pumpAndSettle();
      expect(find.byType(DshListSkeleton), findsNothing);
      expect(find.text(DshKnowledgeZh.empty), findsOneWidget);
      await tester.pumpWidget(
        ShadApp(
          home: Scaffold(
            body: SchedulePage(
              controller: c,
              onClose: () {},
              onOpenSession: (_) {},
              api: schedule,
            ),
          ),
        ),
      );
      await tester.pump();
      expect(find.byType(DshListSkeleton), findsOneWidget);
      schedule.first.complete({
        'tasks': <Json>[],
        'revision': 1,
        'hostTimeZone': 'Asia/Shanghai',
      });
      await tester.pumpAndSettle();
      expect(find.byType(DshListSkeleton), findsNothing);
      expect(find.text(DshScheduleZh.empty), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await c.api.close();
      await knowledge.client.close();
      await schedule.client.close();
    },
  );

  testWidgets(
    'code wrap changes layout, keeps the raw copy and survives text scaling',
    (tester) async {
      final code =
          '${'final variable = "中文 content"; ' * 8}\r\nprint(variable);';
      String? copied;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied = (call.arguments as Map)['text'] as String;
          }
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      Widget atScale(double scale) => ShadApp(
        home: MediaQuery(
          data: MediaQueryData(textScaler: TextScaler.linear(scale)),
          child: Scaffold(
            body: Center(
              child: SingleChildScrollView(
                child: SizedBox(
                  width: 260,
                  child: NativeCodeBlock(code: code, language: 'dart'),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpWidget(atScale(1));
      await tester.pumpAndSettle();
      final initial = tester.getSize(find.text(code));
      await tester.tap(find.byTooltip(DshConversationZh.wrapLines));
      await tester.pumpAndSettle();
      final wrapped = tester.getSize(find.text(code));
      expect(wrapped.height, greaterThan(initial.height));
      expect(wrapped.width, lessThanOrEqualTo(260));
      await tester.tap(find.byTooltip(DshConversationZh.copyCode));
      await tester.pumpAndSettle();
      expect(copied, code);
      await tester.pumpWidget(atScale(2));
      await tester.pumpAndSettle();
      expect(
        tester.getSize(find.text(code)).height,
        greaterThan(wrapped.height),
      );
      expect(tester.takeException(), isNull);
    },
  );
}
