/// Owns the resources of one screen/request generation.
class RequestScope {
  bool _cancelled = false;
  final _callbacks = <void Function()>{};
  bool get cancelled => _cancelled;
  void Function() register(void Function() cancel) {
    if (_cancelled) {
      cancel();
      return () {};
    }
    _callbacks.add(cancel);
    return () => _callbacks.remove(cancel);
  }

  void cancel() {
    if (_cancelled) return;
    _cancelled = true;
    for (final callback in _callbacks.toList()) {
      callback();
    }
    _callbacks.clear();
  }
}

/// Weighted LRU with explicit disposal; an oversized item is never retained.
class ResourceCache<K, V> {
  ResourceCache({
    required this.maxBytes,
    required this.maxEntries,
    required this.sizeOf,
    this.onEvict,
  });
  final int maxBytes, maxEntries;
  final int Function(V) sizeOf;
  final void Function(V)? onEvict;
  final _items = <K, V>{};
  int _bytes = 0;
  int get bytes => _bytes;
  int get length => _items.length;
  V? get(K key) {
    final item = _items.remove(key);
    if (item != null) _items[key] = item;
    return item;
  }

  bool put(K key, V value) {
    remove(key);
    final size = sizeOf(value);
    if (size > maxBytes || maxEntries == 0) return false;
    while (_items.isNotEmpty &&
        (_items.length >= maxEntries || _bytes + size > maxBytes)) {
      remove(_items.keys.first);
    }
    _items[key] = value;
    _bytes += size;
    return true;
  }

  void remove(K key) {
    final previous = _items.remove(key);
    if (previous != null) {
      _bytes -= sizeOf(previous);
      onEvict?.call(previous);
    }
  }

  void clear() {
    for (final key in _items.keys.toList()) {
      remove(key);
    }
  }
}
