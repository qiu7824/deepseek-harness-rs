import 'models.dart';

class CodeNode {
  CodeNode(this.id, this.name, this.path, this.line, this.kind);
  final String id, name, path, kind;
  final int line;
  int degree = 0;
  int get colorIndex =>
      path.runes.fold<int>(
        0,
        (sum, c) => sum + (c > 0xffff ? ((c - 0x10000) >> 10) + 0xd800 : c),
      ) %
      5;
}

class CodeLink {
  CodeLink(
    this.id,
    this.source,
    this.target,
    this.name,
    this.path,
    this.line,
    this.kind,
    this.inferred,
  );
  final String id, source, target, name, path, kind;
  final int line;
  final bool inferred;
  int count = 1;
}

class CodeGraphModel {
  const CodeGraphModel(this.nodes, this.edges, this.catalog, this.total);
  final List<CodeNode> nodes, catalog;
  final List<CodeLink> edges;
  final int total;
  bool get limited => total > nodes.length;
  factory CodeGraphModel.fromJson(
    Json graph, {
    bool files = true,
    String focus = '',
    int hops = 1,
    int cap = 24,
  }) {
    final nodes = <String, CodeNode>{}, edges = <String, CodeLink>{};
    String value(Json row, String key) =>
        row[key] is String ? row[key] as String : '';
    int line(Json row) => row['line'] is num
        ? (row['line'] as num).toInt().clamp(1, 10000000)
        : 1;
    String name(String path) => path.replaceAll('\\', '/').split('/').last;
    if (!files) {
      for (final row in objects(graph['symbols'])) {
        final id = value(row, 'id');
        if (id.isEmpty) continue;
        nodes[id] = CodeNode(
          id,
          value(row, 'name'),
          value(row, 'path'),
          line(row),
          value(row, 'kind'),
        );
      }
    }
    for (final row in objects(graph[files ? 'deps' : 'calls'])) {
      final source = value(row, 'source'), target = value(row, 'target');
      if (source.isEmpty || target.isEmpty) continue;
      if (files) {
        for (final path in [source, target]) {
          nodes.putIfAbsent(
            path,
            () => CodeNode(path, name(path), path, 1, 'file'),
          );
        }
      }
      if (!nodes.containsKey(source) || !nodes.containsKey(target)) continue;
      final resolution = value(row, 'resolution'), kind = value(row, 'kind');
      final id =
          '$source\u0000$target\u0000${resolution.isEmpty ? kind : resolution}';
      if (edges.containsKey(id)) {
        edges[id]!.count++;
        continue;
      }
      edges[id] = CodeLink(
        id,
        source,
        target,
        value(row, 'name'),
        value(row, 'path'),
        line(row),
        kind,
        resolution == 'name-match',
      );
    }
    if (files && nodes.isEmpty) {
      for (final row in objects(graph['symbols'])) {
        final path = value(row, 'path');
        if (path.isNotEmpty) {
          nodes.putIfAbsent(
            path,
            () => CodeNode(path, name(path), path, 1, 'file'),
          );
        }
      }
    }
    for (final e in edges.values) {
      nodes[e.source]!.degree++;
      nodes[e.target]!.degree++;
    }
    final catalog = nodes.values.toList()
      ..sort((a, b) {
        final d = b.degree.compareTo(a.degree);
        if (d != 0) return d;
        final p = a.path.compareTo(b.path);
        return p != 0 ? p : a.name.compareTo(b.name);
      });
    var visible = catalog;
    if (focus.isNotEmpty && nodes.containsKey(focus)) {
      final near = {focus};
      for (var depth = 0; depth < hops.clamp(1, 3); depth++) {
        final previous = Set<String>.of(near);
        for (final edge in edges.values) {
          if (previous.contains(edge.source) ||
              previous.contains(edge.target)) {
            near.add(edge.source);
            near.add(edge.target);
          }
        }
      }
      visible = catalog.where((n) => near.contains(n.id)).toList()
        ..sort((a, b) {
          if (a.id == b.id) return 0;
          if (a.id == focus) return -1;
          if (b.id == focus) return 1;
          final d = b.degree.compareTo(a.degree), p = a.path.compareTo(b.path);
          return d != 0
              ? d
              : p != 0
              ? p
              : a.name.compareTo(b.name);
        });
    }
    final total = visible.length;
    visible = visible.take(focus.isEmpty ? 6 : cap.clamp(1, 24)).toList();
    final ids = visible.map((n) => n.id).toSet();
    return CodeGraphModel(
      visible,
      edges.values
          .where((e) => ids.contains(e.source) && ids.contains(e.target))
          .toList(),
      catalog,
      total,
    );
  }
}

/// Extract a bounded source excerpt without splitting an entire file into lines.
String codeExcerpt(
  String source,
  int line, {
  int lines = 9,
  int maxCharacters = 4096,
}) {
  var start = 0, current = 1;
  while (current < line && start < source.length) {
    final next = source.indexOf('\n', start);
    if (next < 0) return '';
    start = next + 1;
    current++;
  }
  final out = StringBuffer();
  var left = maxCharacters;
  for (var i = 0; i < lines && start < source.length && left > 0; i++) {
    final next = source.indexOf('\n', start),
        end = next < 0 ? source.length : next;
    var cut = (start + left).clamp(start, end);
    if (cut < source.length &&
        cut > start &&
        source.codeUnitAt(cut) >= 0xdc00 &&
        source.codeUnitAt(cut) <= 0xdfff) {
      cut--;
    }
    final text = source.substring(start, cut).replaceAll('\r', '');
    final prefix = '${current + i}  ';
    final value = '$prefix$text${cut < end ? '…' : ''}\n';
    if (value.length > left) {
      var end = left;
      if (end > 0 &&
          value.codeUnitAt(end) >= 0xdc00 &&
          value.codeUnitAt(end) <= 0xdfff)
        end--;
      out.write(value.substring(0, end));
      break;
    }
    out.write(value);
    left -= value.length;
    if (next < 0) break;
    start = next + 1;
  }
  return out.toString().trimRight();
}
