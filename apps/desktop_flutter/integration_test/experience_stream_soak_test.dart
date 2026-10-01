import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' show FramePhase, FrameTiming;

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/conversation.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

class StreamSoakPreferences extends DesktopPreferences {
  @override
  Future<void> save() async {}
}

class StreamSoakChannel extends EventChannel {
  StreamSoakChannel() : super(Uri.parse('ws://127.0.0.1:1'));
  final ready = StreamController<bool>.broadcast();
  final incoming = StreamController<HostFrame>.broadcast();
  @override
  Stream<bool> get states => ready.stream;
  @override
  Stream<HostFrame> get frames => incoming.stream;
  @override
  void start() => ready.add(true);
  @override
  Future<void> close() async {
    await ready.close();
    await incoming.close();
  }
}

/// A bounded, deterministic in-process Host fixture. All updates enter the
/// controller through its EventChannel subscription and normal coalescing timer.
class StreamSoakClient extends DshClient {
  StreamSoakClient() : super('http://127.0.0.1:1') {
    for (var i = 0; i < 800; i++) {
      serverWindow.append(
        HistoryEvent.fromJson({
          'seq': sequence++,
          'type': i.isEven ? 'user/message' : 'assistant/message',
          'data': i.isEven
              ? {
                  'content': [
                    {'type': 'text', 'text': '分析历史记录 $i'},
                  ],
                }
              : {
                  'message': {
                    'content': [
                      {
                        'type': 'text',
                        'text': '**已完成**\n\n- 校验记录\n- 保留结果\n\n编号：$i',
                      },
                    ],
                  },
                },
        }),
      );
    }
  }
  static const session = 'stream-soak-fixture';
  final serverWindow = ConversationWindow();
  final channels = <String, StreamSoakChannel>{};
  int sequence = 0, eventEnvelopes = 0, historyRequests = 0;
  bool running = false;

  void publish(Json payload) {
    eventEnvelopes++;
    channels['mux']!.incoming.add(
      HostFrame.fromJson({
        'type': 'server-request',
        'rpcId': 'fixture-$eventEnvelopes',
        'payload': payload,
      }),
    );
  }

  int event(String type, Json data, {Json extra = const {}}) {
    final seq = sequence++;
    final event = {'seq': seq, 'type': type, 'data': data, ...extra};
    serverWindow.append(HistoryEvent.fromJson(event));
    publish({'type': 'session/event', 'sessionId': session, 'event': event});
    return seq;
  }

  void status(bool value) {
    running = value;
    publish({
      'type': 'host/session-status',
      'sessionId': session,
      'running': value,
    });
  }

  @override
  EventChannel events(String name) => channels[name] = StreamSoakChannel();
  @override
  Future<HostInfo> describe() async => HostInfo.fromJson({
    'version': 'fixture',
    'home': '/fixture',
    'cwd': '/fixture',
  });
  @override
  Future<List<SessionSummary>> sessions() async => [
    SessionSummary.fromJson({
      'sessionId': session,
      'cwd': '/fixture',
      'running': running,
    }),
  ];
  @override
  Future<ModelCatalog> models(String id) async => ModelCatalog.fromJson({
    'current': {'provider': 'fixture', 'model': 'fixture'},
    'groups': [],
    'routable': true,
  });
  @override
  Future<List<Json>> availableCommands(String id) async => [];
  @override
  Future<Json> call(
    String method, [
    Json payload = const {},
    bool mutation = false,
  ]) async => switch (method) {
    'workspace.list' => {
      'items': [
        {
          'workspaceId': 'fixture',
          'title': 'Fixture',
          'path': '/fixture',
          'sessionIds': [session],
        },
      ],
      'archivedSessionIds': [],
    },
    'pluginInventory.list' => {
      'entries': [
        {'moduleName': 'dsh-voice-input', 'enabled': false},
      ],
    },
    _ => {'items': [], 'entries': [], 'presets': [], 'namespaces': []},
  };
  @override
  Future<Json> rpc(
    String method, {
    Json payload = const {},
    bool mutation = false,
    RequestScope? scope,
  }) => call(method, payload, mutation);
  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async => {'items': [], 'entries': [], 'providers': []};
  @override
  Future<HistoryPage> history(
    String id, {
    int? before,
    int? after,
    RequestScope? scope,
  }) async {
    if (id != session || scope?.cancelled == true) {
      throw StateError('Invalid fixture history request');
    }
    historyRequests++;
    return HistoryPage.fromJson({
      'events': [
        for (final event in serverWindow.events) {'event': event.raw},
      ],
      'firstSeq': serverWindow.firstSeq,
      'lastSeq': serverWindow.lastSeq,
      'hasMoreBefore': serverWindow.hasBefore,
      'hasMoreAfter': false,
    });
  }

  @override
  Future<void> close() async {
    for (final channel in channels.values) {
      await channel.close();
    }
    await super.close();
  }
}

const _markdownSegment =
    '# 执行记录\n\n**进度**与行内代码 `value`。\n\n'
    '- 保留第一项\n- 校验第二项\n\n'
    '```dart\nfinal value = 42;\nprint(value);\n```\n\n'
    '| 名称 | 状态 |\n| --- | --- |\n| 客户端 | 正常 |\n\n';

Future<Map<String, Object>> driveStreamSoak(
  WidgetTester tester,
  DesktopController controller,
  StreamSoakClient api, {
  required int seconds,
  int Function()? nowMicros,
  bool realTime = false,
  Future<void> Function(int completedReplies)? onProgress,
}) async {
  final watch = Stopwatch()..start();
  final now = nowMicros ?? () => watch.elapsedMicroseconds;
  final started = now();
  final source = List.filled(24, _markdownSegment).join();
  final count = seconds * 30;
  int paints = 0, completed = 0, firstDelta = -1, lastDelta = -1;
  int maxLag = 0, lateDeltas = 0;
  var body = '', lastCompletedText = '';
  void painted() => paints++;
  controller.messageChanges.addListener(painted);
  Future<void> advance(Duration duration) =>
      realTime ? Future<void>.delayed(duration) : tester.pump(duration);

  Future<void> complete(int turn) async {
    api.event(
      'assistant/message',
      {
        'turn': turn,
        'step': 1,
        'message': {
          'id': 'message-$turn',
          'content': [
            {'type': 'text', 'text': body},
          ],
        },
      },
      extra: {
        'surfaceOp': 'append',
        'sourceEventSeqs': [
          [firstDelta, lastDelta],
        ],
      },
    );
    api.event('turn/end', {
      'turn': turn,
      'reason': {'kind': 'completed'},
    });
    api.status(false);
    lastCompletedText = body;
    completed++;
    await advance(Duration.zero);
    await onProgress?.call(completed);
  }

  try {
    for (var i = 0; i < count; i++) {
      final due = ((i + 1) * 1000000 / 30).round();
      final wait = due - (now() - started);
      await advance(wait > 0 ? Duration(microseconds: wait) : Duration.zero);
      final lag = (now() - started) - due;
      if (lag > maxLag) maxLag = lag;
      if (lag > 16667) lateDeltas++;
      final turn = i ~/ 300, part = i % 300;
      if (part == 0) {
        body = '';
        api.running = true;
        api.event('user/message', {
          'content': [
            {'type': 'text', 'text': '执行固定回复 $turn'},
          ],
        });
        api.event('turn/start', {'turn': turn});
      }
      final text = source.substring(
        source.length * part ~/ 300,
        source.length * (part + 1) ~/ 300,
      );
      body += text;
      lastDelta = api.event('assistant/chunk', {
        'turn': turn,
        'step': 1,
        'chunk': {'type': 'text-delta', 'index': 0, 'text': text},
      });
      if (part == 0) firstDelta = lastDelta;
      if (part == 100) {
        api.event('tool/call', {
          'turn': turn,
          'step': 1,
          'callId': 'read-$turn',
          'name': 'read_file',
          'arguments': jsonEncode({'path': 'fixture.txt'}),
        });
      }
      if (part == 130) {
        api.event('tool/result', {
          'turn': turn,
          'callId': 'read-$turn',
          'message': {
            'source': {'callId': 'read-$turn'},
            'content': [
              {'type': 'text', 'text': 'Fixed fixture result.'},
            ],
          },
        });
      }
      if (part == 299 || i == count - 1) await complete(turn);
    }
    final emissionMicros = now() - started;
    await advance(const Duration(milliseconds: 550));
    expect(
      controller.window.retainedBytes,
      lessThanOrEqualTo(controller.window.maxBytes),
    );
    expect(
      controller.window.eventCount,
      lessThanOrEqualTo(controller.window.maxEvents),
    );
    expect(
      controller.transcript.any(
        (item) =>
            item.kind == 'assistant' &&
            item.text == lastCompletedText &&
            !item.streaming,
      ),
      isTrue,
    );
    expect(paints, greaterThan(0));
    expect(
      paints,
      lessThan(count),
      reason: 'The real controller must coalesce delta notifications.',
    );
    return {
      'deltas': count,
      'completedReplies': completed,
      'messageNotifications': paints,
      'emissionMicros': emissionMicros,
      'actualDeltasPerSecond': count * 1000000 / emissionMicros,
      'deltaSchedulingLagOver16ms': lateDeltas,
      'maximumSchedulingLagMicros': maxLag,
      'historyRequests': api.historyRequests,
      'fixtureHistoryBytes': api.serverWindow.retainedBytes,
      'fixtureHistoryEvents': api.serverWindow.eventCount,
      'controller': controller.resourceDiagnostics,
    };
  } finally {
    controller.messageChanges.removeListener(painted);
  }
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.fullyLive;
  final output =
      Platform.environment['DSH_PERF_OUTPUT'] ??
      const String.fromEnvironment('DSH_PERF_OUTPUT');
  final seconds =
      int.tryParse(Platform.environment['DSH_STREAM_SECONDS'] ?? '') ?? 600;
  final exitWhenDone = Platform.environment['DSH_PERF_EXIT'] == '1';
  final frames = <int>[];
  final samples = <Map<String, Object>>[];
  final report = <String, Object?>{
    'schemaVersion': 1,
    'fixture': 'experience-stream-soak-v1',
    'processId': pid,
    'buildMode': kProfileMode
        ? 'profile'
        : kReleaseMode
        ? 'release'
        : 'debug',
    'dartVersion': Platform.version,
    'operatingSystem': Platform.operatingSystemVersion,
    'durationSeconds': seconds,
    'targetDeltaRate': 30,
    'initialHistoryMessages': 800,
    'replyIntervalSeconds': 10,
    'frameBudgetMicros': 16667,
    'eventRoute': 'Fake EventChannel -> DesktopController._onFrame -> normal 72 ms coalescing',
    'coverage': {
      'realHost': false,
      'installedPackage': false,
      'tenMinuteScenario': seconds >= 600,
    },
    'state': 'running',
    'startedAt': DateTime.now().toUtc().toIso8601String(),
    'samples': samples,
  };
  Future<void> save() async {
    if (output.isEmpty) return;
    final file = File(output);
    await file.parent.create(recursive: true);
    await file.writeAsString(jsonEncode(report), flush: true);
  }

  tearDownAll(() async {
    report['state'] = binding.failureMethodsDetails.isEmpty
        ? 'passed'
        : 'failed';
    report['failures'] = binding.failureMethodsDetails
        .map((failure) => failure.details)
        .toList();
    report['finishedAt'] = DateTime.now().toUtc().toIso8601String();
    await save();
    if (exitWhenDone) {
      Timer(
        const Duration(seconds: 1),
        () => exit(binding.failureMethodsDetails.isEmpty ? 0 : 1),
      );
    }
  });
  testWidgets('thirty deltas per second through the real controller', (
    tester,
  ) async {
    expect(
      output,
      isNotEmpty,
      reason: 'Set DSH_PERF_OUTPUT to the report path.',
    );
    expect(seconds, inInclusiveRange(1, 3600));
    expect(
      kReleaseMode,
      isFalse,
      reason: 'watchPerformance requires the Profile VM service.',
    );
    await save();
    await tester.binding.setSurfaceSize(const Size(1280, 800));
    PaintingBinding.instance.imageCache.maximumSize = 128;
    PaintingBinding.instance.imageCache.maximumSizeBytes = 48 * 1024 * 1024;
    final api = StreamSoakClient();
    final controller = DesktopController(
      StreamSoakPreferences(),
      clientFactory: (_) => api,
    );
    await controller.connect('http://127.0.0.1:1');
    await controller.select(StreamSoakClient.session);
    await tester.pumpWidget(
      ShadApp(
        home: Scaffold(body: Conversation(controller: controller)),
      ),
    );
    await tester.pumpAndSettle();
    expect(controller.transcript.length, 800);
    final view = binding.platformDispatcher.views.first;
    report['viewport'] = {
      'logicalWidth': 1280,
      'logicalHeight': 800,
      'physicalWidth': view.physicalSize.width,
      'physicalHeight': view.physicalSize.height,
      'devicePixelRatio': view.devicePixelRatio,
    };
    report['imageCacheBudgetBytes'] =
        PaintingBinding.instance.imageCache.maximumSizeBytes;
    void timings(List<FrameTiming> batch) {
      for (final frame in batch) {
        frames.addAll([
          frame.timestampInMicroseconds(FramePhase.vsyncStart),
          frame.buildDuration.inMicroseconds,
          frame.rasterDuration.inMicroseconds,
          frame.totalSpan.inMicroseconds,
        ]);
      }
    }

    addTearDown(() async {
      binding.removeTimingsCallback(timings);
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
    });
    try {
      await binding.watchPerformance(() async {
        binding.addTimingsCallback(timings);
        report['stream'] = await driveStreamSoak(
          tester,
          controller,
          api,
          seconds: seconds,
          realTime: true,
          onProgress: (completed) async {
            samples.add({
              'time': DateTime.now().toUtc().toIso8601String(),
              'completedReplies': completed,
              'currentRssBytes': ProcessInfo.currentRss,
              'historyBytes': controller.window.retainedBytes,
              'historyEvents': controller.window.eventCount,
            });
            await save();
          },
        );
      }, reportKey: 'controller_stream_30hz');
      binding.removeTimingsCallback(timings);
      expect(frames, isNotEmpty);
      report['reports'] = binding.reportData;
      report['rawFrameTimings'] = {
        'columns': [
          'vsyncStartMicros',
          'buildMicros',
          'rasterMicros',
          'totalSpanMicros',
        ],
        'values': frames,
      };
      final builds = <int>[], rasters = <int>[];
      for (var i = 0; i < frames.length; i += 4) {
        builds.add(frames[i + 1]);
        rasters.add(frames[i + 2]);
      }
      builds.sort();
      rasters.sort();
      final p95 = ((builds.length - 1) * .95).round();
      report['frameTargets'] = {
        'frameCount': builds.length,
        'buildP95Micros': builds[p95],
        'rasterP95Micros': rasters[p95],
        'buildOverBudgetRatio':
            builds.where((value) => value > 16667).length / builds.length,
        'rasterOverBudgetRatio':
            rasters.where((value) => value > 16667).length / rasters.length,
      };
      expect(tester.takeException(), isNull);
      await save();
    } finally {
      binding.removeTimingsCallback(timings);
    }
  }, timeout: const Timeout(Duration(minutes: 75)));
}
