import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/workbench/workbench_panel.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:xterm/xterm.dart';

void main() {
  final address = Platform.environment['DSH_TEST_HOST'];
  final cwd = Platform.environment['DSH_TEST_CWD'];
  final expectedHome = Platform.environment['DSH_TEST_HOME'];
  test(
    'isolated installed Host accepts chunked Unicode input and releases both terminals',
    () async {
      final api = DshClient(address!);
      final terminals = <String>[];
      try {
        final host = await api.describe();
        expect(expectedHome, contains('terminal-host-qa'));
        final nativeHome = host.home.startsWith(r'\\?\')
            ? host.home.substring(4)
            : host.home;
        expect(nativeHome, expectedHome);
        final workspace = await api.call('workspace.create', {
          'path': cwd!,
        }, true);
        final session =
            (await api.call('session.create', {
                  'cwd': cwd,
                  'workspaceId': object(workspace['workspace'])['workspaceId'],
                  'agentPreset': 'blank',
                }, true))['sessionId']
                as String;
        final permission = await api.call('commands.execute', {
          'args': {
            'agentId': session,
            'line': '/permission danger-full-access',
          },
        }, true);
        expect(object(permission['result'])['kind'], 'success');
        for (final name in ['A', 'B']) {
          final value = await api.request(
            '/__dsh-preview/terminal-action',
            body: {
              'sessionId': session,
              'action': 'open',
              'name': 'Native QA $name',
            },
            mutation: true,
          );
          terminals.add(value['id'] as String);
        }
        final input = '#${'中文🙂' * 3000}\r';
        final chunks = terminalInputChunks(input).toList();
        final output = <String, String>{};
        try {
          Future<void> send(String id, String text) async {
            final response = await api.request(
              '/__dsh-preview/terminal-action',
              body: {
                'sessionId': session,
                'terminalId': id,
                'action': 'input',
                'text': text,
              },
              mutation: true,
            );
            expect(response['written'], isTrue);
          }

          for (final chunk in chunks) {
            await send(terminals[0], chunk);
          }
          for (var i = 0; i < terminals.length; i++) {
            final name = i == 0 ? 'A' : 'B';
            await send(
              terminals[i],
              "Write-Output ('NATIVE'+'-TERMINAL-$name')\r",
            );
            final cursor = TerminalOutputWindow(),
                terminal = Terminal(maxLines: 3000);
            terminal.resize(100, 30);
            final deadline = DateTime.now().add(const Duration(seconds: 35));
            while (true) {
              final page = await api.request(
                previewUrl('terminal-read', session, {
                  'terminalId': terminals[i],
                  'count': 2000,
                }),
              );
              final update =
                  cursor.apply(page) ?? cursor.apply(page, reset: true)!;
              terminal.write(update);
              final text = terminal.buffer.getText();
              if (text.contains('NATIVE-TERMINAL-$name')) {
                expect(
                  text,
                  isNot(contains('NATIVE-TERMINAL-${name == 'A' ? 'B' : 'A'}')),
                );
                output[name] = 'passed';
                break;
              }
              if (DateTime.now().isAfter(deadline)) {
                fail(
                  'Terminal $name did not produce its output: ${text.substring(text.length > 800 ? text.length - 800 : 0)}',
                );
              }
              await Future<void>.delayed(const Duration(milliseconds: 100));
            }
          }
        } finally {
          for (final id in terminals) {
            await api.request(
              '/__dsh-preview/terminal-action',
              body: {'sessionId': session, 'terminalId': id, 'action': 'close'},
              mutation: true,
            );
          }
        }
        final remaining = await api.request(
          previewUrl('terminal-list', session),
        );
        expect(objects(remaining['entries']), isEmpty);
        expect(api.activeRequests, 0);
        final result = Platform.environment['DSH_QA_RESULT'];
        if (result != null) {
          await File(result).writeAsString(
            jsonEncode({
              'terminalOutputs': output,
              'inputBytes': utf8.encode(input).length,
              'chunkBytes': chunks.map((c) => utf8.encode(c).length).toList(),
              'closedTerminals': terminals.length,
              'remainingTerminals': 0,
              'activeRequestsAfterClose': api.activeRequests,
              'scope': 'Installed Host binary with isolated fixture configuration; HTTP, Unicode input and native xterm parser. Not a Release memory soak.',
            }),
          );
        }
      } finally {
        await api.close();
      }
    },
    skip: address == null || cwd == null || expectedHome == null,
    timeout: const Timeout(Duration(minutes: 2)),
  );
}
