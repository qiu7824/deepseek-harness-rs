import 'package:dsh_client/dsh_client.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('restart drops observations beyond the durable cut and accepts a lower baseline', () {
    final projection = ProjectionWindow();
    projection.snapshot({
      'asOfSeq': 20,
      'values': {
        'goal': {'objective': 'unflushed'},
        'plan': {'active': true},
      },
    }, requestVersion: 0);
    expect(projection.truncate(10), isTrue);
    expect(projection.values, isEmpty);
    expect(projection.retainedBytes, 0);
    projection.snapshot({
      'asOfSeq': 10,
      'values': {
        'goal': {'objective': 'durable'},
      },
    }, requestVersion: projection.version);
    expect(object(projection.values['goal'])['objective'], 'durable');
    expect(projection.truncate(10), isFalse);
  });
}
