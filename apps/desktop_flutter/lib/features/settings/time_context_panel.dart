import '../../l10n/zh.dart';
import '../../l10n/plugin_settings_zh.dart';

import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../src/controller.dart';

import 'package:dsh_desktop/design/typography.dart';

class TimeContextPanel extends StatefulWidget {
  const TimeContextPanel({
    super.key,
    required this.controller,
    required this.entryId,
  });

  final DesktopController controller;
  final String entryId;

  @override
  State<TimeContextPanel> createState() => _TimeContextPanelState();
}

class _TimeContextPanelState extends State<TimeContextPanel> {
  late final DesktopController owner;
  late final DshClient? api;
  late final HostInfo? host;
  late final String entryId;
  final interval = TextEditingController();
  final zone = TextEditingController();
  final scope = RequestScope();
  Json? snapshot;
  int intervalUnit = 60000;
  bool busy = false, invalidated = false, needsRefresh = false;
  String? error, notice;

  bool get stale =>
      invalidated ||
      !identical(owner, widget.controller) ||
      entryId != widget.entryId ||
      api == null ||
      !identical(api, owner.client) ||
      !identical(host, owner.host);

  @override
  void initState() {
    super.initState();
    // Bind all identities before a request or controller notification can run.
    owner = widget.controller;
    api = owner.client;
    host = owner.host;
    entryId = widget.entryId;
    owner.addListener(connectionChanged);
    load();
  }

  @override
  void didUpdateWidget(TimeContextPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    connectionChanged();
  }

  @override
  void dispose() {
    owner.removeListener(connectionChanged);
    scope.cancel();
    interval.dispose();
    zone.dispose();
    super.dispose();
  }

  void connectionChanged() {
    if (!mounted || !stale || invalidated) return;
    invalidated = true;
    scope.cancel();
    setState(() {
      busy = false;
      error = DshSettingsZh.timeContextStale;
      notice = null;
    });
  }

  BigInt? get intervalMilliseconds {
    final text = interval.text.trim();
    if (text.isEmpty) return null;
    if (!RegExp(r'^\d+(?:\.\d+)?$').hasMatch(text)) return null;
    final parts = text.split('.');
    final fraction = parts.length == 2 ? parts[1] : '';
    // Bound parsing work as well as the eventual cross-platform integer value.
    if (parts[0].length + fraction.length > 32) return null;
    final numerator =
        BigInt.parse('${parts[0]}$fraction') * BigInt.from(intervalUnit);
    final denominator = BigInt.from(10).pow(fraction.length);
    if (numerator % denominator != BigInt.zero) return null;
    return numerator ~/ denominator;
  }

  String? get intervalError => interval.text.trim().isEmpty
      ? null
      : intervalMilliseconds == null ||
            intervalMilliseconds! > BigInt.from(9007199254740991)
      ? DshPluginSettingsZh.intervalInvalid
      : null;

  void changeIntervalUnit(int value) {
    if (interval.text.trim().isEmpty) {
      setState(() => intervalUnit = value);
      return;
    }
    final milliseconds = intervalMilliseconds;
    final formatted = milliseconds == null
        ? null
        : formatInUnit(milliseconds, value);
    if (formatted == null) {
      setState(() => notice = DshPluginSettingsZh.unitPrecisionHint);
      return;
    }
    setState(() {
      intervalUnit = value;
      interval.text = formatted;
      notice = null;
    });
  }

  String? formatInUnit(BigInt milliseconds, int unit) {
    final divisor = BigInt.from(unit);
    final whole = milliseconds ~/ divisor;
    var remainder = milliseconds % divisor;
    if (remainder == BigInt.zero) return '$whole';
    final fraction = StringBuffer();
    // Milliseconds have at most three decimal places in seconds and five
    // finite decimal places in minutes. A recurring fraction stays unchanged.
    for (var digit = 0; digit < 5 && remainder != BigInt.zero; digit++) {
      remainder *= BigInt.from(10);
      fraction.write(remainder ~/ divisor);
      remainder %= divisor;
    }
    return remainder == BigInt.zero ? '$whole.$fraction' : null;
  }

  Json draftConfig() {
    final config = object(snapshot?['config'])
      ..remove('refreshIntervalMs')
      ..remove('timeZone');
    final value = interval.text.trim(), timeZone = zone.text.trim();
    if (value.isNotEmpty) {
      config['refreshIntervalMs'] = intervalMilliseconds!.toInt();
    }
    if (timeZone.isNotEmpty) config['timeZone'] = timeZone;
    return config;
  }

  bool get dirty =>
      intervalError != null ||
      !mapEquals(draftConfig(), object(snapshot?['config']));

  Json validate(Json value) {
    if (value['entryId'] != entryId ||
        value['revision'] is! String ||
        (value['revision'] as String).isEmpty ||
        !const [
          'dsh-time-context',
          '@deepseek-ai/dsh-time-context',
        ].contains(value['moduleName']) ||
        (value['config'] != null && value['config'] is! Map)) {
      throw const FormatException(DshSettingsZh.timeContextResponseInvalid);
    }
    final config = object(value['config']);
    final refresh = config['refreshIntervalMs'];
    if ((config.containsKey('refreshIntervalMs') &&
            (refresh is! num ||
                !refresh.isFinite ||
                refresh < 0 ||
                refresh > 9007199254740991 ||
                refresh != refresh.truncateToDouble())) ||
        (config.containsKey('timeZone') && config['timeZone'] is! String)) {
      throw const FormatException(DshSettingsZh.timeContextInvalid);
    }
    return object(jsonDecode(jsonEncode(value)));
  }

  void useSnapshot() {
    final config = object(snapshot?['config']);
    final milliseconds = (config['refreshIntervalMs'] as num?)?.toInt();
    intervalUnit = milliseconds == null || milliseconds % 60000 == 0
        ? 60000
        : milliseconds % 1000 == 0
        ? 1000
        : 1;
    interval.text = milliseconds == null
        ? ''
        : '${milliseconds ~/ intervalUnit}';
    zone.text = config['timeZone'] as String? ?? '';
  }

  String describe(Json value) {
    final config = object(value['config']);
    final ms = (config['refreshIntervalMs'] as num?)?.toInt() ?? 600000;
    return '${ms == 0 ? DshSettingsZh.everyApplicableStep : DshPluginSettingsZh.every(ms)}，'
        '${config['timeZone'] ?? DshSettingsZh.systemTimezone}';
  }

  String failure(Object value) {
    if (value is FormatException) return value.message;
    if (value is DshException) {
      if (value.code == 'plugin-config-conflict' ||
          value.code == 'plugin-config-runtime-conflict') {
        return DshSettingsZh.timeContextConflict;
      }
      if (value.outcomeUnknown) {
        return DshSettingsZh.timeContextOutcomeUnknown;
      }
      return DshSettingsZh.errorDraftRetained(message: value.message);
    }
    return DshSettingsZh.timeContextFailure;
  }

  Future<void> load() async {
    if (busy || stale) {
      connectionChanged();
      return;
    }
    final preserve = snapshot != null;
    setState(() {
      busy = true;
      error = notice = null;
    });
    try {
      final value = validate(
        await api!.rpc(
          'pluginInventory.getConfig',
          payload: {'entryId': entryId},
          scope: scope,
        ),
      );
      if (!mounted || stale) return;
      setState(() {
        snapshot = value;
        needsRefresh = false;
        if (preserve) {
          notice = DshSettingsZh.latestConfiguration(
            description: describe(value),
          );
        } else {
          useSnapshot();
        }
      });
    } catch (value) {
      if (mounted && !stale) setState(() => error = failure(value));
    } finally {
      if (mounted && !stale) setState(() => busy = false);
    }
  }

  Future<void> save() async {
    if (busy || stale || snapshot == null || needsRefresh || !dirty) return;
    if (intervalError != null) return;
    final config = draftConfig(), revision = snapshot!['revision'];
    setState(() {
      busy = true;
      error = notice = null;
    });
    try {
      final value = validate(
        await api!.rpc(
          'pluginInventory.setConfig',
          mutation: true,
          scope: scope,
          payload: {
            'entryId': entryId,
            'expectedRevision': revision,
            'config': config,
          },
        ),
      );
      if (!mounted || stale) return;
      setState(() {
        snapshot = value;
        useSnapshot();
        notice = DshSettingsZh.timeContextSaved;
      });
    } catch (value) {
      if (mounted && !stale) {
        setState(() {
          error = failure(value);
          needsRefresh = true;
        });
      }
    } finally {
      if (mounted && !stale) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final disabled = busy || stale || snapshot == null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        const Text(
          DshSettingsZh.timeContext,
          style: TextStyle(
            fontSize: DshTypography.sizeComposer,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        const Text(DshPluginSettingsZh.intervalHint),
        const SizedBox(height: 12),
        const Text(DshPluginSettingsZh.interval),
        const SizedBox(height: 6),
        LayoutBuilder(
          builder: (context, constraints) {
            final field = DshField(
              key: const ValueKey('time-context-interval'),
              controller: interval,
              enabled: !disabled,
              hint: '${600000 ~/ intervalUnit}',
              onChanged: (_) => setState(() => notice = null),
            );
            final unit = DshSelect<int>(
              key: const ValueKey('time-context-unit'),
              options: DshPluginSettingsZh.units,
              value: intervalUnit,
              onChanged: disabled ? null : changeIntervalUnit,
            );
            if (constraints.maxWidth < 360) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [field, const SizedBox(height: 8), unit],
              );
            }
            return Row(
              children: [
                Expanded(child: field),
                const SizedBox(width: 8),
                unit,
              ],
            );
          },
        ),
        const SizedBox(height: 12),
        const Text(DshSettingsZh.fallbackTimezone),
        DshField(
          key: const ValueKey('time-context-zone'),
          controller: zone,
          enabled: !disabled,
          hint: DshSettingsZh.systemTimezoneHint,
          onChanged: (_) => setState(() => notice = null),
        ),
        const SizedBox(height: 8),
        const Text(DshSettingsZh.timeContextDefaultsHint),
        if (busy)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 8),
            child: LinearProgressIndicator(),
          ),
        if (intervalError != null || error != null)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: SelectableText(
              intervalError ?? error!,
              style: TextStyle(color: DshTokens.of(context).error.foreground),
            ),
          ),
        if (notice != null)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Text(notice!),
          ),
        const SizedBox(height: 12),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            DshButton(
              key: const ValueKey('time-context-save'),
              primary: true,
              onPressed:
                  disabled || needsRefresh || !dirty || intervalError != null
                  ? null
                  : save,
              child: const Text(DshSettingsZh.saveConfiguration),
            ),
            DshButton(
              key: const ValueKey('time-context-discard'),
              onPressed: disabled || !dirty
                  ? null
                  : () => setState(() {
                      useSnapshot();
                      notice = null;
                      if (!needsRefresh) error = null;
                    }),
              child: const Text(DshSettingsZh.cancelChanges),
            ),
            DshButton(
              key: const ValueKey('time-context-reload'),
              outline: true,
              onPressed: busy || stale ? null : load,
              child: const Text(DshSettingsZh.reloadKeepDraft),
            ),
          ],
        ),
      ],
    );
  }
}
