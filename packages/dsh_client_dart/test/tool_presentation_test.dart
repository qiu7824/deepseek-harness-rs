import 'dart:convert';
import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

List<HistoryEvent> call({
  String name = 'read',
  String args = '{"file_path":"E:\\\\project\\\\a.txt"}',
  String? result,
  String? error,
  Json view = const {},
}) => [
  HistoryEvent.fromJson(
    {
      'seq': 1,
      'type': 'tool/call',
      'data': {'turn': 1, 'callId': 'c', 'name': name, 'arguments': args},
    },
    view: {'view': view},
  ),
  if (result != null || error != null)
    HistoryEvent.fromJson({
      'seq': 2,
      'type': 'tool/result',
      'data': {
        'turn': 1,
        'message': {
          'source': {'callId': 'c'},
          'content': [
            {
              'type': 'tool-result',
              'isError': error != null,
              'content': [
                {'type': 'text', 'text': result ?? 'failure'},
              ],
            },
          ],
        },
        if (error != null) 'error': {'code': error},
      },
    }),
];
void main() {
  test(
    'file activities use wire name and state without repeating the path in the title',
    () {
      const path = r'E:\project\a.txt';
      final args = jsonEncode({'file_path': path});
      final done = projectTranscript(
        call(
          args: args,
          result: 'body',
          view: {
            'title': 'Read $path',
            'locations': [
              {'path': path},
            ],
          },
        ),
      ).single;
      expect(done.title, '已读取文件');
      expect(done.summary, path);
      expect(done.filePath, path);
      expect(done.text, args);
      expect(done.output, 'body');
      expect(
        projectTranscript(
          call(name: 'write', args: args, error: 'DENIED'),
        ).single.title,
        '写入文件失败',
      );
      expect(
        projectTranscript(call(name: 'edit', args: args)).single.title,
        '正在编辑文件',
      );
      final stopped = projectTranscript([
        ...call(args: args),
        HistoryEvent.fromJson({
          'seq': 3,
          'type': 'turn/end',
          'data': {'turn': 1},
        }),
      ]);
      expect(stopped.first.title, '已停止读取文件');
    },
  );
  test(
    'question rows distinguish answered, skipped, cancelled and interrupted',
    () {
      TranscriptItem question({String? output, String? error}) =>
          projectTranscript(
            call(
              name: 'ask_user_question',
              args: '{}',
              result: output,
              error: error,
            ),
          ).single;
      expect(question().summary, '等待回答');
      final output = jsonEncode({
        'answers': [
          {
            'id': 'a',
            'selected': ['选项'],
          },
          {'id': 'b', 'selected': [], 'custom': '说明'},
          {'id': 'c', 'selected': []},
        ],
      });
      expect(question(output: output).summary, '2/3 已回答');
      expect(
        question(output: '{"answers":[{"id":"a","selected":[]}]}').summary,
        '0/1 已回答',
      );
      expect(question(error: 'ASK_CANCELLED').summary, '已取消');
      final interrupted = question(error: 'ASK_ABORTED');
      expect(interrupted.summary, '已中断');
      expect(interrupted.status, 'interrupted');
      expect(
        question(
          output: '{"answers":[{"id":"a","selected":"not a list"}]}',
        ).summary,
        isEmpty,
      );
    },
  );
  test(
    'large and malformed arguments preserve source and use structured file locations',
    () {
      final raw = jsonEncode({
        'file_path': 'E:/project/a.txt',
        'content': 'x' * 70000,
      });
      final item = projectTranscript(
        call(
          name: 'write',
          args: raw,
          result: 'ok',
          view: {
            'locations': [
              {'path': 'E:/project/a.txt'},
            ],
          },
        ),
      ).single;
      expect(item.filePath, 'E:/project/a.txt');
      expect(item.text, raw);
      final unknown = projectTranscript(
        call(
          name: 'custom',
          args: '{invalid',
          result: 'raw',
          view: {'title': 'Custom inspection', 'summary': 'context'},
        ),
      ).single;
      expect(unknown.title, 'Custom inspection');
      expect(unknown.text, '{invalid');
      expect(unknown.output, 'raw');
    },
  );
  test('display paths respect workspace boundaries and Windows prefixes', () {
    expect(toolPathLabel(r'\\?\E:\Project\a.txt', r'e:\project'), 'a.txt');
    expect(
      toolPathLabel(r'E:\project-other\a.txt', r'E:\project'),
      r'E:\project-other\a.txt',
    );
    expect(toolPathLabel('/project/a.txt', '/project'), 'a.txt');
    expect(toolPathLabel('/Project/a.txt', '/project'), '/Project/a.txt');
  });
}
