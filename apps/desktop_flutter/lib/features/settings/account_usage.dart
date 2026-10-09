enum AccountUsageStatus { fresh, stale, unsupported, unavailable, needsLogin }

class AccountUsageIdentityException implements Exception {
  const AccountUsageIdentityException();
}

class AccountUsageWindow {
  const AccountUsageWindow({
    required this.id,
    required this.label,
    required this.usedPercent,
    required this.remainingPercent,
    required this.resetsAt,
    required this.periodEndsAt,
    required this.windowDurationMins,
    required this.used,
    required this.remaining,
    required this.limit,
    required this.unit,
  });

  final String id, label;
  final double? usedPercent, remainingPercent, windowDurationMins;
  final int? resetsAt, periodEndsAt;
  final double? used, remaining, limit;
  final String unit;

  factory AccountUsageWindow.fromJson(Map<String, dynamic> json) {
    var used = _percentage(json['usedPercent']);
    var remaining = _percentage(json['remainingPercent']);
    // A malformed or inconsistent pair must not become a plausible progress bar.
    if ((json['usedPercent'] != null && used == null) ||
        (json['remainingPercent'] != null && remaining == null) ||
        (used != null &&
            remaining != null &&
            (used + remaining - 100).abs() > .1)) {
      used = null;
      remaining = null;
    } else {
      used ??= remaining == null ? null : 100 - remaining;
      remaining ??= used == null ? null : 100 - used;
    }
    final duration = _finiteNumber(json['windowDurationMins']);
    return AccountUsageWindow(
      id: _text(json['id'], 80) ?? '',
      label: _text(json['label'], 120) ?? '',
      usedPercent: used,
      remainingPercent: remaining,
      resetsAt: accountUsageUnixSeconds(json['resetsAt']),
      periodEndsAt: accountUsageUnixSeconds(json['periodEndsAt']),
      windowDurationMins: duration != null && duration > 0 ? duration : null,
      used: _amount(json['used']),
      remaining: _amount(json['remaining']),
      limit: _amount(json['limit']),
      unit: _text(json['unit'], 32) ?? 'unknown',
    );
  }
}

class AccountUsageSnapshot {
  const AccountUsageSnapshot({
    required this.provider,
    required this.accountScope,
    required this.status,
    required this.plan,
    required this.updatedAt,
    required this.windows,
    this.message,
    this.clearSnapshot = false,
  });

  final String provider, accountScope;
  final AccountUsageStatus status;
  final String? plan, message;
  final int? updatedAt;
  final List<AccountUsageWindow> windows;
  final bool clearSnapshot;

  factory AccountUsageSnapshot.fromJson(
    Map<String, dynamic> json, {
    required String expectedProvider,
    required String expectedScope,
  }) {
    if (json['provider'] != expectedProvider ||
        json['accountScope'] != expectedScope) {
      throw const AccountUsageIdentityException();
    }
    final status = switch (json['status']) {
      'fresh' => AccountUsageStatus.fresh,
      'stale' => AccountUsageStatus.stale,
      'unsupported' => AccountUsageStatus.unsupported,
      'unavailable' => AccountUsageStatus.unavailable,
      'needsLogin' => AccountUsageStatus.needsLogin,
      _ => throw const FormatException('Invalid account usage status'),
    };
    final rawWindows = json['windows'];
    if (rawWindows is! List || rawWindows.length > 32) {
      throw const FormatException('Invalid account usage windows');
    }
    final windows = <AccountUsageWindow>[];
    for (final value in rawWindows) {
      if (value is! Map || value.keys.any((key) => key is! String)) {
        throw const FormatException('Invalid account usage window');
      }
      windows.add(
        AccountUsageWindow.fromJson(Map<String, dynamic>.from(value)),
      );
    }
    return AccountUsageSnapshot(
      provider: expectedProvider,
      accountScope: expectedScope,
      status: status,
      plan: _text(json['plan'], 100),
      updatedAt: accountUsageUnixSeconds(json['updatedAt']),
      windows: List.unmodifiable(windows),
      message: _text(json['message'], 200),
      clearSnapshot: json['clearSnapshot'] == true,
    );
  }
}

double? _finiteNumber(Object? value) =>
    value is num && value.isFinite ? value.toDouble() : null;

double? _percentage(Object? value) {
  final number = _finiteNumber(value);
  return number != null && number >= 0 && number <= 100 ? number : null;
}

double? _amount(Object? value) {
  final number = _finiteNumber(value);
  return number != null && number >= 0 ? number : null;
}

String? _text(Object? value, int limit) {
  if (value is! String) return null;
  final text = value.trim();
  return text.isNotEmpty && text.length <= limit ? text : null;
}

int? accountUsageUnixSeconds(Object? value) {
  if (value is! num ||
      !value.isFinite ||
      value <= 0 ||
      value > 8640000000000 ||
      value != value.roundToDouble()) {
    return null;
  }
  return value.toInt();
}
