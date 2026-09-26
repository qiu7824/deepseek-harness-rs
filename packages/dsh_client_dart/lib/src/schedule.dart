import 'dart:convert';

import 'client.dart';
import 'models.dart';
import 'resources.dart';

Never _invalid(String field) =>
    throw DshException('protocol', '提醒数据格式无效：$field');
Json _map(Object? value, String field) {
  if (value is! Map) _invalid(field);
  return object(value);
}

String _text(Json value, String key) {
  final item = value[key];
  if (item is! String || item.isEmpty) _invalid(key);
  return item;
}

bool _flag(Json value, String key) {
  final item = value[key];
  if (item is! bool) _invalid(key);
  return item;
}

String? _optionalText(Json value, String key) =>
    value[key] == null ? null : _text(value, key);

T _mutationResult<T>(T Function() parse) {
  try {
    return parse();
  } on DshException catch (error) {
    throw DshException(
      error.code,
      error.message,
      details: error.details,
      outcomeUnknown: true,
    );
  }
}

/// An immutable, complete compare-and-set record. Catalog metadata is excluded.
class ScheduleRecord {
  ScheduleRecord._(this._encoded);
  factory ScheduleRecord.fromJson(Object? raw) {
    final value = _map(raw, 'record');
    for (final key in ['id', 'kind', 'title', 'prompt', 'scheduledAt']) {
      _text(value, key);
    }
    final fields = <String>['id', 'kind', 'title', 'prompt', 'scheduledAt'];
    switch (value['kind']) {
      case 'after':
      case 'every':
        final key = value['kind'] == 'after' ? 'afterSeconds' : 'everySeconds';
        if (value[key] is! int || (value[key] as int) <= 0) _invalid(key);
        fields.add(key);
      case 'at':
        break;
      case 'daily':
      case 'weekly':
        _text(value, 'time');
        _text(value, 'timeZone');
        fields.addAll(['time', 'timeZone']);
        if (value['kind'] == 'weekly') {
          final days = value['weekdays'];
          if (days is! List ||
              days.isEmpty ||
              days.any((day) => day is! int || day < 1 || day > 7) ||
              days.toSet().length != days.length)
            _invalid('weekdays');
          fields.add('weekdays');
        }
      case 'cron':
        _text(value, 'expression');
        _text(value, 'timeZone');
        fields.addAll(['expression', 'timeZone']);
      default:
        _invalid('kind');
    }
    return ScheduleRecord._(
      jsonEncode({for (final key in fields) key: value[key]}),
    );
  }
  final String _encoded;
  Json toJson() => object(jsonDecode(_encoded));
  String get id => toJson()['id'] as String;
  String get kind => toJson()['kind'] as String;
  String get title => toJson()['title'] as String;
  String get prompt => toJson()['prompt'] as String;
  String get scheduledAt => toJson()['scheduledAt'] as String;
}

class ScheduleDelivery {
  ScheduleDelivery.fromJson(Object? raw) {
    final value = _map(raw, 'delivery');
    scheduledAt = _text(value, 'scheduledAt');
    deliveredAt = _text(value, 'deliveredAt');
    messageId = _text(value, 'messageId');
    if (value['prompt'] != null && value['prompt'] is! String)
      _invalid('prompt');
    prompt = value['prompt'] as String?;
  }
  late final String scheduledAt, deliveredAt, messageId;
  late final String? prompt;
}

class ScheduleEntry {
  ScheduleEntry.fromJson(Object? raw) {
    final value = _map(raw, 'catalog entry');
    record = ScheduleRecord.fromJson(value);
    sessionId = _text(value, 'sessionId');
    status = _text(value, 'status');
    if (!['active', 'inactive'].contains(status)) _invalid('status');
    lastDelivery = value['lastDelivery'] == null
        ? null
        : ScheduleDelivery.fromJson(value['lastDelivery']);
  }
  late final ScheduleRecord record;
  late final String sessionId, status;
  late final ScheduleDelivery? lastDelivery;
  bool get active => status == 'active';
}

class ScheduleUpdateResult {
  ScheduleUpdateResult.fromJson(Object? raw) {
    final value = _map(raw, 'update');
    id = _text(value, 'id');
    updated = _flag(value, 'updated');
    code = _optionalText(value, 'code');
    record = value['record'] == null
        ? null
        : ScheduleRecord.fromJson(value['record']);
    if ((record == null) == (code == null) ||
        (updated && code != null) ||
        (record != null && record!.id != id))
      _invalid('update result');
  }
  late final String id;
  late final bool updated;
  late final String? code;
  late final ScheduleRecord? record;
}

class ScheduleHistoryPage {
  ScheduleHistoryPage.fromJson(Object? raw) {
    final value = _map(raw, 'history');
    id = _text(value, 'id');
    code = _optionalText(value, 'code');
    if (code != null) {
      records = const [];
      nextBefore = null;
      earlierRecordsUnavailable = earlierRecordsPruned = false;
      retentionDays = retentionRecords = null;
      return;
    }
    if (value['records'] is! List) _invalid('records');
    records = List.unmodifiable(
      (value['records'] as List).map(ScheduleDelivery.fromJson),
    );
    nextBefore = _optionalText(value, 'nextBefore');
    earlierRecordsUnavailable = _flag(value, 'earlierRecordsUnavailable');
    earlierRecordsPruned = _flag(value, 'earlierRecordsPruned');
    final retention = _map(value['retention'], 'retention');
    if (retention['days'] is! int || retention['records'] is! int)
      _invalid('retention');
    retentionDays = retention['days'] as int;
    retentionRecords = retention['records'] as int;
  }
  late final String id;
  late final String? code, nextBefore;
  late final List<ScheduleDelivery> records;
  late final bool earlierRecordsUnavailable, earlierRecordsPruned;
  late final int? retentionDays, retentionRecords;
}

/// Reminders live in Host storage and stay bound to their original session.
class ScheduleApi {
  ScheduleApi(this.client);
  final DshClient client;

  Future<List<ScheduleEntry>> catalog({RequestScope? scope}) async {
    final value = await client.callValue('schedule.catalog', scope: scope);
    if (value is! List) _invalid('catalog');
    return List.unmodifiable(value.map(ScheduleEntry.fromJson));
  }

  Future<ScheduleRecord> create({
    required String sessionId,
    required String title,
    required String prompt,
    required Json timing,
    RequestScope? scope,
  }) async {
    if (timing.length != 1 ||
        !const {
          'after_seconds',
          'at',
          'every_seconds',
          'daily',
          'weekly',
          'cron',
        }.contains(timing.keys.single)) {
      throw ArgumentError('Creation requires exactly one timing selector.');
    }
    final raw = await client.callValue(
      'schedule.create',
      mutation: true,
      scope: scope,
      payload: {
        'sessionId': sessionId,
        'title': title,
        'prompt': prompt,
        ...timing,
      },
    );
    return _mutationResult(() => ScheduleRecord.fromJson(raw));
  }

  Future<ScheduleUpdateResult> update({
    required String sessionId,
    required ScheduleRecord expected,
    String? title,
    String? prompt,
    Json? change,
    RequestScope? scope,
  }) async {
    final raw = await client.callValue(
      'schedule.update',
      mutation: true,
      scope: scope,
      payload: {
        'sessionId': sessionId,
        'id': expected.id,
        'expected': expected.toJson(),
        if (title != null) 'title': title,
        if (prompt != null) 'prompt': prompt,
        if (change != null) 'change': change,
      },
    );
    return _mutationResult(() {
      final result = ScheduleUpdateResult.fromJson(raw);
      if (result.id != expected.id) _invalid('update identity');
      return result;
    });
  }

  Future<bool> delete({
    required String sessionId,
    required String id,
    RequestScope? scope,
  }) async {
    final raw = await client.callValue(
      'schedule.delete',
      mutation: true,
      scope: scope,
      payload: {'sessionId': sessionId, 'id': id},
    );
    return _mutationResult(() {
      final result = _map(raw, 'delete');
      if (result['id'] != id) _invalid('delete identity');
      final deleted = _flag(result, 'deleted');
      if (!deleted && result['code'] != 'schedule_not_found')
        _invalid('delete result');
      return deleted;
    });
  }

  Future<ScheduleHistoryPage> history({
    required String sessionId,
    required String id,
    int limit = 20,
    String? before,
    RequestScope? scope,
  }) async {
    if (limit < 1 || limit > 100)
      throw RangeError.range(limit, 1, 100, 'limit');
    final page = ScheduleHistoryPage.fromJson(
      await client.callValue(
        'schedule.history',
        scope: scope,
        payload: {
          'sessionId': sessionId,
          'id': id,
          'limit': limit,
          if (before != null) 'before': before,
        },
      ),
    );
    if (page.id != id) _invalid('history identity');
    return page;
  }

  Future<void> retry({RequestScope? scope}) async {
    final raw = await client.callValue(
      'schedule.retry',
      mutation: true,
      scope: scope,
    );
    _mutationResult(() {
      final result = _map(raw, 'retry');
      if (result['requested'] != true || result['enabled'] is! bool)
        _invalid('retry result');
    });
  }
}
