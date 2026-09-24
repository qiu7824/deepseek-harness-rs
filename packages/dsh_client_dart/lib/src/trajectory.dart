import 'dart:math' as math;

import 'models.dart';

/// A semantic operation backed by references into the bounded history window.
/// Payloads are never copied into a second unbounded transcript.
class TraceRecord {
  TraceRecord({
    required this.id,
    required this.kind,
    required this.anchorSeq,
    required this.turn,
    required this.step,
    this.title = '',
    this.callId,
    this.parentCallId,
    this.startTime,
    this.endTime,
    this.status = 'running',
  });
  final String id, kind;
  final int turn, step;
  String title, status;
  final String? callId, parentCallId;
  int anchorSeq;
  int? startTime, endTime, firstTokenTime;
  HistoryEvent? header, result;
  final events = <HistoryEvent>[];
  int get lane => kind == 'assistant' || kind == 'compaction'
      ? 1
      : kind == 'tool' || kind == 'subtool'
      ? 2
      : 0;
  bool get failed => status == 'failed' || status == 'interrupted';
  int? get durationMs =>
      startTime == null || endTime == null || endTime! < startTime!
      ? null
      : endTime! - startTime!;
  String get role =>
      const {
        'system': '系统',
        'user': '用户',
        'context': '上下文',
        'assistant': '助手',
        'tool': '工具',
        'subtool': '子工具',
        'compaction': '压缩',
      }[kind] ??
      kind;

  String get argumentsPreview {
    final out = _Preview();
    out.add(
      events
          .where(
            (e) =>
                e.type == 'tool/call' || e.type == 'tool/code-dispatch-start',
          )
          .firstOrNull
          ?.data['arguments'],
    );
    return out.value;
  }

  String get resultPreview {
    final out = _Preview();
    out.add(
      result?.data['content'] ?? object(result?.data['message'])['content'],
    );
    return out.value;
  }

  String get preview {
    if (kind == 'system') return title;
    final out = _Preview();
    if (kind == 'tool' || kind == 'subtool') {
      out.add(title);
      final call = events
          .where(
            (e) =>
                e.type == 'tool/call' || e.type == 'tool/code-dispatch-start',
          )
          .firstOrNull;
      if (call != null) out.add(call.data['arguments']);
      if (result != null) {
        out.add(' → ');
        out.add(
          result!.data['content'] ?? object(result!.data['message'])['content'],
        );
      }
    } else if (result != null) {
      out.add(
        kind == 'assistant'
            ? objects(object(result!.data['message'])['content'])
                  .where(
                    (b) => ['text', 'reasoning', 'image'].contains(b['type']),
                  )
                  .toList()
            : kind == 'compaction'
            ? result!.data['summary']
            : result!.data['content'],
      );
    } else {
      for (final event in events) {
        if (out.full) break;
        if (event.type == 'assistant/chunk') {
          final chunk = object(event.data['chunk']);
          if (chunk['type'] == 'text-delta' ||
              chunk['type'] == 'reasoning-delta') {
            out.add(chunk['text'], separator: '');
          }
        }
      }
    }
    return out.value.isNotEmpty
        ? out.value
        : kind == 'assistant' && result != null
        ? '（仅工具调用）'
        : status == 'running'
        ? '待完成'
        : '无内容';
  }
}

/// Bounded, non-recursive preview of even deeply nested untrusted tool output.
class _Preview {
  final _out = StringBuffer();
  int _left = 512, _visited = 0;
  bool get full => _left <= 0 || _visited >= 128;
  String get value => _out.toString().trim();
  void add(Object? input, {String separator = ' '}) {
    if (full || input == null) return;
    final pending = <Object?>[input];
    while (pending.isNotEmpty && !full) {
      final value = pending.removeLast();
      _visited++;
      if (value is String) {
        final source = value.substring(0, math.min(value.length, 2048));
        final text = source.replaceAll(RegExp(r'\s+'), ' ').trim();
        if (text.isEmpty) continue;
        if (_out.isNotEmpty && separator.isNotEmpty && _left > 0) {
          _out.write(separator);
          _left--;
        }
        var length = math.min(text.length, _left);
        if (length > 0 &&
            length < text.length &&
            text.codeUnitAt(length) >= 0xdc00 &&
            text.codeUnitAt(length) <= 0xdfff) {
          length--;
        }
        _out.write(text.substring(0, length));
        _left -= length;
        if (length < text.length || source.length < value.length) {
          if (_left > 0) {
            _out.write('…');
            _left--;
          }
        }
      } else if (value is List) {
        for (var i = math.min(value.length, 128 - _visited) - 1; i >= 0; i--) {
          pending.add(value[i]);
        }
      } else if (value is Map) {
        if (value['type'] == 'image') {
          pending.add('[图片]');
        } else if (value['text'] is String) {
          pending.add(value['text']);
        } else if (value['content'] != null) {
          pending.add(value['content']);
        } else {
          for (final entry
              in value.entries
                  .take(math.max(0, (128 - _visited) ~/ 2))
                  .toList()
                  .reversed) {
            pending.add(
              entry.value is num || entry.value is bool
                  ? '${entry.value}'
                  : entry.value,
            );
            pending.add('${entry.key}:');
          }
        }
      }
    }
  }
}

int? _time(HistoryEvent e) {
  final t = e.raw['time'];
  return t is num && t.isFinite && t >= 0 ? t.toInt() : null;
}

int _number(Object? n) => n is num && n.isFinite ? n.toInt() : 0;

bool _sameStructure(Object? a, Object? b) {
  final pending = <(Object?, Object?)>[(a, b)];
  var examined = 0;
  while (pending.isNotEmpty) {
    if (++examined > 16384) return false;
    final (left, right) = pending.removeLast();
    if (identical(left, right)) continue;
    if (left is List && right is List) {
      if (left.length != right.length || pending.length + left.length > 16384) {
        return false;
      }
      for (var i = 0; i < left.length; i++) {
        pending.add((left[i], right[i]));
      }
    } else if (left is Map && right is Map) {
      if (left.length != right.length || pending.length + left.length > 16384) {
        return false;
      }
      for (final key in left.keys) {
        if (!right.containsKey(key)) return false;
        pending.add((left[key], right[key]));
      }
    } else if (left != right) {
      return false;
    }
  }
  return true;
}

class TraceSnapshot {
  TraceSnapshot._(this.records, this.eventCount);
  final List<TraceRecord> records;
  final int eventCount;

  factory TraceSnapshot.fromEvents(
    Iterable<HistoryEvent> source, {
    bool live = true,
  }) {
    final records = <TraceRecord>[],
        assistants = <String, TraceRecord>{},
        calls = <String, TraceRecord>{},
        compactions = <String, TraceRecord>{};
    final headers = <String, HistoryEvent>{};
    final open = <TraceRecord>{};
    HistoryEvent? previousHeader;
    int activeTurn = 0, activeStep = 0, count = 0;
    String stepKey(int turn, int step) => '$turn:$step';
    TraceRecord create(
      HistoryEvent e,
      String kind,
      int turn,
      int step, {
      String title = '',
      String? callId,
      String? parentCallId,
      bool started = true,
    }) {
      final row = TraceRecord(
        id: '$kind:${e.seq}',
        kind: kind,
        anchorSeq: e.seq,
        turn: turn,
        step: step,
        title: title,
        callId: callId,
        parentCallId: parentCallId,
        startTime: started ? _time(e) : null,
      );
      records.add(row);
      open.add(row);
      return row;
    }

    void settle(TraceRecord row, HistoryEvent e, String status) {
      row.endTime = _time(e);
      row.status = status;
      open.remove(row);
    }

    TraceRecord assistant(
      HistoryEvent e,
      int turn,
      int step, {
      bool started = false,
    }) {
      final key = stepKey(turn, step), current = assistants[key];
      if (current != null && current.status == 'running') return current;
      final row = create(e, 'assistant', turn, step, started: started);
      row.header = headers[key] ?? previousHeader;
      assistants[key] = row;
      return row;
    }

    for (final e in source) {
      count++;
      final d = e.data, t = _time(e);
      final turn = d['turn'] is num ? _number(d['turn']) : activeTurn;
      final step = d['step'] is num ? _number(d['step']) : activeStep;
      final key = stepKey(turn, step);
      switch (e.type) {
        case 'turn/start':
          activeTurn = turn;
          activeStep = 0;
        case 'step/start':
          activeStep = step;
          final row = assistant(e, turn, step, started: true);
          row.events.add(e);
        case 'request/header':
          final header = object(d['header']),
              prior = object(previousHeader?.data['header']);
          final initial = previousHeader == null && d['reason'] == 'initial';
          if (initial ||
              previousHeader != null &&
                  (header['system'] != prior['system'] ||
                      !_sameStructure(header['tools'], prior['tools']))) {
            final row = create(
              e,
              'system',
              turn,
              step,
              title: previousHeader == null
                  ? 'Initial System Prompt'
                  : 'System Prompt / Tools changed',
            );
            row.header = e;
            if (initial && records.length > 1) {
              row.anchorSeq = records.first.anchorSeq - 1;
            }
            row.result = e;
            row.events.add(e);
            settle(row, e, 'complete');
          }
          previousHeader = e;
          headers[key] = e;
          assistants[key]?.header = e;
        case 'user/message':
          final context = object(d['source'])['kind'];
          final row = create(
            e,
            context == null || context == 'user' ? 'user' : 'context',
            turn,
            step,
          );
          row.events.add(e);
          row.result = e;
          settle(row, e, 'complete');
        case 'assistant/chunk':
          final row = assistant(e, turn, step);
          row.events.add(e);
          final type = object(d['chunk'])['type'];
          if ([
            'text-delta',
            'reasoning-delta',
            'tool-call-start',
          ].contains(type)) {
            row.firstTokenTime ??= t;
          }
        case 'assistant/message':
          final row = assistant(e, turn, step);
          row.events.add(e);
          row.result = e;
          row.anchorSeq = e.seq;
          settle(row, e, d['interrupted'] == true ? 'interrupted' : 'complete');
        case 'llm/retry':
          final row = assistant(e, turn, step);
          row.events.add(e);
          settle(row, e, 'failed');
        case 'request/phase':
          if (assistants[key]?.status == 'running') {
            assistants[key]!.events.add(e);
          }
        case 'step/end':
          final row = assistants[key];
          if (row != null) {
            row.events.add(e);
            if (row.status == 'running') settle(row, e, 'interrupted');
          }
        case 'tool/call':
        case 'tool/code-dispatch-start':
          final sub = e.type == 'tool/code-dispatch-start',
              callId =
                  '${d[e.type == 'tool/call' ? 'callId' : 'subCallId'] ?? e.seq}';
          final row = create(
            e,
            sub ? 'subtool' : 'tool',
            turn,
            step,
            title: '${d['name'] ?? '工具调用'}',
            callId: callId,
            parentCallId: sub ? d['parentCallId'] as String? : null,
          );
          row.events.add(e);
          calls['$turn:$callId'] = row;
        case 'tool/result':
        case 'tool/code-dispatch':
          final sub = e.type == 'tool/code-dispatch',
              message = object(d['message']);
          final callId =
              '${sub ? d['subCallId'] : object(message['source'])['callId'] ?? d['callId']}';
          final row =
              calls['$turn:$callId'] ??
              create(
                e,
                sub ? 'subtool' : 'tool',
                turn,
                step,
                title: '${d['name'] ?? '工具结果'}',
                callId: callId,
                parentCallId: sub ? d['parentCallId'] as String? : null,
                started: false,
              );
          row.events.add(e);
          row.result = e;
          final error =
              d['error'] != null ||
              d['isError'] == true ||
              objects(message['content']).any((b) => b['isError'] == true);
          settle(row, e, error ? 'failed' : 'complete');
        case 'compaction/start':
          final row = create(e, 'compaction', turn, step, title: '上下文压缩');
          row.events.add(e);
          compactions['${d['compactionId']}'] = row;
        case 'compaction/summary':
        case 'compaction/end':
          final row =
              compactions['${d['compactionId']}'] ??
              create(e, 'compaction', turn, step, started: false);
          compactions['${d['compactionId']}'] = row;
          row.events.add(e);
          if (e.type == 'compaction/summary') {
            row.result = e;
          } else {
            settle(row, e, d['error'] == null ? 'complete' : 'failed');
          }
        case 'turn/end':
        case 'session/end-seed':
          for (final row in open.toList()) {
            if (row.status == 'running' &&
                (e.type == 'session/end-seed' || row.turn == turn)) {
              settle(row, e, 'interrupted');
            }
          }
          activeTurn = 0;
          activeStep = 0;
      }
    }
    if (!live) {
      for (final row in open) {
        row.status = 'unavailable';
      }
    }
    records.sort((a, b) => a.anchorSeq.compareTo(b.anchorSeq));
    return TraceSnapshot._(List.unmodifiable(records), count);
  }

  List<TraceSpan> spans({required bool actualDuration, int? now}) {
    if (records.isEmpty) return [];
    int? end(TraceRecord r) =>
        r.endTime ?? (r.status == 'running' ? now : null);
    final times = records
        .expand((r) => [r.startTime, end(r)])
        .whereType<int>()
        .toList();
    final first = times.isEmpty ? 0 : times.reduce(math.min),
        last = times.isEmpty ? 0 : times.reduce(math.max);
    final range = math.max(1, last - first);
    return [
      for (final (index, row) in records.indexed)
        if (!actualDuration || row.startTime != null || row.endTime != null)
          TraceSpan(
            row,
            actualDuration
                ? ((row.startTime ?? row.endTime!) - first) / range
                : index / records.length,
            actualDuration
                ? ((end(row) ?? row.startTime!) - first) / range
                : (index + 1) / records.length,
          ),
    ];
  }
}

class TraceSpan {
  const TraceSpan(this.record, this.start, this.end);
  final TraceRecord record;
  final double start, end;
}

class TraceDisplayRow {
  const TraceDisplayRow(
    this.record, {
    this.summaryCount = 0,
    this.turnStart = false,
    this.summaryKind = 'turn',
  });
  final TraceRecord record;
  final int summaryCount;
  final bool turnStart;
  final String summaryKind;
  String get requestKey => '${record.turn}:${record.step}';
  String get id => summaryCount > 0
      ? '$summaryKind:${record.turn}:${summaryKind == 'turn' ? '' : record.step}'
      : record.id;
}

List<TraceDisplayRow> traceDisplayRows(
  TraceSnapshot snapshot, {
  String search = '',
  bool collapseTurns = false,
  bool collapseCalls = false,
  Set<int> expandedTurns = const {},
  Set<String> expandedRequests = const {},
}) {
  final query = search.trim().toLowerCase();
  final rows = snapshot.records
      .where(
        (r) =>
            (query.isEmpty ||
            '${r.role} ${r.title} ${r.preview}'.toLowerCase().contains(query)),
      )
      .toList();
  final counts = <int, int>{};
  bool request(TraceRecord row) =>
      ['assistant', 'tool', 'subtool', 'compaction'].contains(row.kind);
  final requests = <String, int>{};
  for (final row in rows) {
    counts.update(row.turn, (v) => v + 1, ifAbsent: () => 1);
    if (request(row)) {
      requests.update(
        '${row.turn}:${row.step}',
        (v) => v + 1,
        ifAbsent: () => 1,
      );
    }
  }
  final seen = <int>{};
  final seenRequests = <String>{}, output = <TraceDisplayRow>[];
  for (final row in rows) {
    final turnStart = seen.add(row.turn), key = '${row.turn}:${row.step}';
    if (collapseTurns && row.turn > 0 && !expandedTurns.contains(row.turn)) {
      if (turnStart) {
        output.add(
          TraceDisplayRow(
            row,
            summaryCount: counts[row.turn]!,
            turnStart: true,
          ),
        );
      }
    } else if (collapseCalls &&
        request(row) &&
        !expandedRequests.contains(key)) {
      if (seenRequests.add(key)) {
        output.add(
          TraceDisplayRow(
            row,
            summaryCount: requests[key]!,
            turnStart: turnStart,
            summaryKind: 'request',
          ),
        );
      }
    } else {
      output.add(TraceDisplayRow(row, turnStart: turnStart));
    }
  }
  return output;
}
