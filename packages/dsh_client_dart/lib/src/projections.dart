import 'dart:convert';
import 'dart:math';
import 'models.dart';

class PlanModeProjection {
  const PlanModeProjection(this.active, this.pending);
  final bool active, pending;
  bool get requestedActive => pending ? !active : active;
  static PlanModeProjection? from(Object? value) {
    final data = object(value);
    return data['active'] is bool && data['pending'] is bool
        ? PlanModeProjection(data['active'] as bool, data['pending'] as bool)
        : null;
  }
}

/// Latest registered display projections, independent of the paged transcript.
class ProjectionWindow {
  ProjectionWindow({this.maxBytes = 2 * 1024 * 1024});
  static const keys = {
    'sessionStats',
    'sessionListMetadata',
    'tokenUsage',
    'contextPressure',
    'contextBreakdown',
    'contextInsights',
    'todos',
    'goal',
    'modelSelection',
    'permissions',
    'plan',
    'subagentTiming',
    'userMessageRail',
  };
  final int maxBytes;
  final Json values = {};
  final Set<String> oversized = {};
  final _sequences = <String, int>{},
      _versions = <String, int>{},
      _sizes = <String, int>{};
  int version = 0, retainedBytes = 0, _snapshotSeq = -1;
  int revisionOf(String key) => _versions[key] ?? 0;
  bool truncate(int lastSeq) {
    var changed = false;
    for (final key in _sequences.keys.toList()) {
      if (_sequences[key]! <= lastSeq) continue;
      retainedBytes -= _sizes.remove(key) ?? 0;
      values.remove(key);
      oversized.remove(key);
      _sequences.remove(key);
      _versions.remove(key);
      changed = true;
    }
    if (_snapshotSeq > lastSeq) {
      _snapshotSeq = lastSeq;
      changed = true;
    }
    if (changed) version++;
    return changed;
  }

  void clear() {
    values.clear();
    oversized.clear();
    _sequences.clear();
    _versions.clear();
    _sizes.clear();
    retainedBytes = 0;
    version = 0;
    _snapshotSeq = -1;
  }

  bool apply(String key, Object? value, int seq) {
    if (!keys.contains(key) || seq < max(_snapshotSeq, _sequences[key] ?? -1)) {
      return false;
    }
    _store(key, value, seq);
    return true;
  }

  void snapshot(Json block, {required int requestVersion}) {
    if (block['asOfSeq'] is! num || block['values'] is! Map) return;
    final seq = (block['asOfSeq'] as num).toInt(),
        next = object(block['values']);
    if (seq < _snapshotSeq) return;
    for (final key in keys) {
      final current = _sequences[key] ?? -1;
      if (current > seq ||
          (current == seq && (_versions[key] ?? 0) > requestVersion)) {
        continue;
      }
      _store(key, next[key], seq);
    }
    _snapshotSeq = seq;
  }

  void _store(String key, Object? value, int seq) {
    retainedBytes -= _sizes.remove(key) ?? 0;
    values.remove(key);
    oversized.remove(key);
    _sequences[key] = seq;
    _versions[key] = ++version;
    if (value == null) return;
    final size = jsonEncode(value).length * 2;
    if (size + retainedBytes > maxBytes) {
      oversized.add(key);
      return;
    }
    values[key] = value;
    _sizes[key] = size;
    retainedBytes += size;
  }
}

num nonNegative(Json value, String key) {
  final number = value[key];
  return number is num && number.isFinite && number >= 0 ? number : 0;
}

class ContextOccupancy {
  const ContextOccupancy(this.used, this.capacity, this.estimated);
  final num used, capacity;
  final bool estimated;
  double get ratio => (used / capacity).clamp(0.0, 1.0).toDouble();
  int get percent => (ratio * 100).round();
  static ContextOccupancy? fromJson(Json value) {
    final used = value['projectedTokens'] ?? value['pressureTokens'],
        capacity = value['contextWindow'];
    if (used is! num ||
        capacity is! num ||
        !used.isFinite ||
        !capacity.isFinite ||
        used < 0 ||
        capacity <= 0) {
      return null;
    }
    return ContextOccupancy(
      used,
      capacity,
      value['contextWindowEstimated'] == true,
    );
  }
}

num billedInputTokens(Json usage) =>
    nonNegative(usage, 'uncachedInputTokens') +
    nonNegative(usage, 'cacheReadTokens') +
    nonNegative(usage, 'cacheWriteTokens');
int? cacheHitPercent(Json usage) {
  final stats = object(usage['cacheStatistics']);
  final denominator = nonNegative(stats, 'reportedInputTokens');
  if (nonNegative(stats, 'reportedSamples') == 0 || denominator == 0) {
    return null;
  }
  return (nonNegative(usage, 'cacheReadTokens') / denominator * 100)
      .clamp(0, 100)
      .round();
}
