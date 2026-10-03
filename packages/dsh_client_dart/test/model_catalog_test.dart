import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  test(
    'selection copies preserve declared reasoning metadata and directory',
    () {
      final source = ModelCatalog.fromJson({
        'current': {'provider': 'p', 'model': 'a', 'reasoningEffort': 'low'},
        'routable': true,
        'groups': [
          {
            'id': 'p',
            'name': 'Provider',
            'models': [
              {
                'id': 'a',
                'name': 'Model A',
                'reasoning': {
                  'defaultEffort': 'high',
                  'efforts': [
                    {'id': 'low', 'name': 'Low'},
                    {'id': 'high', 'name': 'High'},
                  ],
                },
              },
            ],
          },
        ],
        'failures': [
          {'provider': 'other', 'message': 'Unavailable'},
        ],
      });
      final selection = <String, dynamic>{
        'provider': 'p',
        'model': 'a',
        'reasoningEffort': 'high',
      };
      final changed = ModelCatalog.withCurrent(source, selection);
      selection['reasoningEffort'] = 'low';
      expect(source.current['reasoningEffort'], 'low');
      expect(changed.current['reasoningEffort'], 'high');
      expect(identical(changed.choices, source.choices), isTrue);
      expect(changed.choices.single.defaultReasoningEffort, 'high');
      expect(changed.choices.single.reasoning.length, 2);
      expect(changed.providerNames, {'p': 'Provider'});
      expect(changed.failures, source.failures);
      expect(changed.routable, isTrue);
    },
  );

  test(
    'an unselected directory does not inherit another session selection',
    () {
      final source = ModelCatalog.fromJson({
        'current': {'provider': 'p', 'model': 'a'},
        'routable': true,
        'groups': [],
      });
      final directory = ModelCatalog.withCurrent(source, {}, routable: false);
      expect(directory.current, isEmpty);
      expect(directory.routable, isFalse);
      expect(source.current['model'], 'a');
    },
  );
}
