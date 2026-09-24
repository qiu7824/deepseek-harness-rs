import 'dart:collection';
import 'dart:convert';

import 'models.dart';
import 'tool_presentation.dart';
import 'turn_usage.dart';

class UploadedFileReceipt {
  const UploadedFileReceipt(this.name, this.path, this.size);
  final String name, path;
  final int size;
  static const referencePrefix = 'dsh-file-attachment:';
  static UploadedFileReceipt? fromAttachment(Object? value) {
    final ref = object(value),
        name = ref['name'],
        id = ref['attachmentId'],
        bytes = ref['bytes'];
    if (name is! String ||
        id is! String ||
        bytes is! int ||
        bytes < 0 ||
        bytes > 9007199254740991)
      return null;
    return UploadedFileReceipt(
      name,
      '$referencePrefix${Uri.encodeComponent(jsonEncode(ref))}',
      bytes,
    );
  }

  static Json? referenceFromPath(String path) {
    if (!path.startsWith(referencePrefix)) return null;
    return object(
      jsonDecode(Uri.decodeComponent(path.substring(referencePrefix.length))),
    );
  }

  static final _receipt = RegExp(
    r'^Attached file: ([^\r\n]+)\nPath: ([^\r\n]+)\nSize: (\d+) bytes$',
  );
  static final _managed = RegExp(
    r'(?:^|[\\/])\.dsh-attachments[\\/][a-f0-9]{64}[\\/][a-f0-9]{64}[\\/][^\\/]+$',
  );
  static UploadedFileReceipt? parse(String text) {
    if (text.length > 65536) return null;
    final match = _receipt.firstMatch(text);
    if (match == null ||
        match.end != text.length ||
        !_managed.hasMatch(match[2]!))
      return null;
    final bytes = int.tryParse(match[3]!);
    if (bytes == null || bytes < 0 || bytes > 9007199254740991) return null;
    return UploadedFileReceipt(match[1]!, match[2]!, bytes);
  }

  String get displaySize => size < 1024
      ? '$size B'
      : size < 1048576
      ? '${(size / 1024).toStringAsFixed(1)} KiB'
      : '${(size / 1048576).toStringAsFixed(1)} MiB';
}

class TranscriptItem {
  TranscriptItem({
    required this.id,
    required this.kind,
    required this.text,
    this.title = '',
    this.streaming = false,
    this.images = const [],
    this.files = const [],
    this.originalText,
    this.summary = '',
    this.output = '',
    this.status = '',
    this.iconKind = '',
    this.messageId,
    this.filePath,
    this.planText,
    this.time,
    this.seq,
    this.turnUsage,
    this.runMs,
    this.ttftMs,
    this.tokensPerSecond,
  });
  final String id, kind, text, title;
  final String summary, output, status, iconKind;
  final String? messageId;
  final String? filePath;
  final String? planText;
  final int? time;
  final int? seq;
  final Json? turnUsage;
  final int? runMs, ttftMs;
  final double? tokensPerSecond;
  final bool streaming;
  final List<Json> images;
  final List<UploadedFileReceipt> files;
  final String? originalText;
  String get clipboardText => originalText ?? text;
}

String contentText(Object? content) => objects(content)
    .map((block) {
      switch (block['type']) {
        case 'text':
        case 'reasoning':
          return block['text'] as String? ?? '';
        case 'tool-result':
          return contentText(block['content']);
        case 'image':
          return '[图片：${object(block['attachment'])['name'] ?? '附件'}]';
        case 'file':
          return '[文件：${object(block['attachment'])['name'] ?? '附件'}]';
        default:
          return '';
      }
    })
    .where((s) => s.isNotEmpty)
    .join('\n\n');

/// Bounded event window. Reading an older page never accumulates live events.
class ConversationWindow {
  ConversationWindow({this.maxEvents = 8192, this.maxBytes = 8 * 1024 * 1024});
  final int maxEvents, maxBytes;
  final _events = SplayTreeMap<int, HistoryEvent>();
  final _sizes = <int, int>{};
  int _bytes = 0;
  int revision = 0;
  bool hasBefore = false, hasAfter = false, needsRefresh = false;
  int? oversizedEventSeq;
  int? firstSeq, lastSeq;
  Iterable<HistoryEvent> get events => _events.values;
  int get retainedBytes => _bytes;
  int get eventCount => _events.length;
  void mergePage(HistoryPage page, {required bool older}) {
    revision++;
    for (final event in page.events) {
      _insert(event);
    }
    if (older) {
      hasBefore = page.hasBefore;
      firstSeq = page.firstSeq;
    } else {
      hasAfter = page.hasAfter;
      lastSeq = page.lastSeq;
    }
    while (_events.isNotEmpty &&
        (_events.length > maxEvents || _bytes > maxBytes)) {
      final key = older ? _events.lastKey()! : _events.firstKey()!;
      _events.remove(key);
      _bytes -= _sizes.remove(key)!;
      if (older) {
        hasAfter = true;
        lastSeq = _events.values.lastOrNull?.endSeq;
      } else {
        hasBefore = true;
        firstSeq = _events.values.firstOrNull?.startSeq;
      }
    }
  }

  void replace(HistoryPage page) {
    revision++;
    _events.clear();
    _sizes.clear();
    _bytes = 0;
    hasBefore = page.hasBefore;
    hasAfter = page.hasAfter;
    needsRefresh = false;
    oversizedEventSeq = null;
    firstSeq = page.firstSeq;
    lastSeq = page.lastSeq;
    for (final event in page.events) {
      _insert(event);
    }
    _bound();
  }

  bool append(HistoryEvent event) {
    if (event.endSeq <= (lastSeq ?? -1)) return false;
    if (hasAfter) {
      needsRefresh = true;
      return false;
    }
    if (lastSeq != null && event.startSeq > lastSeq! + 1) {
      needsRefresh = true;
      return false;
    }
    _insert(event);
    revision++;
    lastSeq = event.endSeq;
    firstSeq ??= event.startSeq;
    _bound();
    return true;
  }

  void _insert(HistoryEvent event) {
    final size = event.retainedBytes;
    if (size > maxBytes) {
      // This event cannot fit even in an empty window. Retrying the same
      // snapshot would loop forever and must not evict valid neighboring rows.
      oversizedEventSeq = event.seq;
      return;
    }
    _bytes -= _sizes[event.seq] ?? 0;
    _events[event.seq] = event;
    _sizes[event.seq] = size;
    _bytes += size;
  }

  void _bound() {
    while (_events.length > maxEvents || _bytes > maxBytes) {
      final first = _events.firstKey()!;
      _events.remove(first);
      _bytes -= _sizes.remove(first)!;
      hasBefore = true;
    }
    if (_events.isNotEmpty) firstSeq = _events.values.first.startSeq;
  }

  List<TranscriptItem> project() => projectTranscript(events, live: !hasAfter);
}

List<TranscriptItem> projectTranscript(
  Iterable<HistoryEvent> source, {
  bool live = true,
}) {
  final events = source.toList();
  final compactions = <String, List<HistoryEvent>>{};
  for (final event in events) {
    if ([
          'compaction/start',
          'compaction/summary',
          'compaction/end',
        ].contains(event.type) &&
        event.data['compactionId'] is String) {
      compactions
          .putIfAbsent(event.data['compactionId'] as String, () => [])
          .add(event);
    }
  }
  final turnEvents = <Object, List<HistoryEvent>>{};
  for (final event in events) {
    final turn = event.data['turn'];
    if (turn != null) turnEvents.putIfAbsent(turn, () => []).add(event);
  }
  String callKey(HistoryEvent e, {bool result = false}) =>
      '${e.data['turn']}:${result ? object(object(e.data['message'])['source'])['callId'] ?? e.data['callId'] : e.data['callId']}';
  final results = {
    for (final e in events)
      if (e.type == 'tool/result') callKey(e, result: true): e,
  };
  final calls = {
    for (final e in events)
      if (e.type == 'tool/call') callKey(e),
  };
  final ended = {
    for (final e in events)
      if (e.type == 'turn/end') e.data['turn'],
  };
  final commands = {
    for (final e in events)
      if (e.type == 'command/run')
        e.data['commandId']: e.data['name'] ?? e.data['command'] ?? '命令',
  };
  String stepKey(HistoryEvent e) => '${e.data['turn']}:${e.data['step']}';
  final completed = {
    for (final e in events)
      if (e.type == 'assistant/message') stepKey(e),
  };
  final closing = <Object, HistoryEvent>{};
  final retryFirst = <String, int>{};
  final retries = <String, HistoryEvent>{};
  final retryStarted = <String>{};
  final closedSteps = <String>{};
  for (final event in events) {
    final id = '${event.data['turn']}:${event.data['retryId']}';
    if (event.type == 'llm/retry' && event.data['retryId'] is String) {
      retryFirst.putIfAbsent(id, () => event.seq);
      retries[id] = event;
    } else if (event.type == 'llm/retry-started') {
      retryStarted.add('$id:${event.data['retry']}');
    } else if (event.type == 'step/end') {
      closedSteps.add(stepKey(event));
    }
  }
  final latestTranscript = <Object, int>{};
  for (final event in events) {
    final turn = event.data['turn'];
    if (turn == null) continue;
    final append =
        event.raw['surfaceOp'] == null || event.raw['surfaceOp'] == 'append';
    if (event.type == 'assistant/message' &&
        append &&
        objects(object(event.data['message'])['content']).any(
          (b) => b['type'] == 'text' && '${b['text'] ?? ''}'.trim().isNotEmpty,
        ))
      closing[turn] = event;
    if (append &&
            [
              'assistant/message',
              'tool/call',
              'tool/result',
              'llm/retry',
            ].contains(event.type) ||
        event.type == 'turn/end' &&
            object(event.data['reason'])['kind'] == 'error')
      latestTranscript[turn] = event.seq;
  }
  final chunks = <String, StringBuffer>{};
  final chunkPositions = <String, int>{};
  final activeChunk = <String, String>{}, chunkStep = <String, String>{};
  final chunkTurn = <String, Object?>{};
  final items = <({int seq, TranscriptItem item})>[];
  for (final event in events) {
    final data = event.data;
    final seq = event.seq;
    void add(
      String kind,
      String text, {
      String title = '',
      String suffix = '',
      List<Json> images = const [],
      List<UploadedFileReceipt> files = const [],
      String? originalText,
      String summary = '',
      String output = '',
      String status = '',
      String iconKind = '',
      String? messageId,
      String? filePath,
      String? planText,
      String? id,
    }) {
      if (text.isNotEmpty ||
          title.isNotEmpty ||
          images.isNotEmpty ||
          files.isNotEmpty) {
        items.add((
          seq: seq,
          item: TranscriptItem(
            id: id ?? '$seq$suffix',
            kind: kind,
            text: text,
            title: title,
            images: images,
            files: files,
            originalText: originalText,
            summary: summary,
            output: output,
            status: status,
            iconKind: iconKind,
            messageId: messageId,
            filePath: filePath,
            planText: planText,
            time: event.raw['time'] is num
                ? (event.raw['time'] as num).toInt()
                : null,
            seq: event.seq,
          ),
        ));
      }
    }

    switch (event.type) {
      case 'user/message':
        // Host context and skill injections use the user role but are not
        // messages authored by the person using the client.
        final sourceKind = object(data['source'])['kind'];
        final messageSource = object(data['source']);
        if (sourceKind == 'compact-checkpoint' ||
            messageSource['plugin'] == 'compact') {
          if (!compactions.containsKey(messageSource['compactionId'])) {
            add(
              'compaction',
              contentText(data['content']),
              title: '上下文压缩摘要',
              status: 'complete',
            );
          }
          break;
        }
        if (sourceKind != null && sourceKind != 'user') {
          add(
            'context',
            contentText(data['content']),
            title: '上下文注入',
            summary:
                '${messageSource['plugin'] ?? messageSource['name'] ?? sourceKind ?? ''}',
            iconKind: 'context',
          );
          break;
        }
        final blocks = objects(data['content']),
            files = <UploadedFileReceipt>[],
            visible = <Json>[];
        for (final block in blocks) {
          final file = block['type'] == 'file'
              ? UploadedFileReceipt.fromAttachment(block['attachment'])
              : block['type'] == 'text' && block['text'] is String
              ? UploadedFileReceipt.parse(block['text'] as String)
              : null;
          if (file == null) {
            if (block['type'] != 'image') visible.add(block);
          } else {
            files.add(file);
          }
        }
        add(
          'user',
          contentText(visible),
          files: files,
          originalText: files.isEmpty
              ? null
              : contentText(blocks.where((b) => b['type'] != 'image').toList()),
          images: objects(data['content'])
              .where((b) => b['type'] == 'image')
              .map((b) => object(b['attachment']))
              .toList(),
        );
      case 'assistant/message':
        final storedId = object(data['message'])['id'];
        final messageId =
            storedId is String &&
                storedId.isNotEmpty &&
                (event.raw['surfaceOp'] == null ||
                    event.raw['surfaceOp'] == 'append')
            ? storedId
            : null;
        for (final (index, block) in objects(
          object(data['message'])['content'],
        ).indexed) {
          if (block['type'] == 'text' || block['type'] == 'reasoning') {
            add(
              block['type'] == 'reasoning' ? 'reasoning' : 'assistant',
              block['text'] as String? ?? '',
              suffix: ':$index',
              messageId: messageId,
              id:
                  data['turn'] != null &&
                      data['step'] != null &&
                      (event.raw['surfaceOp'] == null ||
                          event.raw['surfaceOp'] == 'append')
                  ? 'assistant:${stepKey(event)}:$index:${block['type']}'
                  : null,
            );
          } else if (block['type'] == 'image') {
            add(
              'assistant',
              '',
              suffix: ':$index',
              images: [object(block['attachment'])],
            );
          }
        }
      case 'assistant/chunk':
        if (completed.contains(stepKey(event))) break;
        final chunk = object(data['chunk']);
        if (!['text-delta', 'reasoning-delta'].contains(chunk['type'])) break;
        final key = '${stepKey(event)}:${chunk['index']}:${chunk['type']}';
        activeChunk[stepKey(event)] = key;
        chunkStep[key] = stepKey(event);
        chunkTurn[key] = data['turn'];
        chunks
            .putIfAbsent(key, () => StringBuffer())
            .write(chunk['text'] ?? '');
        chunkPositions.putIfAbsent(key, () => seq);
      case 'tool/call':
        final view = object(event.view?['view']),
            result = results[callKey(event)],
            resultData = result?.data ?? <String, dynamic>{};
        final failed =
            resultData['error'] != null ||
            object(resultData['message'])['isError'] == true ||
            objects(
              object(resultData['message'])['content'],
            ).any((b) => b['isError'] == true);
        final name = '${data['name'] ?? '工具调用'}';
        final errorCode = object(resultData['error'])['code'] as String?;
        final status = result != null
            ? (errorCode == 'ASK_ABORTED' || errorCode == 'interrupted'
                  ? 'interrupted'
                  : failed
                  ? 'failed'
                  : 'complete')
            : ended.contains(data['turn'])
            ? 'interrupted'
            : live
            ? 'pending'
            : '';
        final output = contentText(object(resultData['message'])['content']);
        final presentation = toolPresentation(
          name: name,
          args: event.toolSummaryArguments,
          view: view,
          status: status,
          output: output,
          errorCode: errorCode,
        );
        add(
          'tool',
          data['arguments'] as String? ?? '',
          title: event.planText == null
              ? presentation.title
              : planHeading(event.planText!),
          summary: presentation.summary,
          filePath: presentation.filePath,
          planText: event.planText,
          output: output,
          status: status,
          iconKind: presentation.kind,
        );
      case 'tool/result':
        if (calls.contains(callKey(event, result: true))) break;
        final message = object(data['message']);
        final failed =
            data['error'] != null ||
            message['isError'] == true ||
            objects(message['content']).any(
              (block) =>
                  block['type'] == 'tool-result' && block['isError'] == true,
            );
        add(
          'result',
          contentText(object(data['message'])['content']),
          title: failed ? '工具执行失败' : '工具结果',
          status: failed ? 'failed' : 'complete',
        );
      case 'system/message':
        if (data['prefix'] != true)
          add(
            'context',
            contentText(object(data['message'])['content']),
            title: '系统提示词',
            summary:
                '${object(object(data['message'])['source'])['plugin'] ?? object(object(data['message'])['source'])['kind'] ?? ''}',
            iconKind: 'system',
          );
      case 'command/done':
        add(
          'context',
          '${data['text'] ?? ''}',
          title: '${commands[data['commandId']] ?? '命令'}',
          summary: '${data['text'] ?? ''}',
          iconKind: 'command',
          status: data['kind'] == 'error' ? 'failed' : 'complete',
        );
      case 'llm/retry':
        final id = '${data['turn']}:${data['retryId']}';
        if (retryFirst[id] != seq) break;
        final current = retries[id]!, retry = current.data;
        final state = retryStarted.contains('$id:${retry['retry']}')
            ? 'started'
            : ended.contains(retry['turn']) ||
                  closedSteps.contains(stepKey(current))
            ? 'cancelled'
            : 'scheduled';
        add(
          'retry',
          '',
          id: 'retry:$id',
          title: '模型请求重试',
          status: state,
          output: jsonEncode({
            'retry': retry['retry'],
            'maximum': retry['mode'] == 'normal' ? retry['maxRetries'] : '∞',
            'delayMs': retry['delayMs'],
            'failure': retry['failure'],
          }),
        );
      case 'turn/end':
        final reason = object(data['reason']);
        if (reason['kind'] == 'aborted' &&
            object(reason['reason'])['kind'] == 'user') {
          add('notice', '任务已停止。');
        } else if (reason['kind'] == 'error' || reason['kind'] == 'aborted') {
          add('error', jsonEncode(reason), title: '执行结束');
        }
      case 'agent/error':
        add('error', data['message'] as String? ?? jsonEncode(data));
    }
  }
  for (final entry in chunks.entries) {
    items.add((
      seq: chunkPositions[entry.key]!,
      item: TranscriptItem(
        id: 'assistant:${entry.key.replaceFirst(RegExp(r'-delta$'), '')}',
        kind: entry.key.endsWith('reasoning-delta') ? 'reasoning' : 'assistant',
        text: entry.value.toString(),
        streaming:
            live &&
            !ended.contains(chunkTurn[entry.key]) &&
            (!entry.key.endsWith('reasoning-delta') ||
                activeChunk[chunkStep[entry.key]] == entry.key),
      ),
    ));
  }
  for (final entry in closing.entries) {
    if (!ended.contains(entry.key)) continue;
    final event = entry.value, message = object(entry.value.data['message']);
    final metrics = deriveTurnFooterMetrics(turnEvents[entry.key] ?? const []);
    final text = objects(message['content'])
        .where((b) => b['type'] == 'text')
        .map((b) => '${b['text'] ?? ''}')
        .join('\n\n');
    items.add((
      seq: event.seq,
      item: TranscriptItem(
        id: 'turn-tail:${entry.key}',
        kind: 'turn-tail',
        text: text,
        seq: event.seq,
        time: (event.raw['time'] as num?)?.toInt(),
        messageId: message['id'] is String ? message['id'] as String : null,
        turnUsage: metrics.usage,
        runMs: metrics.runMs,
        ttftMs: metrics.ttftMs,
        tokensPerSecond: metrics.tokensPerSecond,
        status: latestTranscript[entry.key] == event.seq
            ? 'complete'
            : 'branch-unavailable',
      ),
    ));
  }
  for (final entry in compactions.entries) {
    final summary = entry.value
        .where((e) => e.type == 'compaction/summary')
        .lastOrNull;
    final end = entry.value.where((e) => e.type == 'compaction/end').lastOrNull;
    final failure = end?.data['error'];
    final status = failure != null
        ? 'failed'
        : end != null
        ? 'complete'
        : live
        ? 'pending'
        : 'incomplete';
    items.add((
      seq: entry.value.first.seq,
      item: TranscriptItem(
        id: 'compaction:${entry.key}',
        kind: 'compaction',
        seq: summary?.seq ?? entry.value.first.seq,
        title: status == 'pending'
            ? '正在压缩…'
            : status == 'failed'
            ? '上下文压缩未完成'
            : '上下文压缩摘要',
        status: status,
        text: summary == null
            ? (failure == null ? '' : '$failure')
            : contentText(summary.data['summary']),
      ),
    ));
  }
  final ordered = items.indexed.toList()
    ..sort((a, b) {
      final seq = a.$2.seq.compareTo(b.$2.seq);
      return seq != 0 ? seq : a.$1.compareTo(b.$1);
    });
  return ordered.map((e) => e.$2.item).toList();
}
