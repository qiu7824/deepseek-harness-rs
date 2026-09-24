import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/src/app.dart';
import 'package:dsh_desktop/src/controller.dart';
import 'package:dsh_desktop/src/preferences.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

class PlanFlowPreferences extends DesktopPreferences {
  PlanFlowPreferences(String address) : super(address: address);
  @override
  Future<void> save() async {}
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets(
    'native plan review approves, keeps planning and cancels without changing choices',
    (tester) async {
      final address = Platform.environment['DSH_TEST_HOST']!,
          cwd = Platform.environment['DSH_TEST_CWD']!,
          home = Platform.environment['DSH_TEST_HOME']!;
      expect(home, contains('question-flow-host-qa'));
      final c = DesktopController(PlanFlowPreferences(address)),
          root = GlobalKey();
      addTearDown(() async {
        await tester.pumpWidget(const SizedBox());
        c.dispose();
      });
      await tester.pumpWidget(
        RepaintBoundary(
          key: root,
          child: DesktopApp(controller: c),
        ),
      );
      await c.connect(address);
      expect(c.host!.home.replaceFirst(r'\\?\', ''), home);
      Future<void> until(bool Function() ready) async {
        final end = DateTime.now().add(const Duration(seconds: 25));
        while (!ready()) {
          if (DateTime.now().isAfter(end)) {
            fail(
              'Plan flow timed out: ${c.error}; pending=${c.interactions.map((f) => f.type).toList()}; output=${c.transcript.map((m) => '${m.title} ${m.output}').join("\n")}',
            );
          }
          await tester.pump(const Duration(milliseconds: 50));
        }
        await tester.pump();
      }

      await until(() => c.connected);
      await c.addWorkspace(cwd);
      c.preset = 'standard';
      await c.create(cwd);
      final session = c.selectedId!;
      final results = <String, bool>{};
      final layouts = <String, Object>{};
      Future<void> capture(String name) async {
        final output = Platform.environment['DSH_QA_RESULT'];
        if (output == null) return;
        await tester.pump(const Duration(milliseconds: 100));
        final rect = tester.getRect(find.byKey(const Key('plan-review-card')));
        final button = tester.getSize(find.byKey(const Key('approve-plan')));
        layouts[name] = {
          'x': rect.left,
          'y': rect.top,
          'width': rect.width,
          'height': rect.height,
          'approveButtonWidth': button.width,
          'approveButtonHeight': button.height,
        };
        final image =
            await (root.currentContext!.findRenderObject()!
                    as RenderRepaintBoundary)
                .toImage();
        final data = await image.toByteData(format: ui.ImageByteFormat.png);
        await File('${File(output).parent.path}/plan-review-$name.png')
            .writeAsBytes(data!.buffer.asUint8List());
        image.dispose();
      }

      for (final (i, button) in ['确认执行', '拒绝', '去聊天里说'].indexed) {
        c.preferences.dark = i > 0;
        c.emit();
        await tester.pump();
        final mode = await c.client!.call('commands.execute', {
          'args': {'agentId': session, 'line': '/plan'},
        }, true);
        expect(object(mode['result'])['kind'], 'success');
        await c.sendParts('desktop-plan-$i', []);
        await until(
          () => c.interactions.any((f) => f.type == 'question/requested'),
        );
        expect(find.byKey(const Key('plan-review-card')), findsOneWidget);
        expect(find.text('计划待审'), findsOneWidget);
        await capture(
          i == 0
              ? 'light'
              : i == 1
              ? 'dark'
              : 'discuss',
        );
        await tester.tap(find.text(button));
        await until(() => c.interactions.isEmpty && !c.running && !c.sending);
        expect(c.error, isNull);
        await c.loadHistory();
        final active = c.window.events
            .where((e) => e.type == 'plan/mode')
            .last
            .data['active'];
        expect(
          active,
          i != 0,
          reason: 'Only explicit approval may leave plan mode',
        );
        results[button] = true;
      }
      expect(tester.takeException(), isNull);
      final output = Platform.environment['DSH_QA_RESULT'];
      if (output != null) {
        await File(output).writeAsString(
          jsonEncode({
            'pid': pid,
            'hostVersion': c.host!.version,
            'decisions': results,
            'layouts': layouts,
            'scope': 'Windows native Profile, installed Host and isolated deterministic provider; not a memory soak',
          }),
        );
      }
    },
  );
}
