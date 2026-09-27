import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

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
      error = '连接或插件已变化，草稿已保留；请重新打开时间上下文设置。';
      notice = null;
    });
  }

  String? get intervalError {
    final text = interval.text.trim();
    if (text.isEmpty) return null;
    final value = int.tryParse(text);
    return !RegExp(r'^\d+$').hasMatch(text) ||
            value == null ||
            value > 9007199254740991
        ? '刷新间隔须为 0–9007199254740991 的整数（毫秒）。'
        : null;
  }

  Json draftConfig() {
    final config = object(snapshot?['config'])
      ..remove('refreshIntervalMs')
      ..remove('timeZone');
    final value = interval.text.trim(), timeZone = zone.text.trim();
    if (value.isNotEmpty) config['refreshIntervalMs'] = int.parse(value);
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
      throw const FormatException('时间上下文配置返回的数据不完整，请重新读取。');
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
      throw const FormatException('时间上下文配置格式无效，请检查插件配置。');
    }
    return object(jsonDecode(jsonEncode(value)));
  }

  void useSnapshot() {
    final config = object(snapshot?['config']);
    interval.text = config['refreshIntervalMs'] == null
        ? ''
        : '${(config['refreshIntervalMs'] as num).toInt()}';
    zone.text = config['timeZone'] as String? ?? '';
  }

  String describe(Json value) {
    final config = object(value['config']);
    final ms = (config['refreshIntervalMs'] as num?)?.toInt() ?? 600000;
    return '${ms == 0 ? '每个适用步骤更新' : '每 $ms 毫秒更新'}，'
        '${config['timeZone'] ?? '系统时区'}';
  }

  String failure(Object value) {
    if (value is FormatException) return value.message;
    if (value is DshException) {
      if (value.code == 'plugin-config-conflict' ||
          value.code == 'plugin-config-runtime-conflict') {
        return '配置已在其他位置更改，草稿已保留；请读取最新配置并核对后再保存。';
      }
      if (value.outcomeUnknown) {
        return '保存结果尚未确认，草稿已保留；请读取最新配置后核对。';
      }
      return '${value.message}；草稿已保留。';
    }
    return '配置操作失败，草稿已保留；请重新读取后重试。';
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
          notice = '已读取最新配置：${describe(value)}。草稿已保留，请核对后保存。';
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
        notice = '时间上下文配置已保存，启停状态保持不变。';
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
          '时间上下文',
          style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
        ),
        const SizedBox(height: 8),
        const Text('默认每十分钟更新时间；间隔为 0 时在每个适用步骤更新。'),
        const SizedBox(height: 12),
        const Text('刷新间隔（毫秒）'),
        DshField(
          key: const ValueKey('time-context-interval'),
          controller: interval,
          enabled: !disabled,
          hint: '600000',
          onChanged: (_) => setState(() => notice = null),
        ),
        const SizedBox(height: 12),
        const Text('备用时区（IANA）'),
        DshField(
          key: const ValueKey('time-context-zone'),
          controller: zone,
          enabled: !disabled,
          hint: '留空使用系统时区',
          onChanged: (_) => setState(() => notice = null),
        ),
        const SizedBox(height: 8),
        const Text('留空使用默认值，保存配置保持当前启停状态。'),
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
              style: const TextStyle(color: Colors.red),
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
              child: const Text('保存配置'),
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
              child: const Text('取消修改'),
            ),
            DshButton(
              key: const ValueKey('time-context-reload'),
              outline: true,
              onPressed: busy || stale ? null : load,
              child: const Text('重新读取（保留草稿）'),
            ),
          ],
        ),
      ],
    );
  }
}
