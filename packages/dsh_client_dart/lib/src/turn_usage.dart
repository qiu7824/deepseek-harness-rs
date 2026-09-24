import 'models.dart';

const _maxSafe = 9007199254740991;

int? _count(Object? value) {
  if (value is! num || !value.isFinite || value < 0 || value > _maxSafe) {
    return null;
  }
  final integer = value.toInt();
  return integer == value ? integer : null;
}

int? _sum(Iterable<int> values) {
  var total = 0;
  for (final value in values) {
    total += value;
    if (total > _maxSafe) return null;
  }
  return total;
}

Json? _normalizeAttempt(Json sample, Json? route) {
  final input = _count(sample['inputTokens']);
  final output = _count(sample['outputTokens']);
  final read = sample.containsKey('cacheReadTokens')
      ? _count(sample['cacheReadTokens'])
      : null;
  final write = sample.containsKey('cacheWriteTokens')
      ? _count(sample['cacheWriteTokens'])
      : null;
  final reasoning = sample.containsKey('reasoningTokens')
      ? _count(sample['reasoningTokens'])
      : null;
  if (input == null ||
      output == null ||
      (sample.containsKey('cacheReadTokens') && read == null) ||
      (sample.containsKey('cacheWriteTokens') && write == null) ||
      (sample.containsKey('reasoningTokens') &&
          (reasoning == null || reasoning > output))) {
    return null;
  }
  final known = _sum([input, ?read, ?write]);
  if (known == null) return null;
  final explicit = sample.containsKey('totalTokens')
      ? _count(sample['totalTokens'])
      : null;
  int? total;
  if (sample.containsKey('totalTokens')) {
    if (explicit == null || explicit < output) return null;
    final prompt = explicit - output;
    if (prompt < known || (read != null && write != null && prompt != known)) {
      return null;
    }
    total = explicit;
  } else {
    if (read == null || write == null) return null;
    total = _sum([known, output]);
  }
  if (total == null) return null;
  return {
    'inputTokens': input,
    'outputTokens': output,
    'totalTokens': total,
    'cacheReadTokens': ?read,
    'cacheWriteTokens': ?write,
    'reasoningTokens': ?reasoning,
    'route': ?route,
  };
}

Json? _aggregate(List<Json> attempts) {
  if (attempts.isEmpty) return null;
  final input = _sum(attempts.map((a) => a['inputTokens'] as int));
  final output = _sum(attempts.map((a) => a['outputTokens'] as int));
  final total = _sum(attempts.map((a) => a['totalTokens'] as int));
  if (input == null || output == null || total == null) return null;
  int? optional(String key) => attempts.every((a) => a[key] is int)
      ? _sum(attempts.map((a) => a[key] as int))
      : null;
  final routes = <String, Json>{};
  final attributed = attempts.every((a) => a['route'] is Map);
  if (attributed) {
    for (final attempt in attempts) {
      final route = object(attempt['route']);
      routes['${route['provider']}\u0000${route['model']}'] = route;
    }
  }
  return {
    'uncachedInputTokens': input,
    'outputTokens': output,
    'totalTokens': total,
    if (optional('cacheReadTokens') case final int value)
      'cacheReadTokens': value,
    if (optional('cacheWriteTokens') case final int value)
      'cacheWriteTokens': value,
    if (optional('reasoningTokens') case final int value)
      'reasoningTokens': value,
    if (attributed) 'routes': routes.values.toList(),
  };
}

Json? deriveTurnUsage(Iterable<HistoryEvent> source) {
  var state = 'idle', settledBy = '';
  Object? turn, step;
  Json? sample;
  var ended = false;
  final attempts = <Json>[];

  bool close([Json? route]) {
    if (state != 'open' || sample == null) return false;
    final normalized = _normalizeAttempt(sample!, route);
    if (normalized == null) return false;
    attempts.add(normalized);
    sample = null;
    return true;
  }

  bool same(HistoryEvent event) =>
      event.data['turn'] == turn && event.data['step'] == step;
  for (final event in source) {
    final data = event.data;
    if (event.type == 'turn/start') {
      if (turn != null || state != 'idle') return null;
      turn = data['turn'];
      if (turn == null) return null;
      continue;
    }
    if (turn == null) return null;
    if (event.type == 'turn/end') {
      if (data['turn'] != turn || state != 'idle' || ended) return null;
      ended = true;
      continue;
    }
    if (ended) return null;
    switch (event.type) {
      case 'step/start':
        if (data['turn'] != turn || state != 'idle') return null;
        state = 'open';
        step = data['step'];
        sample = null;
      case 'llm/retry-started':
        if (state != 'settled' || settledBy != 'retry' || !same(event)) {
          return null;
        }
        state = 'open';
        sample = null;
      case 'assistant/chunk':
        if (state != 'open' || !same(event)) return null;
        final chunk = object(data['chunk']);
        if (chunk['type'] == 'usage') {
          sample = object(chunk['usage']);
        } else if (chunk['type'] == 'finish' &&
            ['error', 'aborted'].contains(object(chunk['reason'])['kind'])) {
          if (!close()) return null;
          state = 'finishClosed';
        }
      case 'assistant/message':
        if (state != 'open' || !same(event)) return null;
        if (data['usage'] != null) sample = object(data['usage']);
        final source = object(object(data['message'])['source']);
        final provider = source['provider'], model = source['model'];
        final route =
            provider is String &&
                provider.isNotEmpty &&
                model is String &&
                model.isNotEmpty
            ? {'provider': provider, 'model': model}
            : null;
        if (!close(route)) return null;
        state = 'settled';
        settledBy = 'message';
      case 'llm/retry':
        if (state == 'idle' || !same(event) || state == 'settled') {
          return null;
        }
        if (state == 'open' && !close()) return null;
        state = 'settled';
        settledBy = 'retry';
      case 'step/end':
        if (state == 'idle' || !same(event)) return null;
        if (state == 'open' && !close()) return null;
        state = 'idle';
        step = null;
        sample = null;
    }
  }
  return ended && state == 'idle' ? _aggregate(attempts) : null;
}

({Json? usage, int? runMs, int? ttftMs, double? tokensPerSecond})
deriveTurnFooterMetrics(Iterable<HistoryEvent> source) {
  final events = source.toList();
  int? start, end, firstStep, ttft;
  var networkMs = 0, outputTokens = 0;
  for (final event in events) {
    final timestamp = _count(event.raw['time']);
    if (event.type == 'turn/start') start = timestamp;
    if (event.type == 'turn/end') end = timestamp;
    if (event.type != 'assistant/message') continue;
    final step = _count(event.data['step']);
    final timing = object(event.data['requestMetrics']);
    if (step != null && (firstStep == null || step < firstStep)) {
      firstStep = step;
      ttft = _count(timing['firstTokenMs']);
    }
    final measured = object(timing['requestMeasurement']);
    final ms = _count(measured['networkElapsedMs']);
    final tokens = _count(measured['outputTokens']);
    if (measured['phase'] == 'completed' &&
        measured['measurement'] == 'request-average' &&
        measured['attemptId'] is String &&
        measured['executionInstanceId'] is String &&
        ms != null &&
        ms > 0 &&
        tokens != null &&
        _sum([networkMs, ms]) != null &&
        _sum([outputTokens, tokens]) != null) {
      networkMs += ms;
      outputTokens += tokens;
    }
  }
  return (
    usage: deriveTurnUsage(events),
    runMs: start != null && end != null && end >= start ? end - start : null,
    ttftMs: ttft,
    tokensPerSecond: networkMs > 0 ? outputTokens * 1000 / networkMs : null,
  );
}
