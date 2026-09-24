import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

HistoryEvent event(int seq, String type, Json data) =>
    HistoryEvent.fromJson({'seq': seq, 'type': type, 'data': data});

void main() {
  test(
    'compaction lifecycle produces one summary instead of a user bubble',
    () {
      final events = [
        event(1, 'compaction/start', {'compactionId': 'c'}),
        event(2, 'compaction/summary', {
          'compactionId': 'c',
          'summary': [
            {'type': 'text', 'text': '保持目标与文件路径'},
          ],
        }),
        event(3, 'user/message', {
          'source': {
            'kind': 'plugin',
            'plugin': 'compact',
            'compactionId': 'c',
          },
          'content': [
            {'type': 'text', 'text': 'checkpoint'},
          ],
        }),
        event(4, 'compaction/end', {'compactionId': 'c'}),
      ];
      final items = projectTranscript(events);
      expect(items, hasLength(1));
      expect(items.single.kind, 'compaction');
      expect(items.single.status, 'complete');
      expect(items.single.text, '保持目标与文件路径');
      expect(projectTranscript(events.take(1)).single.status, 'pending');
    },
  );
  test('cancelled compaction is not represented as a successful summary', () {
    final items = projectTranscript([
      event(1, 'compaction/start', {'compactionId': 'c'}),
      event(2, 'compaction/end', {'compactionId': 'c', 'error': 'cancelled'}),
    ]);
    expect(items.single.status, 'failed');
    expect(items.single.text, 'cancelled');
  });
}
