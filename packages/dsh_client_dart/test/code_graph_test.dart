import 'package:dsh_client/dsh_client.dart';
import 'package:test/test.dart';

void main() {
  test(
    'parallel edges aggregate by resolution without inventing symbol targets',
    () {
      final graph = {
        'symbols': [
          {
            'id': 'a',
            'name': 'run',
            'path': 'a.py',
            'kind': 'function',
            'line': 2,
          },
          {
            'id': 'b',
            'name': 'work',
            'path': 'b.py',
            'kind': 'function',
            'line': 4,
          },
        ],
        'calls': [
          {
            'source': 'a',
            'target': 'b',
            'resolution': 'static',
            'path': 'a.py',
            'line': 3,
          },
          {
            'source': 'a',
            'target': 'b',
            'resolution': 'static',
            'path': 'a.py',
            'line': 5,
          },
          {
            'source': 'a',
            'target': 'b',
            'resolution': 'name-match',
            'path': 'a.py',
            'line': 8,
          },
          {'source': 'a', 'target': 'missing'},
        ],
      };
      final model = CodeGraphModel.fromJson(graph, files: false);
      expect(model.edges.length, 2);
      expect(model.edges.first.count, 2);
      expect(model.edges.first.line, 3);
      expect(model.edges.last.inferred, true);
      expect(model.nodes.map((n) => n.degree), [2, 2]);
      expect(model.nodes.any((n) => n.id == 'missing'), false);
    },
  );
  test(
    'file dependencies preserve direction, and overview and focus stay bounded',
    () {
      final graph = {
        'deps': List.generate(
          40,
          (i) => {
            'source': 'root.py',
            'target': 'lib/file$i.py',
            'kind': 'import',
            'path': 'root.py',
            'line': i + 1,
          },
        ),
      };
      final overview = CodeGraphModel.fromJson(graph);
      expect(overview.nodes.length, 6);
      expect(overview.catalog.length, 41);
      expect(overview.limited, true);
      final focus = CodeGraphModel.fromJson(graph, focus: 'root.py');
      expect(focus.nodes.first.id, 'root.py');
      expect(focus.nodes.length, 24);
      expect(focus.edges.length, 23);
      expect(focus.edges.every((e) => e.source == 'root.py'), true);
    },
  );
  test(
    'impact traversal is capped at three hops and file-only catalogs have no fake edges',
    () {
      final symbols = List.generate(
        6,
        (i) => {'id': '$i', 'path': '$i.rs', 'name': 'f$i', 'kind': 'function'},
      );
      final graph = {
        'symbols': symbols,
        'calls': List.generate(
          5,
          (i) => {'source': '$i', 'target': '${i + 1}'},
        ),
      };
      expect(
        CodeGraphModel.fromJson(
          graph,
          files: false,
          focus: '0',
          hops: 1,
        ).nodes.length,
        2,
      );
      expect(
        CodeGraphModel.fromJson(
          graph,
          files: false,
          focus: '0',
          hops: 99,
        ).nodes.length,
        4,
      );
      final files = CodeGraphModel.fromJson({'symbols': symbols});
      expect(files.nodes.length, 6);
      expect(files.edges, isEmpty);
    },
  );
  test(
    'source excerpts are line-addressed and bounded without splitting the file',
    () {
      final source = List.generate(10000, (i) => 'line ${i + 1}').join('\r\n');
      final excerpt = codeExcerpt(source, 8000);
      expect(excerpt, startsWith('8000  line 8000'));
      expect(excerpt, contains('8008  line 8008'));
      expect(excerpt, isNot(contains('line 8009')));
      expect(codeExcerpt(source, 10001), '');
      expect(
        codeExcerpt('x' * 100000, 1, maxCharacters: 64).length,
        lessThanOrEqualTo(64),
      );
    },
  );
}
