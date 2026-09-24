import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  final path = 'E:\\project\\.dsh-attachments\\${'a' * 64}\\${'b' * 64}\\报告.pdf';
  final receipt = 'Attached file: 报告.pdf\nPath: $path\nSize: 1068760 bytes';
  HistoryEvent user(List<Json> blocks, {String source = 'user'}) => HistoryEvent.fromJson({
    'seq': 9, 'type': 'user/message', 'data': {'source': {'kind': source}, 'content': blocks},
  });
  test('managed upload receipts become cards, with exact original copy text retained', () {
    final item = projectTranscript([user([
      {'type': 'text', 'text': '查看附件'}, {'type': 'text', 'text': receipt},
    ])]).single;
    expect(item.text, '查看附件');
    expect(item.files.single.path, path);
    expect(item.files.single.name, '报告.pdf');
    expect(item.files.single.displaySize, '1.0 MiB');
    expect(item.clipboardText, '查看附件\n\n$receipt');
    expect(projectTranscript([user([{'type': 'text', 'text': receipt}])]).single.files, hasLength(1));
  });
  test('ordinary prose, malformed paths and unsafe sizes remain literal text', () {
    for (final literal in [
      '请解释这个例子：\n$receipt',
      '```\n$receipt\n```',
      receipt.replaceFirst(path, 'E:\\ordinary\\报告.pdf'),
      receipt.replaceFirst('1068760', '9007199254740992'),
      '$receipt\n',
    ]) {
      final item = projectTranscript([user([{'type': 'text', 'text': literal}])]).single;
      expect(item.files, isEmpty);
      expect(item.text, literal);
    }
    final injected = projectTranscript([user([{'type': 'text', 'text': receipt}], source: 'plugin')]).single;
    expect(injected.kind, 'context');
    expect(injected.files, isEmpty);
  });
}
