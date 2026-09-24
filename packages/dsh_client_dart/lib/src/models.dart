import 'dart:convert';

typedef Json = Map<String, dynamic>;

Json object(Object? value) =>
    value is Map ? Map<String, dynamic>.from(value) : <String, dynamic>{};
List<Json> objects(Object? value) =>
    value is List ? value.map(object).toList() : <Json>[];

class HostInfo {
  HostInfo.fromJson(Json value)
    : version = value['version'] as String,
      home = value['home'] as String,
      cwd = value['cwd'] as String,
      supportsIdleTodoEdits = value['supportsIdleTodoEdits'] == true;
  final String version, home, cwd;
  final bool supportsIdleTodoEdits;
}

class SessionSummary {
  SessionSummary.fromJson(Json value)
    : id = value['sessionId'] as String,
      cwd = value['cwd'] as String? ?? '',
      agentPreset = value['agentPreset'] as String?,
      running = value['running'] == true,
      blank = value['blank'] == true,
      updatedAt = (value['updatedAt'] as num?)?.toInt() ?? 0,
      titleEditBase = _titleEditBase(value),
      title =
          object(object(value['projections'])['values'])['title'] as String? ??
          (value['displayTitle'] as String?) ??
          '新会话';
  final String id, cwd;
  final String? agentPreset;
  String title;
  Json? titleEditBase;
  bool running;
  bool blank;
  String get displayTitle => blank ? '新会话' : title;
  final int updatedAt;

  static Json? _titleEditBase(Json value) {
    final projection = object(value['projections']);
    final values = object(projection['values']);
    final seq = projection['asOfSeq'];
    final title = values['title'];
    if (seq is! int ||
        seq < -1 ||
        !values.containsKey('title') ||
        (title != null && title is! String))
      return null;
    return {'value': title, 'throughSeq': seq};
  }
}

class HistoryEvent {
  HistoryEvent.fromJson(Json value, {this.view})
    : seq = (value['seq'] as num).toInt(),
      type = value['type'] as String,
      data = object(value['data']),
      raw = value;
  final int seq;
  final String type;
  final Json data, raw;
  final Json? view;
  late final String? planText = () {
    if (data['name'] != 'exit_plan_mode') return null;
    final blocks = objects(object(view?['view'])['content']);
    final text = blocks
        .where((b) => b['type'] == 'text' && b['text'] is String)
        .map((b) => b['text'] as String)
        .join('\n\n');
    if (text.isNotEmpty) return text;
    final raw = data['arguments'];
    if (raw is! String || raw.length > 1024 * 1024) return null;
    try {
      final plan = object(jsonDecode(raw))['plan'];
      return plan is String ? plan : null;
    } on FormatException {
      return null;
    }
  }();

  /// Cache only summary fields, not a second copy of large tool input trees.
  late final Json toolSummaryArguments = () {
    final text = data['arguments'];
    if (text is! String || text.length > 65536) return <String, dynamic>{};
    try {
      final parsed = object(jsonDecode(text));
      return <String, dynamic>{
        for (final key in [
          'path',
          'file_path',
          'description',
          'command',
          'query',
          'pattern',
          'url',
          'action',
        ])
          if (parsed[key] is String) key: parsed[key],
      };
    } on FormatException {
      return <String, dynamic>{};
    }
  }();
  late final int retainedBytes = utf8
      .encode(jsonEncode({'event': raw, 'view': ?view}))
      .length;
  int get startSeq => (data['__historyStartSeq'] as num?)?.toInt() ?? seq;
  int get endSeq => (data['__historyEndSeq'] as num?)?.toInt() ?? seq;
}

class HistoryPage {
  HistoryPage.fromJson(Json value)
    : events = objects(value['events'])
          .map(
            (e) => HistoryEvent.fromJson(
              object(e['event']),
              view: e['view'] == null ? null : object(e['view']),
            ),
          )
          .toList(),
      hasBefore = value['hasMoreBefore'] == true,
      hasAfter = value['hasMoreAfter'] == true,
      firstSeq = (value['firstSeq'] as num?)?.toInt(),
      lastSeq = (value['lastSeq'] as num?)?.toInt(),
      projections = object(value['projections']);
  final List<HistoryEvent> events;
  final bool hasBefore, hasAfter;
  final int? firstSeq, lastSeq;
  final Json projections;
}

class HostFrame {
  HostFrame.fromJson(Json value)
    : rpcId = value['rpcId'] as String,
      payload = object(value['payload']) {
    if (value['type'] != 'server-request' || payload['type'] is! String) {
      throw const FormatException('Invalid server event envelope');
    }
  }
  final String rpcId;
  final Json payload;
  String get type => payload['type'] as String;
  String? get sessionId => payload['sessionId'] as String?;
}

class ModelChoice {
  ModelChoice({
    required this.provider,
    required this.id,
    required this.name,
    this.reasoning = const [],
  });
  final String provider, id, name;
  final List<Json> reasoning;
  String get key => '$provider\u0000$id';
}

class ModelCatalog {
  ModelCatalog.fromJson(Json value)
    : current = object(value['current']),
      providerNames = {
        for (final group in objects(value['groups']))
          group['id'] as String: '${group['name'] ?? group['id']}',
      },
      routable = value['routable'] == true,
      choices = [
        for (final group in objects(value['groups']))
          for (final model in objects(group['models']))
            ModelChoice(
              provider: group['id'] as String,
              id: model['id'] as String,
              name: model['name'] as String? ?? model['id'] as String,
              reasoning: objects(object(model['reasoning'])['efforts']),
            ),
      ],
      failures = objects(value['failures']);
  final Json current;
  final Map<String, String> providerNames;
  final bool routable;
  final List<ModelChoice> choices;
  final List<Json> failures;
  String get currentKey => '${current['provider']}\u0000${current['model']}';
  String get currentName =>
      choices.where((choice) => choice.key == currentKey).firstOrNull?.name ??
      current['model'] as String? ??
      '选择模型';
}
