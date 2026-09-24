import 'package:dsh_client/dsh_client.dart';
import 'package:flutter_test/flutter_test.dart';

HistoryEvent event(int seq, String type, Json data) => HistoryEvent.fromJson({
  'seq': seq,
  'time': seq,
  'type': type,
  'data': data,
  if (type == 'tool/result' || type == 'user/message') 'surfaceOp': 'append',
});

void main() {
  test(
    'structured files retain a resolvable reference and render as file cards',
    () {
      final reference = {
        'attachmentId': 'sha256:${List.filled(64, 'a').join()}',
        'name': '报告.docx',
        'bytes': 42,
      };
      final item = projectTranscript([
        event(0, 'user/message', {
          'source': {'kind': 'user'},
          'content': [
            {'type': 'file', 'attachment': reference},
          ],
        }),
      ], live: false).single;
      expect(item.files.single.name, '报告.docx');
      expect(item.files.single.size, 42);
      expect(
        UploadedFileReceipt.referenceFromPath(item.files.single.path),
        reference,
      );
      expect(item.text, isEmpty);
    },
  );
  test(
    'native failed tool result stays failed with and without its call window',
    () {
      final call = event(1, 'tool/call', {
        'turn': 1,
        'step': 1,
        'callId': 'call-1',
        'name': 'read',
        'arguments': '{}',
      });
      final result = event(2, 'tool/result', {
        'turn': 1,
        'step': 1,
        'message': {
          'id': 'result-1',
          'role': 'tool',
          'toolCallId': 'call-1',
          'isError': true,
          'source': {'kind': 'tool', 'callId': 'call-1'},
          'content': [
            {'type': 'text', 'text': 'Access denied'},
          ],
        },
      });
      for (final window in [
        [call, result],
        [result],
      ]) {
        final item = projectTranscript(window, live: false).single;
        expect(item.status, 'failed');
        expect('${item.output}${item.text}', contains('Access denied'));
      }
    },
  );
  test(
    'producer-owned compaction and runtime context retain their presentation',
    () {
      final items = projectTranscript([
        event(0, 'user/message', {
          'source': {'kind': 'compact-checkpoint', 'compactionId': 'compact-1'},
          'content': [
            {'type': 'text', 'text': 'Keep task'},
          ],
        }),
        event(1, 'user/message', {
          'source': {'kind': 'runtime-context'},
          'content': [
            {'type': 'text', 'text': 'Current workspace'},
          ],
        }),
      ], live: false);
      expect(items.map((item) => item.kind), ['compaction', 'context']);
      expect(items.last.summary, 'runtime-context');
    },
  );
}
