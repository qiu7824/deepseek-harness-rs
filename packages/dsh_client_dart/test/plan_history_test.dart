import 'dart:convert';
import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  test(
    'plan text comes from structured presentation and preserves complete content',
    () {
      final text = '# 计划\n${'body\n' * 200000}';
      final event = HistoryEvent.fromJson(
        {
          'seq': 1,
          'type': 'tool/call',
          'data': {'name': 'exit_plan_mode', 'arguments': '{bad'},
        },
        view: {
          'view': {
            'title': '计划',
            'content': [
              {'type': 'text', 'text': text},
            ],
          },
        },
      );
      expect(projectTranscript([event]).single.planText, text);
    },
  );
  test(
    'legacy plan arguments are recognized without reclassifying other tools',
    () {
      HistoryEvent make(String name, String args) => HistoryEvent.fromJson({
        'seq': 1,
        'type': 'tool/call',
        'data': {'name': name, 'arguments': args},
      });
      expect(
        projectTranscript([
          make('exit_plan_mode', jsonEncode({'plan': '# Plan'})),
        ]).single.planText,
        '# Plan',
      );
      expect(
        projectTranscript([
          make('custom', jsonEncode({'plan': '# Plan'})),
        ]).single.planText,
        isNull,
      );
      expect(
        projectTranscript([make('exit_plan_mode', 'broken')]).single.planText,
        isNull,
      );
    },
  );
}
