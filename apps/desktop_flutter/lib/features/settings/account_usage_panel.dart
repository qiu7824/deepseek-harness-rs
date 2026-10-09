import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';
import '../../l10n/account_usage_zh.dart';
import '../../src/controller.dart';
import 'account_usage.dart';

class AccountUsageDisclosure extends StatefulWidget {
  const AccountUsageDisclosure({
    super.key,
    required this.controller,
    required this.api,
    required this.provider,
    required this.accountScope,
    required this.visible,
    this.needsLogin = false,
    this.compact = false,
  });

  final DesktopController controller;
  final DshClient api;
  final String provider, accountScope;
  final bool visible, needsLogin, compact;

  @override
  State<AccountUsageDisclosure> createState() => _AccountUsageDisclosureState();
}

class _AccountUsageDisclosureState extends State<AccountUsageDisclosure> {
  bool expanded = false;
  Object? identity;

  Object get currentIdentity {
    final provider = widget.controller.subscriptionAccounts
        .where((row) => row['id'] == widget.provider)
        .firstOrNull;
    final active = objects(provider?['accounts'])
        .where((row) => row['active'] == true)
        .firstOrNull;
    return (
      widget.controller,
      widget.api,
      widget.controller.client,
      widget.controller.host,
      widget.provider,
      widget.accountScope,
      widget.needsLogin,
      provider?['signedIn'],
      provider?['accountScope'],
      active?['accountScope'],
      active?['needsLogin'],
    );
  }

  @override
  void initState() {
    super.initState();
    identity = currentIdentity;
    widget.controller.addListener(contextChanged);
  }

  void contextChanged() {
    if (!mounted || identity == currentIdentity) return;
    setState(() {
      expanded = false;
      identity = currentIdentity;
    });
  }

  @override
  void didUpdateWidget(AccountUsageDisclosure oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, widget.controller)) {
      oldWidget.controller.removeListener(contextChanged);
      widget.controller.addListener(contextChanged);
    }
    if (!widget.visible || identity != currentIdentity) expanded = false;
    identity = currentIdentity;
  }

  @override
  void dispose() {
    widget.controller.removeListener(contextChanged);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Align(
        alignment: Alignment.centerLeft,
        child: DshButton(
          key: ValueKey('show-account-usage-${widget.provider}'),
          height: 32,
          padding: const EdgeInsets.symmetric(horizontal: 6),
          onPressed: widget.visible
              ? () => setState(() => expanded = !expanded)
              : null,
          child: Text(
            expanded ? DshAccountUsageZh.hide : DshAccountUsageZh.show,
          ),
        ),
      ),
      if (expanded && widget.visible)
        AccountUsagePanel(
          controller: widget.controller,
          api: widget.api,
          provider: widget.provider,
          accountScope: widget.accountScope,
          visible: true,
          needsLogin: widget.needsLogin,
          compact: widget.compact,
        ),
    ],
  );
}

class AccountUsagePanel extends StatefulWidget {
  const AccountUsagePanel({
    super.key,
    required this.controller,
    required this.api,
    required this.provider,
    required this.accountScope,
    required this.visible,
    this.needsLogin = false,
    this.compact = false,
    this.now,
  });

  final DesktopController controller;
  final DshClient api;
  final String provider, accountScope;
  final bool visible, needsLogin, compact;
  final DateTime Function()? now;

  @override
  State<AccountUsagePanel> createState() => _AccountUsagePanelState();
}

class _AccountUsagePanelState extends State<AccountUsagePanel> {
  static const cacheTtl = Duration(seconds: 60);
  AccountUsageSnapshot? _snapshot;
  RequestScope _requestScope = RequestScope();
  Timer? _resetTimer;
  final Set<int> _refreshedResets = {};
  DateTime? _fetchedAt;
  Object? _contextSignature;
  int _generation = 0;
  bool _loading = false, _stale = false, _refreshAfterLoading = false;
  String? _message;
  DateTime get _now => widget.now?.call() ?? DateTime.now();
  String get _inactiveMessage => _currentNeedsLogin
      ? DshAccountUsageZh.needsLogin
      : widget.accountScope.isEmpty
      ? widget.provider == 'claude-code'
            ? DshAccountUsageZh.claudeUsageHint
            : DshAccountUsageZh.unavailable
      : DshAccountUsageZh.identityChanged;

  Json? get _currentProvider => widget.controller.subscriptionAccounts
      .where((row) => row['id'] == widget.provider)
      .firstOrNull;

  String? _providerScope(Json? provider) {
    if (provider == null) return null;
    final own = provider['accountScope'];
    if (own is String && own.isNotEmpty) return own;
    final active = objects(provider['accounts'])
        .where((row) => row['active'] == true)
        .firstOrNull;
    final scope = active?['accountScope'];
    return scope is String ? scope : null;
  }

  bool get _currentNeedsLogin {
    final provider = _currentProvider;
    final active = provider == null
        ? null
        : objects(provider['accounts'])
              .where((row) => row['active'] == true)
              .firstOrNull;
    return widget.needsLogin || active?['needsLogin'] == true;
  }

  Object get _signature => (
    widget.controller,
    widget.api,
    widget.controller.client,
    widget.controller.host,
    widget.provider,
    widget.accountScope,
    _providerScope(_currentProvider),
    _currentProvider?['signedIn'],
    _currentNeedsLogin,
  );

  bool get _currentIdentity {
    if (!identical(widget.controller.client, widget.api) ||
        widget.accountScope.isEmpty ||
        _currentNeedsLogin) {
      return false;
    }
    final provider = _currentProvider;
    if (provider == null) {
      return widget.controller.subscriptionAccounts.isEmpty;
    }
    final currentScope = _providerScope(provider);
    return provider['signedIn'] != false &&
        (currentScope == null || currentScope == widget.accountScope);
  }

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_contextChanged);
    _contextSignature = _signature;
    _activate();
  }

  @override
  void didUpdateWidget(AccountUsagePanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, widget.controller)) {
      oldWidget.controller.removeListener(_contextChanged);
      widget.controller.addListener(_contextChanged);
    }
    if (_contextSignature != _signature) _clearForContext();
    if (!widget.visible) {
      _resetTimer?.cancel();
    } else if (!oldWidget.visible || _snapshot == null) {
      _activate();
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_contextChanged);
    _requestScope.cancel();
    _resetTimer?.cancel();
    super.dispose();
  }

  void _clearForContext() {
    _generation++;
    _requestScope.cancel();
    _requestScope = RequestScope();
    _resetTimer?.cancel();
    _refreshedResets.clear();
    _snapshot = null;
    _fetchedAt = null;
    _loading = false;
    _stale = false;
    _refreshAfterLoading = false;
    _message = _currentIdentity ? null : _inactiveMessage;
    _contextSignature = _signature;
  }

  void _contextChanged() {
    if (!mounted || _contextSignature == _signature) return;
    setState(_clearForContext);
  }

  void _activate() {
    if (!widget.visible) return;
    if (!_currentIdentity) {
      _message = _inactiveMessage;
      return;
    }
    if (_fetchedAt == null || _now.difference(_fetchedAt!) >= cacheTtl) {
      unawaited(_fetch());
    } else {
      _scheduleReset();
    }
  }

  Future<void> _fetch({bool refresh = false}) async {
    if (!mounted || !widget.visible || !_currentIdentity || _loading) return;
    final generation = ++_generation;
    final signature = _signature;
    setState(() {
      _loading = true;
      _message = null;
    });
    bool ownsResponse() =>
        mounted &&
        generation == _generation &&
        signature == _signature &&
        _currentIdentity;
    try {
      final value = await widget.api.request(
        '/provider-auth/account-usage',
        body: {
          'provider': widget.provider,
          'accountScope': widget.accountScope,
          if (refresh) 'refresh': true,
        },
        scope: _requestScope,
        maxBytes: 128 * 1024,
      );
      if (!ownsResponse()) return;
      final snapshot = AccountUsageSnapshot.fromJson(
        value,
        expectedProvider: widget.provider,
        expectedScope: widget.accountScope,
      );
      setState(() {
        _fetchedAt = _now;
        if (snapshot.status == AccountUsageStatus.needsLogin) {
          _refreshAfterLoading = false;
          _snapshot = null;
          _stale = false;
          _message = snapshot.message ?? DshAccountUsageZh.needsLogin;
        } else if (snapshot.status == AccountUsageStatus.unavailable &&
            _snapshot != null &&
            !snapshot.clearSnapshot) {
          _stale = true;
          _message = snapshot.message ?? DshAccountUsageZh.refreshFailed;
        } else {
          if (snapshot.clearSnapshot) _refreshAfterLoading = false;
          _snapshot = snapshot;
          _stale = snapshot.status == AccountUsageStatus.stale;
          _message = snapshot.message;
        }
      });
    } on AccountUsageIdentityException {
      if (!ownsResponse()) return;
      setState(() {
        _refreshAfterLoading = false;
        _snapshot = null;
        _fetchedAt = null;
        _stale = false;
        _message = DshAccountUsageZh.identityChanged;
      });
    } catch (error) {
      if (!ownsResponse()) return;
      final identityFailure =
          error is DshException &&
          (error.code == 'http-401' ||
              error.code == 'http-403' ||
              error.code == 'http-409' ||
              '${error.details['httpStatus']}' == '401' ||
              '${error.details['httpStatus']}' == '403' ||
              error.details['code'] == 'ACCOUNT_SCOPE_CHANGED');
      setState(() {
        if (identityFailure) {
          _refreshAfterLoading = false;
          _snapshot = null;
          _fetchedAt = null;
          _stale = false;
          _message = DshAccountUsageZh.needsLogin;
        } else {
          _stale = _snapshot != null;
          _message = _stale
              ? DshAccountUsageZh.refreshFailed
              : error is DshException && error.code == 'unsupported'
              ? DshAccountUsageZh.unsupported
              : DshAccountUsageZh.unavailable;
        }
      });
    } finally {
      if (ownsResponse()) {
        setState(() => _loading = false);
        if (_refreshAfterLoading && widget.visible) {
          _refreshAfterLoading = false;
          unawaited(_fetch(refresh: true));
        } else {
          _scheduleReset();
        }
      }
    }
  }

  void _scheduleReset() {
    _resetTimer?.cancel();
    if (!widget.visible || !_currentIdentity || _snapshot == null) return;
    final now = _now;
    final resets =
        _snapshot!.windows
            .map((window) => window.resetsAt)
            .whereType<int>()
            .where((reset) => !_refreshedResets.contains(reset))
            .toList()
          ..sort();
    for (final reset in resets) {
      final at = DateTime.fromMillisecondsSinceEpoch(reset * 1000);
      if (!at.isAfter(now)) {
        _refreshedResets.add(reset);
        if (_fetchedAt != null && _fetchedAt!.isBefore(at)) {
          _resetTimer = Timer(Duration.zero, _refreshAtReset);
          return;
        }
        continue;
      }
      final signature = _signature;
      _resetTimer = Timer(at.difference(now), () {
        if (!mounted || !widget.visible || signature != _signature) return;
        _refreshedResets.add(reset);
        _refreshAtReset();
      });
      return;
    }
  }

  void _refreshAtReset() {
    if (_loading) {
      _refreshAfterLoading = true;
    } else {
      unawaited(_fetch(refresh: true));
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final snapshot = _snapshot;
    final statusMessage =
        _message ??
        switch (snapshot?.status) {
          AccountUsageStatus.unsupported => DshAccountUsageZh.unsupported,
          AccountUsageStatus.unavailable => DshAccountUsageZh.unavailable,
          AccountUsageStatus.needsLogin => DshAccountUsageZh.needsLogin,
          _ => null,
        };
    return Container(
      key: ValueKey('account-usage-${widget.provider}'),
      margin: const EdgeInsets.only(top: 12, bottom: 8),
      padding: EdgeInsets.all(widget.compact ? 8 : 12),
      decoration: BoxDecoration(
        color: colors.layer,
        border: Border.all(color: colors.border),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  DshAccountUsageZh.title,
                  style: TextStyle(
                    fontSize: DshTypography.sizeAuxiliary,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              if (_loading)
                const Padding(
                  padding: EdgeInsets.only(right: 8),
                  child: SizedBox.square(
                    dimension: 12,
                    child: CircularProgressIndicator(
                      strokeWidth: 1.5,
                      semanticsLabel: DshAccountUsageZh.loading,
                    ),
                  ),
                ),
              DshButton(
                key: ValueKey('account-usage-refresh-${widget.provider}'),
                outline: true,
                height: 28,
                fontSize: DshTypography.sizeCaption,
                onPressed: _loading || !_currentIdentity
                    ? null
                    : () => unawaited(_fetch(refresh: true)),
                child: const Text(DshAccountUsageZh.refresh),
              ),
            ],
          ),
          if (snapshot != null) ...[
            const SizedBox(height: 6),
            _caption(
              DshAccountUsageZh.plan(
                snapshot.plan ?? DshAccountUsageZh.unavailable,
              ),
            ),
          ],
          if (statusMessage != null) ...[
            const SizedBox(height: 6),
            _caption(statusMessage),
          ],
          if (snapshot != null &&
              (snapshot.status == AccountUsageStatus.fresh ||
                  snapshot.status == AccountUsageStatus.stale) &&
              snapshot.windows.isNotEmpty)
            for (final window in snapshot.windows) _window(window),
          if (!_loading &&
              statusMessage == null &&
              (snapshot == null || snapshot.windows.isEmpty)) ...[
            const SizedBox(height: 8),
            _caption(DshAccountUsageZh.noWindows),
          ],
          if (snapshot?.updatedAt != null || _stale) ...[
            const SizedBox(height: 8),
            _caption(
              [
                if (_stale) DshAccountUsageZh.stale,
                if (snapshot?.updatedAt != null)
                  DshAccountUsageZh.updated(_localTime(snapshot!.updatedAt!)),
              ].join(' · '),
            ),
          ],
        ],
      ),
    );
  }

  Widget _caption(String value) => Text(
    value,
    style: TextStyle(
      color: DshColors(context).muted,
      fontSize: DshTypography.sizeCaption,
    ),
  );

  Widget _window(AccountUsageWindow window) {
    final colors = DshColors(context);
    final used = window.usedPercent;
    final remaining = window.remainingPercent;
    final amounts = _amountSummary(window);
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Wrap(
            alignment: WrapAlignment.spaceBetween,
            spacing: 12,
            runSpacing: 4,
            children: [
              Text(
                window.label.isEmpty ? DshAccountUsageZh.window : window.label,
                style: const TextStyle(fontSize: DshTypography.sizeAuxiliary),
              ),
              Text(
                used == null || remaining == null
                    ? amounts ?? DshAccountUsageZh.unavailable
                    : '${DshAccountUsageZh.used(_number(used))} · ${DshAccountUsageZh.remaining(_number(remaining))}',
                style: const TextStyle(fontSize: DshTypography.sizeCaption),
              ),
            ],
          ),
          if (used != null) ...[
            const SizedBox(height: 6),
            ClipRRect(
              borderRadius: BorderRadius.circular(3),
              child: LinearProgressIndicator(
                key: ValueKey(
                  'account-usage-progress-${widget.provider}-${window.id}',
                ),
                value: used / 100,
                minHeight: 6,
                backgroundColor: colors.border,
                color: used >= 90 ? colors.warning : colors.blue,
                semanticsLabel:
                    '${window.label} ${DshAccountUsageZh.used(_number(used))}',
                semanticsValue: _number(used),
              ),
            ),
            if (amounts != null) ...[
              const SizedBox(height: 5),
              _caption(amounts),
            ],
          ],
          ...[
            const SizedBox(height: 5),
            Wrap(
              spacing: 12,
              runSpacing: 4,
              children: [
                if (!widget.compact && window.windowDurationMins != null)
                  _caption(
                    DshAccountUsageZh.duration(
                      _duration(window.windowDurationMins!),
                    ),
                  ),
                _caption(
                  DshAccountUsageZh.reset(
                    window.resetsAt == null
                        ? DshAccountUsageZh.unavailable
                        : _localTime(window.resetsAt!),
                  ),
                ),
                if (window.periodEndsAt != null)
                  _caption(
                    DshAccountUsageZh.periodEnd(
                      _localTime(window.periodEndsAt!),
                    ),
                  ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

String? _amountSummary(AccountUsageWindow window) {
  String amount(double value) {
    final unit = switch (window.unit) {
      'credits' => 'credits',
      'requests' => '次',
      'usd' => 'USD',
      'acu' => 'ACU',
      'percent' => '%',
      _ => DshAccountUsageZh.unknownUnit,
    };
    final number = value > 0 && value < .0001
        ? value.toStringAsPrecision(3)
        : value.toStringAsFixed(4).replaceFirst(RegExp(r'\.?0+$'), '');
    return '$number $unit';
  }

  final values = [
    if (window.used != null) DshAccountUsageZh.usedAmount(amount(window.used!)),
    if (window.remaining != null)
      DshAccountUsageZh.remainingAmount(amount(window.remaining!)),
    if (window.limit != null)
      DshAccountUsageZh.limitAmount(amount(window.limit!)),
  ];
  return values.isEmpty ? null : values.join(' · ');
}

String _number(double value) =>
    value.toStringAsFixed(1).replaceFirst(RegExp(r'\.0$'), '');

String _duration(double minutes) => minutes >= 1440 && minutes % 1440 == 0
    ? DshAccountUsageZh.days(_number(minutes / 1440))
    : minutes >= 60 && minutes % 60 == 0
    ? DshAccountUsageZh.hours(_number(minutes / 60))
    : DshAccountUsageZh.minutes(_number(minutes));

String _localTime(int seconds) {
  final date = DateTime.fromMillisecondsSinceEpoch(seconds * 1000).toLocal();
  String pad(int value) => value.toString().padLeft(2, '0');
  final offset = date.timeZoneOffset.inMinutes;
  final zone =
      '${offset >= 0 ? '+' : '-'}${pad(offset.abs() ~/ 60)}:${pad(offset.abs() % 60)}';
  return '${date.year}-${pad(date.month)}-${pad(date.day)} ${pad(date.hour)}:${pad(date.minute)} UTC$zone';
}
