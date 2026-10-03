import 'package:dsh_client/dsh_client.dart';
import 'package:dsh_desktop/features/page_operation.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('only a missing catalog is reported as an older Host', () {
    final missing = DshException('http-404', 'not found');
    final older = unsupportedHostPage(missing, 'catalog', '定时任务');
    expect(older, isA<DshException>());
    expect((older as DshException).code, 'unsupported');
    expect(older.message, contains('定时任务'));
    // A missing task or knowledge base is a real 404 and keeps its meaning.
    expect(unsupportedHostPage(missing, 'documents', '知识库'), same(missing));
    final conflict = DshException('http-409', 'conflict');
    expect(unsupportedHostPage(conflict, 'catalog', '知识库'), same(conflict));
  });
  test('older Hosts answering 405 or an unknown method read the same way', () {
    for (final code in ['http-405', 'unsupported']) {
      final error = DshException(code, '服务请求失败');
      expect(isUnsupportedHost(error), isTrue);
      final page = unsupportedHostPage(error, 'catalog', '定时任务');
      expect((page as DshException).code, 'unsupported');
      expect(page.message, contains('版本过旧'));
    }
    expect(isUnsupportedHost(DshException('http-500', 'boom')), isFalse);
    expect(isUnsupportedHost(StateError('x')), isFalse);
  });
}
