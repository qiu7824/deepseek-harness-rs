import 'dart:async';
import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

/// Manages Host-owned operations; disposing this view never cancels an operation.
class PluginOperationsPanel extends StatefulWidget {
  const PluginOperationsPanel({
    super.key,
    required this.controller,
    this.onOperationCompleted,
  });

  final DesktopController controller;
  final VoidCallback? onOperationCompleted;

  @override
  State<PluginOperationsPanel> createState() => _PluginOperationsPanelState();
}

const _activePhases = {
  'running',
  'cancelling',
  'awaiting-client',
  'rolling-back',
  'committing',
};
const _phaseLabels = {
  'running': '正在处理',
  'cancelling': '正在取消并清理',
  'awaiting-client': '等待网页客户端确认',
  'rolling-back': '正在恢复原状态',
  'committing': '正在保存',
  'succeeded': '已完成',
  'failed': '操作失败',
  'cancelled': '已取消',
  'interrupted': '操作被中断',
  'recovery-required': '需要恢复检查',
};

class _Operation {
  _Operation(Json value)
    : id = value['operationId'] as String,
      action = value['action'] as String,
      spec = value['spec'] as String,
      phase = value['phase'] as String,
      log = value['log'] as String,
      error = value['error'] as String?,
      restartRequired = value['restartRequired'] as bool;

  static _Operation? parse(Object? raw) {
    if (raw == null) return null;
    if (raw is! Map ||
        raw['version'] != 1 ||
        raw['operationId'] is! String ||
        (raw['operationId'] as String).isEmpty ||
        raw['action'] is! String ||
        raw['spec'] is! String ||
        !_phaseLabels.containsKey(raw['phase']) ||
        raw['log'] is! String ||
        (raw['error'] != null && raw['error'] is! String) ||
        raw['restartRequired'] is! bool) {
      throw const FormatException('插件操作状态不完整或版本不受支持');
    }
    return _Operation(object(raw));
  }

  final String id, action, spec, phase, log;
  final String? error;
  final bool restartRequired;
  bool get active => _activePhases.contains(phase);
}

class _PluginOperationsPanelState extends State<PluginOperationsPanel> {
  late final DshClient? _api = widget.controller.client;
  final _scope = RequestScope();
  final _installDraft = TextEditingController();
  final _removeDraft = TextEditingController();
  RequestScope? _readScope;
  Timer? _timer;
  int _generation = 0;
  bool _invalidated = false,
      _sending = false,
      _statusReady = false,
      _hasSnapshot = false;
  String _action = 'add';
  String? _error, _statusError, _configurationError, _lastCompletedId;
  _Operation? _operation;
  Json? _confirmation;

  bool get _current =>
      mounted &&
      !_invalidated &&
      _api != null &&
      identical(_api, widget.controller.client);
  bool get _busy => _sending || (_operation?.active ?? false);
  bool get _canStart => _current && _statusReady && !_busy;
  TextEditingController get _draft =>
      _action == 'add' ? _installDraft : _removeDraft;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_connectionChanged);
    if (_api == null) {
      _statusError = '请先连接服务，再重新打开插件设置。';
    } else {
      unawaited(_refresh());
    }
  }

  @override
  void didUpdateWidget(covariant PluginOperationsPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, widget.controller)) {
      oldWidget.controller.removeListener(_connectionChanged);
      widget.controller.addListener(_connectionChanged);
    }
    _connectionChanged();
  }

  void _connectionChanged() {
    if (_invalidated || identical(_api, widget.controller.client)) return;
    _invalidated = true;
    _generation++;
    _timer?.cancel();
    _scope.cancel();
    if (mounted) {
      setState(() {
        _confirmation = null;
        _sending = false;
        _statusReady = false;
        _statusError = '连接已变化，请关闭后重新打开插件设置。';
      });
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_connectionChanged);
    _generation++;
    _timer?.cancel();
    _scope.cancel();
    _installDraft.dispose();
    _removeDraft.dispose();
    super.dispose();
  }

  void _scheduleRefresh({bool failed = false}) {
    _timer?.cancel();
    if (!_current || _sending) return;
    _timer = Timer(
      Duration(
        milliseconds: !failed && (_operation?.active ?? false) ? 750 : 5000,
      ),
      () => unawaited(_refresh()),
    );
  }

  void _accept(Json response, {bool mutation = false}) {
    if (!response.containsKey('operation') ||
        (response['configurationError'] != null &&
            response['configurationError'] is! String)) {
      throw const FormatException('插件管理响应不完整');
    }
    final next = _Operation.parse(response['operation']);
    if (mutation && next == null) {
      throw const FormatException('插件管理响应缺少操作标识');
    }
    final hadSnapshot = _hasSnapshot;
    setState(() {
      _operation = next;
      _hasSnapshot = true;
      if (!mutation || response.containsKey('configurationError')) {
        _configurationError = response['configurationError'] as String?;
      }
      _statusReady = true;
      _statusError = null;
      if (_confirmation?['action'] == 'cancel' &&
          (next?.id != _confirmation?['operationId'] ||
              next?.active != true ||
              next?.phase == 'cancelling')) {
        _confirmation = null;
      }
    });
    if (next != null && !next.active && next.id != _lastCompletedId) {
      _lastCompletedId = next.id;
      if (mutation || hadSnapshot) widget.onOperationCompleted?.call();
    }
  }

  Future<void> _refresh() async {
    if (!_current || _sending || _readScope != null) return;
    _timer?.cancel();
    final generation = _generation;
    final scope = _readScope = RequestScope();
    final unregister = _scope.register(scope.cancel);
    var failed = false;
    try {
      final result = await _api!.request(
        '/__dsh-plugin-manager',
        body: {'action': 'status'},
        scope: scope,
      );
      if (_current && generation == _generation) _accept(result);
    } catch (error) {
      failed = true;
      if (_current && generation == _generation) {
        setState(() {
          _statusReady = false;
          _statusError = '无法读取插件操作状态：$error';
        });
      }
    } finally {
      unregister();
      scope.cancel();
      if (identical(_readScope, scope)) _readScope = null;
      if (_current && generation == _generation) {
        _scheduleRefresh(failed: failed);
      }
    }
  }

  void _prepare() {
    if (!_canStart) return;
    final spec = _draft.text.trim();
    String? invalid;
    if (spec.isEmpty ||
        utf8.encode(spec).length > 400 ||
        RegExp(r'[\x00-\x1f\x7f-\x9f]').hasMatch(spec)) {
      invalid = '请输入不超过 400 字节的插件来源或包名。';
    } else if (_action == 'add' &&
        !RegExp(r'^github:[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+#[A-Fa-f0-9]{40}$')
            .hasMatch(spec)) {
      invalid = '安装来源须为 github:owner/repo#完整的 40 位提交 SHA。';
    }
    setState(() {
      _error = invalid;
      _confirmation = invalid == null
          ? {'action': _action, 'spec': spec}
          : null;
    });
  }

  Future<void> _mutate() async {
    final input = _confirmation;
    if (!_current || _sending || !_statusReady || input == null) return;
    if (input['action'] == 'cancel') {
      if (_operation?.id != input['operationId'] ||
          _operation?.active != true ||
          _operation?.phase == 'cancelling') {
        return;
      }
    } else if (_busy) {
      return;
    }
    final generation = ++_generation;
    _timer?.cancel();
    _readScope?.cancel();
    _readScope = null;
    setState(() {
      _sending = true;
      _error = null;
      _confirmation = null;
    });
    try {
      final result = await _api!.request(
        '/__dsh-plugin-manager',
        body: input,
        scope: _scope,
        mutation: true,
      );
      if (_current && generation == _generation) {
        if (input['action'] == 'cancel' &&
            object(result['operation'])['operationId'] !=
                input['operationId']) {
          throw const FormatException('取消结果与指定操作不一致');
        }
        _accept(result, mutation: true);
      }
    } catch (error) {
      if (_current && generation == _generation) {
        setState(() {
          _error = '$error';
          _statusReady = false;
          _statusError = '操作结果需要核对，正在重新读取后台状态。';
        });
      }
    } finally {
      if (_current && generation == _generation) {
        setState(() => _sending = false);
        if (_statusReady) {
          _scheduleRefresh();
        } else {
          unawaited(_refresh());
        }
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final operation = _operation;
    final confirmation = _confirmation;
    final confirmingCancel = confirmation?['action'] == 'cancel';
    final canConfirm =
        _current &&
        _statusReady &&
        !_sending &&
        (confirmingCancel ? operation?.active == true : !_busy);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text('安装、更新与卸载', style: Theme.of(context).textTheme.titleMedium),
        const SizedBox(height: 8),
        const Text('支持纯 Web 插件，安装来源须固定到完整提交 SHA；插件界面在网页端运行。'),
        const Text('关闭面板不会取消后台操作，重新打开可继续查看进度。'),
        const SizedBox(height: 12),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            for (final entry in const {
              'add': '安装 / 更新',
              'remove': '卸载',
            }.entries)
              DshButton(
                key: ValueKey('plugin-action-${entry.key}'),
                active: _action == entry.key,
                onPressed: !_current || _busy
                    ? null
                    : () => setState(() {
                        _action = entry.key;
                        _confirmation = null;
                        _error = null;
                      }),
                child: Text(entry.value),
              ),
            DshButton(
              key: const ValueKey('plugin-refresh'),
              onPressed: !_current || _sending
                  ? null
                  : () => unawaited(_refresh()),
              child: const Text('刷新状态'),
            ),
          ],
        ),
        const SizedBox(height: 10),
        Text(_action == 'add' ? 'GitHub 来源' : '已安装包名'),
        const SizedBox(height: 4),
        DshField(
          key: const ValueKey('plugin-spec'),
          controller: _draft,
          enabled: _current && !_busy,
          hint: _action == 'add'
              ? 'github:owner/repo#完整提交 SHA'
              : '@scope/package',
          onChanged: (_) => setState(() => _confirmation = null),
        ),
        const SizedBox(height: 8),
        DshButton(
          key: const ValueKey('plugin-check'),
          outline: true,
          onPressed: _canStart ? _prepare : null,
          child: const Text('检查操作'),
        ),
        if (_configurationError != null) ...[
          const SizedBox(height: 12),
          SelectableText(_configurationError!),
          DshButton(
            key: const ValueKey('plugin-recover'),
            onPressed: _canStart
                ? () => setState(() {
                    _confirmation = {'action': 'recover'};
                  })
                : null,
            child: const Text('恢复上次有效配置'),
          ),
        ],
        if (confirmation != null) ...[
          const SizedBox(height: 12),
          Text(switch (confirmation['action']) {
            'add' => '插件将在网页端运行，请确认信任此来源并安装或更新：',
            'remove' => '确认卸载此插件？提交后须重启应用使变更生效。',
            'recover' => '确认用上次有效配置恢复当前插件配置？',
            _ => '确认取消这项后台操作？清理完成前请勿重复提交。',
          }),
          SelectableText(
            '${confirmation['spec'] ?? confirmation['operationId'] ?? ''}',
          ),
          Wrap(
            spacing: 8,
            children: [
              DshButton(
                key: const ValueKey('plugin-confirm'),
                primary: true,
                onPressed: canConfirm ? () => unawaited(_mutate()) : null,
                child: const Text('确认操作'),
              ),
              DshButton(
                key: const ValueKey('plugin-dismiss-confirm'),
                onPressed: () => setState(() => _confirmation = null),
                child: const Text('返回编辑'),
              ),
            ],
          ),
        ],
        if (_sending)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 8),
            child: LinearProgressIndicator(),
          ),
        if (operation != null) ...[
          const SizedBox(height: 12),
          Semantics(
            liveRegion: true,
            child: Text(_phaseLabels[operation.phase]!),
          ),
          SelectableText('操作标识：${operation.id}'),
          Text(
            '操作：${switch (operation.action) {
              'add' => '安装 / 更新',
              'remove' => '卸载',
              'recover' => '恢复配置',
              'enable' => '启用',
              'disable' => '停用',
              _ => operation.action,
            }}',
          ),
          if (operation.spec.isNotEmpty) SelectableText(operation.spec),
          if (operation.active)
            DshButton(
              key: const ValueKey('plugin-cancel'),
              onPressed:
                  !_current ||
                      !_statusReady ||
                      _sending ||
                      operation.phase == 'cancelling'
                  ? null
                  : () => setState(() {
                      _confirmation = {
                        'action': 'cancel',
                        'operationId': operation.id,
                      };
                    }),
              child: const Text('取消后台操作'),
            ),
          if (operation.restartRequired) const Text('变更已提交，请重启应用使插件配置生效。'),
          if (operation.error != null) SelectableText(operation.error!),
          if (operation.log.isNotEmpty)
            ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 220),
              child: SingleChildScrollView(
                child: SelectableText(operation.log),
              ),
            ),
        ],
        if (_statusError != null) SelectableText(_statusError!),
        if (_error != null) SelectableText(_error!),
      ],
    );
  }
}
