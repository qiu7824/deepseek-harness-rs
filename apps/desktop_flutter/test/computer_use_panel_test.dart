import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/computer_use_panel.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_ui/shadcn_ui.dart' show ShadApp;

import 'controller_test.dart' show FakeClient, MemoryPreferences;

const pixel =
    'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';

/// The same-origin `/__dsh-computer-use` routes of one Host.
class ComputerUseHost extends DshClient {
  ComputerUseHost() : super('http://127.0.0.1');
  bool enabled = true;
  List<String> browserSessions = [];
  Map<String, String> adapters = {'local': 'native-browser'};
  List<Json>? targets;
  final actions = <Json>[];
  final failures = <String, String>{};
  Json control = {'mode': 'agent', 'generation': 1};
  Iterable<Object?> get names => actions.map((a) => a['action']);

  @override
  Future<Json> request(
    String path, {
    Json? body,
    RequestScope? scope,
    bool mutation = false,
    int maxBytes = 16 * 1024 * 1024,
  }) async {
    final request = {...?body};
    // Requests also arrive inside guarded tester calls (focus changes during
    // enterText), where flutter_test's expect cannot run.
    if (request['ownerSessionId'] != 's') throw StateError('foreign owner');
    if (path == '/__dsh-computer-use/meta') {
      return {
        'enabled': enabled,
        'available': true,
        'adapter': adapters[request['target']] ?? adapters.values.first,
        'targets': targets,
        'actions': [
          'start',
          'capture',
          'click',
          'list_windows',
          'focus_window',
        ],
      };
    }
    if (path != '/__dsh-computer-use/action') throw StateError(path);
    actions.add(request);
    final name = request['action'] as String;
    final failure = failures.remove(name);
    if (failure != null) {
      throw DshException(
        'http-409',
        '$failure message',
        details: {'error': failure},
      );
    }
    if (name == 'list_sessions') return {'sessions': browserSessions};
    if (name == 'takeover' || name == 'resume_agent') {
      control = {
        'mode': name == 'takeover' ? 'manual' : 'agent',
        if (name == 'takeover') 'pauseReason': 'gui-takeover',
        'generation': (control['generation'] as int) + 1,
      };
    }
    if (name == 'close') return {};
    final adapter = adapters[request['target']] ?? adapters.values.first;
    return {
      'state': {
        // Only desktop drivers report a connection; browser state has none.
        if (adapter != 'native-browser') 'connected': true,
        'interactive': true,
        'phase': 'ready',
        'viewport': {'width': 800, 'height': 600},
        'url': 'https://example.com/',
        'title': 'Example',
      },
      'control': control,
      if (request['includeScreenshot'] == true)
        'screenshot': {'base64': pixel, 'mediaType': 'image/png'},
    };
  }
}

/// The workbench hosts panels on an opaque ColoredBox, as here.
Widget frame(Widget child) => ShadApp(
  home: Scaffold(
    body: Center(
      child: SizedBox(
        width: 820,
        height: 1000,
        child: ColoredBox(color: Colors.white, child: child),
      ),
    ),
  ),
);

/// Room for the whole panel, like a docked workbench on a desktop window.
Future<void> desktopSurface(WidgetTester tester) async {
  await tester.binding.setSurfaceSize(const Size(900, 1100));
  addTearDown(() => tester.binding.setSurfaceSize(null));
}

class ActivityApi extends FakeClient {
  void activity(int seq, Json data, {String owner = 's'}) =>
      channels.first.data.add(
        HostFrame.fromJson({
          'type': 'server-request',
          'rpcId': 'test',
          'payload': {
            'type': 'session/event',
            'sessionId': owner,
            'event': {
              'seq': seq,
              'type': 'computer-use/activity',
              'data': {'ownerSessionId': owner, ...data},
            },
          },
        }),
      );
}

void main() {
  testWidgets('a disabled runtime points to the settings that enable it', (
    tester,
  ) async {
    await desktopSurface(tester);
    final host = ComputerUseHost()..enabled = false;
    var opened = 0;
    await tester.pumpWidget(
      frame(
        ComputerUsePanel(
          api: host,
          session: 's',
          onOpenSettings: () => opened++,
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.textContaining('Computer Use 未启用'), findsOneWidget);
    expect(host.names, ['list_sessions']);
    await tester.tap(find.text('打开设置'));
    expect(opened, 1);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('an existing browser session is attached and receives input', (
    tester,
  ) async {
    await desktopSurface(tester);
    final host = ComputerUseHost()
      ..browserSessions = ['default', 'model']
      ..adapters = {'browser': 'native-browser'};
    await tester.pumpWidget(frame(ComputerUsePanel(api: host, session: 's')));
    await tester.pumpAndSettle();
    // Attaching observes the model's page instead of starting another one.
    expect(host.names, ['list_sessions', 'capture']);
    expect(host.actions.last['browserSessionId'], 'model');
    expect(host.actions.last['target'], 'browser');
    expect(find.text('智能体可操作'), findsOneWidget);

    final center = tester.getCenter(
      find.byKey(const ValueKey('computer-use-frame')),
    );
    await tester.tapAt(center);
    await tester.pumpAndSettle();
    expect(host.actions.last['action'], 'click');
    expect(host.actions.last['x'], closeTo(400, 1));
    expect(host.actions.last['y'], closeTo(300, 1));

    await tester.dragFrom(center, const Offset(120, 0));
    await tester.pumpAndSettle();
    expect(host.actions.last['action'], 'drag');
    expect(
      (host.actions.last['endX'] as num) - (host.actions.last['x'] as num),
      greaterThan(50),
    );

    final mouse = TestPointer(9, PointerDeviceKind.mouse);
    await tester.sendEventToBinding(mouse.hover(center));
    await tester.sendEventToBinding(mouse.scroll(const Offset(0, 120)));
    await tester.pumpAndSettle();
    expect(host.actions.last['action'], 'scroll');
    expect(host.actions.last['deltaY'], 120);

    await tester.enterText(
      find.byKey(const ValueKey('computer-use-url')),
      'example.org/path',
    );
    await tester.tap(find.text('转到'));
    await tester.pumpAndSettle();
    expect(host.actions.last['action'], 'navigate');
    expect(host.actions.last['url'], 'https://example.org/path');

    // Typing into the page is direct manipulation: it takes over first.
    await tester.enterText(
      find.byKey(const ValueKey('computer-use-typing')),
      '你好',
    );
    await tester.pumpAndSettle();
    expect(host.names, contains('takeover'));
    expect(find.text('交还智能体'), findsOneWidget);
    expect(find.text('智能体控制暂停 · 已在控制面板选择人工接管'), findsOneWidget);
    await tester.tap(find.text('输入'));
    await tester.pumpAndSettle();
    expect(host.actions.last, containsPair('text', '你好'));

    await tester.tap(find.text('交还智能体'));
    await tester.pumpAndSettle();
    expect(host.actions.last['action'], 'resume_agent');
    expect(find.text('人工接管'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets(
    'the local desktop refreshes while visible and switches targets',
    (tester) async {
      await desktopSurface(tester);
      final host = ComputerUseHost()
        ..adapters = {'local': 'native-desktop', 'browser': 'native-browser'}
        ..targets = [
          {'id': 'local', 'default': true},
          {'id': 'remote', 'requiresBinding': true},
          {'id': 'browser'},
        ];
      final key = GlobalKey<ComputerUsePanelState>();
      await tester.pumpWidget(
        frame(ComputerUsePanel(key: key, api: host, session: 's')),
      );
      await tester.pump();
      await tester.pump();
      expect(host.names, ['list_sessions', 'start']);
      expect(host.actions.last['target'], 'local');
      expect(find.textContaining('本机桌面 · 与本机共用键鼠'), findsOneWidget);
      await tester.pump(const Duration(milliseconds: 600));
      expect(host.names.last, 'capture');
      // A paused agent is expected while a person controls the desktop.
      host.failures['capture'] = 'COMPUTER_USE_MANUAL_CONTROL';
      await tester.pump(const Duration(milliseconds: 600));
      await tester.pump();
      expect(find.textContaining('COMPUTER_USE_MANUAL_CONTROL'), findsNothing);

      await key.currentState!.switchTarget('browser');
      await tester.pump();
      final tail = host.actions.skip(host.actions.length - 2).toList();
      expect(tail.map((a) => a['action']), ['close', 'start']);
      expect(tail.first['target'], 'local');
      expect(tail.last['target'], 'browser');
      expect(find.byKey(const ValueKey('computer-use-url')), findsOneWidget);

      // Closing the tab ends the session this view started.
      key.currentState!.endSession();
      await tester.pump();
      expect(host.actions.last['action'], 'close');
      expect(host.actions.last['target'], 'browser');
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('browser auto refresh observes the page while visible', (
    tester,
  ) async {
    await desktopSurface(tester);
    final host = ComputerUseHost()..adapters = {'local': 'native-browser'};
    await tester.pumpWidget(frame(ComputerUsePanel(api: host, session: 's')));
    await tester.pumpAndSettle();
    expect(host.names, ['list_sessions', 'start']);
    await tester.pump(const Duration(seconds: 4));
    expect(host.names.last, 'start');
    await tester.tap(find.text('自动刷新'));
    await tester.pump(const Duration(milliseconds: 3100));
    await tester.pump();
    expect(host.names.last, 'capture');
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('model activity attaches to its session and leaves it open', (
    tester,
  ) async {
    await desktopSurface(tester);
    final host = ComputerUseHost()..adapters = {'browser': 'native-browser'};
    final key = GlobalKey<ComputerUsePanelState>();
    await tester.pumpWidget(
      frame(
        ComputerUsePanel(
          key: key,
          api: host,
          session: 's',
          binding: const ComputerUseBinding(
            browserSessionId: 'agent-7',
            target: 'browser',
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(host.names, ['capture']);
    expect(host.actions.single['browserSessionId'], 'agent-7');
    key.currentState!.endSession();
    await tester.pump();
    expect(host.names, ['capture']);
    await tester.pumpWidget(const SizedBox());
  });

  test('model activity asks to show each control session once', () async {
    final api = ActivityApi();
    final c = DesktopController(MemoryPreferences(), clientFactory: (_) => api);
    await c.connect('http://127.0.0.1');
    await Future<void>.delayed(Duration.zero);
    await c.select('s');
    var requests = 0;
    c.computerUseRequests.addListener(() => requests++);
    final data = {
      'browserSessionId': 'default',
      'target': 'browser',
      'controlId': 'c1',
    };
    api.activity(10, {...data, 'action': 'start'});
    api.activity(11, {...data, 'action': 'click'});
    api.activity(11, {...data, 'action': 'start'});
    api.activity(12, {...data, 'target': 'nowhere', 'action': 'start'});
    api.activity(13, {...data, 'action': 'start'}, owner: 'other');
    await Future<void>.delayed(Duration.zero);
    expect(requests, 1);
    expect(
      c.computerUse,
      const ComputerUseBinding(browserSessionId: 'default', target: 'browser'),
    );
    api.activity(14, {...data, 'controlId': 'c2', 'action': 'capture'});
    api.activity(15, {...data, 'controlId': 'c2', 'action': 'start'});
    await Future<void>.delayed(Duration.zero);
    expect(requests, 3);
    c.newConversation();
    expect(c.computerUse, isNull);
    c.dispose();
    await Future<void>.delayed(Duration.zero);
  });
}
