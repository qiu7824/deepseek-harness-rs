import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/settings/plugin_page.dart';
import 'package:dsh_desktop/features/settings/settings_shell.dart';
import 'package:dsh_desktop/features/workbench/start_panel.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';
import 'package:dsh_desktop/l10n/statistics_zh.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'workbench_test.dart' show TestController;

class ProductVisualClient extends DshClient {
  ProductVisualClient() : super('http://127.0.0.1:1');
  final readPaths = <String>[];
  final readMethods = <String>[];
  final mutationAttempts = <String>[];
  final inventory = <Json>[
    for (final entry in [
      ('review', 'dsh-experimental-auto-review', false, null),
      ('clock', 'dsh-time-context', true, 'active'),
      ('schedule', 'dsh-schedule', true, 'active'),
      ('artifacts', 'dsh-artifacts', true, 'active'),
      ('context-jump', 'dsh-context-jump', true, 'active'),
      ('sidebar', 'dsh-better-sidebar', true, 'pending'),
      ('workbench', 'dsh-sidebar-workbench-suite', true, 'active'),
      ('voice', 'dsh-voice-input', false, null),
    ])
      {
        'entryId': entry.$1,
        'moduleName': entry.$2,
        'name': entry.$2,
        'enabled': entry.$3,
        'fiberPhase': entry.$4,
        if (entry.$1 == 'review') 'experimental': true,
      },
  ];

  @override
  Future<ModelCatalog> models(String id) async {
    readMethods.add('session.models');
    return ModelCatalog.fromJson(productVisualModelCatalog());
  }

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    if (mutation) {
      mutationAttempts.add(path);
      throw StateError('Visual fixtures are read-only');
    }
    readPaths.add(path);
    final route = Uri.parse(path).path;
    if (route == '/__dsh-schedule/catalog') {
      return {
        'tasks': [
          {
            'id': 'morning-check',
            'sessionId': 'startup-check',
            'title': '每日启动检查',
            'prompt': '检查桌面客户端启动日志，汇总异常。',
            'status': 'active',
            'rule': {
              'kind': 'daily',
              'time': '08:30',
              'timeZone': 'Asia/Shanghai',
            },
            'origin': 'user',
            'createdAt': '2026-09-01T00:00:00.000Z',
            'updatedAt': '2026-09-01T00:00:00.000Z',
            'nextRunAt': '2026-10-01T00:30:00.000Z',
            'historyCount': 0,
          },
        ],
        'revision': 1,
        'error': null,
        'hostTimeZone': 'Asia/Shanghai',
      };
    }
    if (route == '/__dsh-schedule/wait' && scope != null) {
      final pending = Completer<Json>();
      final unregister = scope.register(() {
        if (!pending.isCompleted) pending.complete({'revision': 1});
      });
      try {
        return await pending.future;
      } finally {
        unregister();
      }
    }
    return <String, dynamic>{};
  }

  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) async {
    if (mutation) {
      mutationAttempts.add(method);
      throw StateError('Visual fixtures are read-only');
    }
    readMethods.add(method);
    if (method == 'pluginInventory.list') return {'entries': inventory};
    return <String, dynamic>{};
  }
}

class ProductVisualController extends TestController {
  final api = ProductVisualClient();
  @override
  DshClient get client => api;

  @override
  Future<void> refreshModels() async {
    if (selectedId == null) return;
    catalog = await api.models(selectedId!);
    emit();
  }
}

Json productVisualModelCatalog() => {
  'routable': true,
  'current': {
    'provider': 'deepseek',
    'model': 'deepseek-chat',
    'reasoningEffort': 'high',
  },
  'groups': [
    {
      'id': 'deepseek',
      'name': 'DeepSeek',
      'models': [
        {
          'id': 'deepseek-chat',
          'name': 'DeepSeek Chat',
          'reasoning': {
            'efforts': [
              for (final effort in ['low', 'medium', 'high', 'max'])
                {'id': effort, 'name': effort},
            ],
          },
        },
      ],
    },
  ],
};

ProductVisualController productVisualFixture(bool dark) {
  final controller = ProductVisualController()
    ..preferences.dark = dark
    ..selectedId = 'startup-check'
    ..workspaceId = 'desktop'
    ..preset = 'standard'
    ..menuSettings = {'tasks': true}
    ..workspaces = [
      {
        'workspaceId': 'desktop',
        'title': '桌面客户端',
        'path': r'D:\Projects\desktop-client',
        'sessionIds': ['startup-check', 'resource-review'],
      },
    ]
    ..sessions = [
      for (final entry in [
        ('startup-check', '排查启动流程'),
        ('resource-review', '资源管理与交互检查'),
      ])
        SessionSummary.fromJson({
          'sessionId': entry.$1,
          'cwd': r'D:\Projects\desktop-client',
          'updatedAt': 1790697600000,
          'projections': {
            'values': {'title': entry.$2},
          },
        }),
    ]
    ..transcript = [
      TranscriptItem(
        id: 'question',
        kind: 'user',
        text: '检查客户端的启动流程，并整理需要继续跟进的事项。',
      ),
      TranscriptItem(
        id: 'answer',
        kind: 'assistant',
        text:
            '启动流程检查已完成。\n\n'
            '- 主窗口与会话列表正常加载。\n'
            '- 连接状态与模型选择保持一致。\n'
            '- 下一步检查日志导出和资源管理。',
      ),
    ]
    ..subscriptionAccounts = [
      {
        'id': 'openai-codex',
        'name': 'ChatGPT / Codex',
        'signedIn': true,
        'accounts': [
          {
            'accountId': 'preview-account',
            'label': 'demo@example.test',
            'active': true,
            'needsLogin': false,
          },
        ],
      },
      {
        'id': 'anthropic',
        'name': 'Claude',
        'signedIn': true,
        'accounts': [
          {
            'accountId': 'expired-preview',
            'label': 'review@example.test',
            'active': true,
            'needsLogin': true,
          },
        ],
      },
    ]
    ..catalog = ModelCatalog.fromJson(productVisualModelCatalog());
  controller.projectionWindow.apply('sessionStats', {
    'turns': 3,
    'steps': 9,
    'llmMs': 18400,
    'toolMs': 3200,
    'ttftMs': 6000,
    'ttftSteps': 3,
    'requestMs': 6400,
    'requestSamples': 3,
    'requestOutputTokens': 1280,
    'requestSources': [
      {'provider': 'DeepSeek', 'model': 'deepseek-chat'},
    ],
  }, 1);
  controller.projectionWindow.apply('tokenUsage', {
    'uncachedInputTokens': 4800,
    'cacheReadTokens': 7200,
    'cacheWriteTokens': 0,
    'outputTokens': 1280,
    'cacheStatistics': {
      'reportedSamples': 3,
      'unreportedSamples': 1,
      'reportedInputTokens': 12000,
    },
  }, 1);
  controller.projectionWindow.apply('contextPressure', {
    'projectedTokens': 19200,
    'contextWindow': 128000,
  }, 1);
  controller.projectionWindow.apply('contextBreakdown', {
    'systemTokens': 1600,
    'toolsTokens': 3600,
    'messageTokens': 14000,
  }, 1);
  return controller;
}

Future<void> exportProductFrame(
  WidgetTester tester,
  GlobalKey boundary,
  String filename,
) async {
  final output = Platform.environment['DSH_PRODUCT_VISUAL_DIR'];
  if (output == null) return;
  await tester.runAsync(() async {
    final image =
        await (boundary.currentContext!.findRenderObject()!
                as RenderRepaintBoundary)
            .toImage(pixelRatio: 1);
    final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
    await Directory(output).create(recursive: true);
    await File('$output/$filename.png')
        .writeAsBytes(bytes!.buffer.asUint8List());
    image.dispose();
  });
}

void main() {
  var fontsLoaded = false;
  for (final dark in [false, true]) {
    for (final viewport in [
      (size: const Size(1440, 900), scale: 1.0),
      (size: const Size(720, 640), scale: 2.0),
    ]) {
      testWidgets(
        'product shell and read-only overlays dark=$dark '
        '${viewport.size.width.toInt()}x${viewport.size.height.toInt()} '
        'text=${viewport.scale}',
        (tester) async {
          await tester.binding.setSurfaceSize(viewport.size);
          tester.view.devicePixelRatio = 1;
          tester.view.physicalSize = viewport.size;
          tester.platformDispatcher.textScaleFactorTestValue = viewport.scale;
          final previousShadows = debugDisableShadows;
          debugDisableShadows = false;
          addTearDown(() => debugDisableShadows = previousShadows);
          addTearDown(tester.view.resetDevicePixelRatio);
          addTearDown(tester.view.resetPhysicalSize);
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
          addTearDown(() => tester.binding.setSurfaceSize(null));
          // Ordinary runs and exported frames use the same Windows metrics.
          if (!fontsLoaded) {
            await tester.runAsync(() async {
              final windows = Platform.environment['WINDIR'] ?? 'C:/Windows';
              for (final entry in {
                'Segoe UI': 'segoeui.ttf',
                'Microsoft YaHei UI': 'msyh.ttc',
                'Microsoft YaHei': 'msyh.ttc',
                'Cascadia Mono': 'CascadiaMono.ttf',
                'Consolas': 'consola.ttf',
              }.entries) {
                final font = File('$windows/Fonts/${entry.value}');
                if (await font.exists()) {
                  await (FontLoader(
                        entry.key,
                      )..addFont(font.readAsBytes().then(ByteData.sublistView)))
                      .load();
                }
              }
              fontsLoaded = true;
            });
          }
          final controller = productVisualFixture(dark);
          final boundary = GlobalKey();
          addTearDown(() async {
            await tester.pumpWidget(const SizedBox());
            controller.dispose();
            await controller.api.close();
          });
          await tester.pumpWidget(
            RepaintBoundary(
              key: boundary,
              child: DesktopApp(controller: controller),
            ),
          );
          await tester.pumpAndSettle();
          final prefix =
              '${dark ? 'dark' : 'light'}-'
              '${viewport.size.width.toInt()}x${viewport.size.height.toInt()}-'
              '${(viewport.scale * 100).toInt()}pct';
          for (final view in [
            'conversation',
            'trajectory',
            'artifacts',
            'code-graph',
            'context',
          ]) {
            expect(
              find.byKey(ValueKey('conversation-view-$view')),
              findsOneWidget,
            );
          }
          expect(find.text('项目任务'), findsNothing);
          expect(
            find.byKey(const ValueKey('conversation-view-project-tasks')),
            findsNothing,
          );
          expect(find.byKey(const Key('prompt-input')), findsOneWidget);
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-main');
          final modelTrigger = find.byKey(
            const ValueKey('composer-model-menu'),
          );
          expect(modelTrigger.hitTestable(), findsOneWidget);
          final readsBeforeModel = controller.api.readPaths.length;
          final methodsBeforeModel = controller.api.readMethods.length;
          final modelHint =
              '${DshConversationZh.modelAndReasoning}：'
              'DeepSeek Chat · ${DshConversationZh.reasoningHigh}';
          final mouse = await tester.createGesture(
            kind: PointerDeviceKind.mouse,
          );
          await mouse.addPointer(location: Offset.zero);
          await mouse.moveTo(tester.getCenter(modelTrigger));
          await tester.pump(const Duration(milliseconds: 700));
          await tester.pumpAndSettle();
          expect(find.text(modelHint), findsOneWidget);
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-model-tooltip');
          await mouse.moveTo(Offset.zero);
          await tester.pumpAndSettle();
          expect(find.text(modelHint), findsNothing);
          await mouse.removePointer();
          await tester.tap(modelTrigger);
          await tester.pumpAndSettle();
          final modelPicker = find.byKey(const ValueKey('model-picker'));
          expect(modelPicker, findsOneWidget);
          expect(find.text(modelHint), findsNothing);
          expect(find.text('当前模型：DeepSeek Chat').hitTestable(), findsOneWidget);
          for (final effort in ['low', 'medium', 'high', 'max']) {
            final level = find.byKey(ValueKey('reasoning-level-$effort'));
            expect(level.hitTestable(), findsOneWidget);
            final label = find.descendant(
              of: level,
              matching: find.byType(Text),
            );
            expect(
              tester
                  .renderObject<RenderParagraph>(label)
                  .text
                  .style
                  ?.fontFamily,
              'Segoe UI',
            );
          }
          expect(controller.catalog!.current['reasoningEffort'], 'high');
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-model-picker');
          final contextAction = find.byKey(const Key('compact-now'));
          await tester.ensureVisible(contextAction);
          await tester.pumpAndSettle();
          expect(contextAction.hitTestable(), findsOneWidget);
          final modelDone = find.byKey(const ValueKey('model-picker-done'));
          expect(modelDone.hitTestable(), findsOneWidget);
          expect(tester.takeException(), isNull);
          await exportProductFrame(
            tester,
            boundary,
            '$prefix-model-picker-tail',
          );
          await tester.tap(modelDone);
          await tester.pumpAndSettle();
          expect(modelPicker, findsNothing);
          expect(modelTrigger.hitTestable(), findsOneWidget);
          expect(controller.catalog!.current['reasoningEffort'], 'high');
          expect(controller.api.readPaths.length, readsBeforeModel);
          expect(controller.api.readMethods.skip(methodsBeforeModel), [
            'session.models',
          ]);
          expect(controller.api.mutationAttempts, isEmpty);
          expect(find.byTooltip('显示工作台').hitTestable(), findsOneWidget);
          await tester.tap(find.byKey(const Key('more-header-menu')));
          await tester.pumpAndSettle();
          for (final key in [
            'session-menu-download',
            'session-menu-feedback',
            'session-menu-schedule',
          ]) {
            final item = find.byKey(Key(key));
            expect(item, findsOneWidget);
            expect(item.hitTestable(), findsOneWidget);
            final label = find
                .descendant(of: item, matching: find.byType(Text))
                .first;
            expect(
              tester
                  .renderObject<RenderParagraph>(label)
                  .text
                  .style
                  ?.fontFamily,
              'Segoe UI',
            );
          }
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-more');
          await tester.sendKeyEvent(LogicalKeyboardKey.escape);
          await tester.pumpAndSettle();
          expect(find.byKey(const Key('session-menu-download')), findsNothing);
          for (final overlay in [
            (
              key: 'session-statistics-button',
              title: DshStatisticsZh.modelTime,
              name: 'statistics',
            ),
            (
              key: 'session-usage-button',
              title: DshStatisticsZh.sessionUsage,
              name: 'usage',
            ),
            (
              key: 'session-context-button',
              title: '上下文已用 15%',
              name: 'context',
            ),
          ]) {
            final trigger = find.byKey(ValueKey(overlay.key));
            expect(trigger, findsOneWidget);
            final triggerRect = tester.getRect(trigger);
            expect(
              (Offset.zero & viewport.size).contains(triggerRect.topLeft),
              isTrue,
            );
            expect(
              (Offset.zero & viewport.size).contains(
                triggerRect.bottomRight - const Offset(.01, .01),
              ),
              isTrue,
            );
            await tester.tap(trigger);
            await tester.pumpAndSettle();
            expect(find.text(overlay.title), findsOneWidget);
            expect(
              tester
                  .renderObject<RenderParagraph>(find.text(overlay.title))
                  .text
                  .style
                  ?.fontFamily,
              'Segoe UI',
              reason: 'Overlay text must inherit the desktop typeface',
            );
            expect(tester.takeException(), isNull);
            await exportProductFrame(
              tester,
              boundary,
              '$prefix-${overlay.name}',
            );
            if (viewport.scale > 1.5 && overlay.name != 'context') {
              final lastRow = find.text(
                overlay.name == 'statistics'
                    ? DshStatisticsZh.sources
                    : DshStatisticsZh.cacheHit,
              );
              await tester.ensureVisible(lastRow);
              await tester.pumpAndSettle();
              expect(lastRow.hitTestable(), findsOneWidget);
              expect(tester.takeException(), isNull);
              await exportProductFrame(
                tester,
                boundary,
                '$prefix-${overlay.name}-tail',
              );
            }
            await tester.sendKeyEvent(LogicalKeyboardKey.escape);
            await tester.pumpAndSettle();
            expect(find.text(overlay.title), findsNothing);
          }
          if (viewport.size.width < 900) {
            await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
            await tester.sendKeyEvent(LogicalKeyboardKey.keyB);
            await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
            await tester.pumpAndSettle();
          }
          final settings = find.byKey(const Key('open-settings-direct'));
          final schedule = find.byKey(const Key('open-schedule-direct'));
          expect(settings, findsOneWidget);
          expect(schedule, findsOneWidget);
          expect(settings.hitTestable(), findsOneWidget);
          expect(schedule.hitTestable(), findsOneWidget);
          final account = find.byKey(const ValueKey('account-connection-menu'));
          expect(account, findsOneWidget);
          final accountRect = tester.getRect(account);
          final settingsRect = tester.getRect(settings);
          expect(accountRect.height, viewport.scale == 1 ? 36 : 58);
          expect(accountRect.size, settingsRect.size);
          expect(accountRect.left, settingsRect.left);
          expect(find.text('账号'), findsOneWidget);
          expect(accountRect.bottom, lessThanOrEqualTo(settingsRect.top));
          if (viewport.size.width < 900) {
            await exportProductFrame(tester, boundary, '$prefix-sidebar');
          }
          await tester.tap(settings);
          await tester.pumpAndSettle();
          expect(find.byType(SettingsShell), findsOneWidget);
          expect(
            tester
                .widget<SettingsShell>(find.byType(SettingsShell))
                .initialPage,
            'general',
          );
          expect(find.text('ChatGPT / Codex'), findsNothing);
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-settings');
          await tester.sendKeyEvent(LogicalKeyboardKey.escape);
          await tester.pumpAndSettle();
          expect(find.byType(SettingsShell), findsNothing);
          expect(settings.hitTestable(), findsOneWidget);
          expect(schedule.hitTestable(), findsOneWidget);
          await tester.tap(account);
          await tester.pumpAndSettle();
          expect(find.text('ChatGPT / Codex'), findsOneWidget);
          expect(find.textContaining('de***@example.test'), findsOneWidget);
          expect(find.textContaining('re***@example.test'), findsOneWidget);
          expect(find.text('de***@example.test · 已授权'), findsOneWidget);
          expect(find.text('re***@example.test · 需重新登录'), findsOneWidget);
          expect(find.text('管理订阅账号'), findsOneWidget);
          expect(find.text('API 连接与模型'), findsOneWidget);
          expect(find.text('设置'), findsOneWidget);
          expect(find.text('demo@example.test'), findsNothing);
          expect(find.text('review@example.test'), findsNothing);
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-account');
          await tester.sendKeyEvent(LogicalKeyboardKey.escape);
          await tester.pumpAndSettle();
          expect(find.text('ChatGPT / Codex'), findsNothing);
          expect(tester.takeException(), isNull);
          controller.subscriptionAccounts = [];
          controller.emit();
          await tester.pumpAndSettle();
          expect(account, findsNothing);
          expect(settings, findsOneWidget);
          expect(schedule, findsOneWidget);
          expect(settings.hitTestable(), findsOneWidget);
          expect(schedule.hitTestable(), findsOneWidget);
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-unauthorized');
          await tester.tap(find.byKey(const Key('open-plugins')));
          await tester.pumpAndSettle();
          expect(find.byType(PluginPage), findsOneWidget);
          expect(find.byType(SettingsShell), findsNothing);
          expect(find.text('8 个插件'), findsOneWidget);
          expect(
            find.byKey(const ValueKey('plugin-add')).hitTestable(),
            findsOneWidget,
          );
          final pluginHeading = find.descendant(
            of: find.byType(PluginPage),
            matching: find.text('插件'),
          );
          expect(
            tester
                .renderObject<RenderParagraph>(pluginHeading)
                .text
                .style
                ?.fontFamily,
            'Segoe UI',
          );
          expect(find.text('实验性'), findsOneWidget);
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-plugins');
          final pluginList = find
              .descendant(
                of: find.byType(PluginPage),
                matching: find.byType(Scrollable),
              )
              .last;
          final lastPlugin = find.byKey(const ValueKey('plugin-row-voice'));
          await tester.scrollUntilVisible(
            lastPlugin,
            250,
            scrollable: pluginList,
          );
          await tester.pumpAndSettle();
          expect(lastPlugin.hitTestable(), findsOneWidget);
          expect(find.text('语音输入'), findsOneWidget);
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-plugins-tail');
          await tester.pumpWidget(const SizedBox());
          controller.selectedId = null;
          controller.transcript = [];
          controller.projectionWindow.clear();
          await tester.pumpWidget(
            RepaintBoundary(
              key: boundary,
              child: DesktopApp(controller: controller),
            ),
          );
          await tester.pumpAndSettle();
          final readsBeforeStart = controller.api.readPaths.length;
          final methodsBeforeStart = controller.api.readMethods.length;
          await tester.tap(find.byKey(const Key('hero-open-workbench')));
          await tester.pumpAndSettle();
          expect(find.byType(WorkbenchStartPanel), findsOneWidget);
          expect(
            find.byKey(const ValueKey('workbench-tab-start')),
            findsOneWidget,
          );
          for (final key in ['files', 'terminal', 'computer-use']) {
            expect(
              find.byKey(ValueKey('workbench-start-$key')),
              findsOneWidget,
            );
          }
          expect(controller.selectedId, isNull);
          expect(controller.api.readPaths.length, readsBeforeStart);
          expect(controller.api.readMethods.length, methodsBeforeStart);
          expect(tester.takeException(), isNull);
          await exportProductFrame(tester, boundary, '$prefix-hero-start');
          if (viewport.size.width < 900) {
            final browserCard = find.byKey(
              const ValueKey('workbench-start-computer-use'),
            );
            await tester.ensureVisible(browserCard);
            await tester.pumpAndSettle();
            expect(browserCard.hitTestable(), findsOneWidget);
            expect(tester.takeException(), isNull);
            await exportProductFrame(
              tester,
              boundary,
              '$prefix-hero-start-tail',
            );
          }
          expect(controller.api.inventory.length, 8);
          expect(controller.api.readPaths.length, readsBeforeStart);
          expect(controller.api.readMethods.length, methodsBeforeStart);
          debugDisableShadows = previousShadows;
        },
        variant: TargetPlatformVariant({TargetPlatform.windows}),
        skip: !Platform.isWindows,
      );
    }
  }
}
