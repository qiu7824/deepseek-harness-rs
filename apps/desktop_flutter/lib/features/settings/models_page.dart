import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import 'account_login.dart';
import 'model_editor_widgets.dart';
import '../../src/controller.dart';

class _DashedBorderPainter extends CustomPainter {
  const _DashedBorderPainter(this.color);
  final Color color;
  @override
  void paint(Canvas canvas, Size size) {
    final path = Path()
      ..addRRect(
        RRect.fromRectAndRadius(Offset.zero & size, const Radius.circular(8)),
      );
    final paint = Paint()
      ..color = color
      ..strokeWidth = 1
      ..style = PaintingStyle.stroke;
    for (final metric in path.computeMetrics()) {
      for (var offset = 0.0; offset < metric.length; offset += 8) {
        final end = offset + 4 < metric.length ? offset + 4 : metric.length;
        canvas.drawPath(metric.extractPath(offset, end), paint);
      }
    }
  }

  @override
  bool shouldRepaint(_DashedBorderPainter oldDelegate) =>
      oldDelegate.color != color;
}

class ModelsPage extends StatefulWidget {
  const ModelsPage({
    super.key,
    required this.controller,
    required this.onSettingsChanged,
    this.initialTab = 'api',
    this.onDirtyChanged,
  });
  final DesktopController controller;
  final Future<void> Function() onSettingsChanged;
  final String initialTab;
  final ValueChanged<bool>? onDirtyChanged;
  @override
  State<ModelsPage> createState() => _ModelsPageState();
}

class _ModelsPageState extends State<ModelsPage> {
  DshClient? boundApi;
  DshClient get api => boundApi ?? (throw StateError('请先连接服务'));
  bool get staleConnection =>
      boundApi == null || widget.controller.client != boundApi;
  void connectionChanged() {
    if (staleConnection && mounted) {
      scope.cancel();
      keyInput.clear();
      setState(() => error = '连接已变化，请关闭后重新打开模型设置。');
    }
  }

  final providerSearch = TextEditingController();
  final connectionName = TextEditingController();
  String connectionProtocol = '';
  bool editingConnection = false,
      advancedOpen = false,
      declaring = false,
      addingProvider = false,
      modelsExpanded = true,
      connectionDirty = false,
      customDirty = false;
  bool needsAttention = false;
  String visibility = 'all';
  int draftSequence = 0, modelLimit = 100;
  final invalidFields = <String>{};
  final capacityDrafts = <String, Map<String, String>>{};
  final search = TextEditingController(),
      base = TextEditingController(),
      keyInput = TextEditingController();
  final scope = RequestScope();
  List<Json> providers = [], accounts = [];
  final namespaces = <String, Json>{};
  final credentials = <String, Json>{};
  Json? provider, catalog;
  final changes = <String, Json>{};
  final manual = <Json>[];
  final removed = <String>{};
  final expandedAccounts = <String>{};
  late String tab = widget.initialTab;
  bool busy = false;
  bool lastReportedDirty = false;
  String? error, notice;
  int generation = 0;
  bool get modelDirty =>
      changes.isNotEmpty ||
      manual.isNotEmpty ||
      removed.isNotEmpty ||
      capacityDrafts.isNotEmpty;
  bool get dirty => modelDirty || connectionDirty || customDirty;
  @override
  void initState() {
    super.initState();
    boundApi = widget.controller.client;
    widget.controller.addListener(connectionChanged);
    load();
  }

  @override
  void dispose() {
    widget.controller.removeListener(connectionChanged);
    scope.cancel();
    providerSearch.dispose();
    connectionName.dispose();
    search.dispose();
    base.dispose();
    keyInput.dispose();
    super.dispose();
  }

  Future<void> run(Future<void> Function() work) async {
    if (staleConnection || !mounted) return;
    setState(() {
      busy = true;
      error = null;
      notice = null;
    });
    try {
      await work();
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> load() async {
    await run(() async {
      final values = await Future.wait([
        api.rpc('llm.providers', scope: scope),
        api.rpc('settings.describe', scope: scope),
      ]);
      if (!mounted) return;
      providers = objects(values[0]['providers']);
      credentials.clear();
      for (final ns in objects(values[1]['namespaces'])) {
        namespaces[ns['ns'] as String] = ns;
      }
      final refs = apiProviders
          .map((p) => profile(p)['apiKeyEnv'])
          .whereType<String>()
          .where((ref) => ref.isNotEmpty)
          .toSet()
          .toList();
      if (refs.isNotEmpty) {
        try {
          final states = await api.rpc(
            'credentials.describe',
            payload: {'refs': refs},
            scope: scope,
          );
          if (!mounted || staleConnection) return;
          for (final entry in object(states['credentials']).entries) {
            credentials[entry.key] = object(entry.value);
          }
        } catch (e) {
          if (!mounted || staleConnection) return;
          notice = '凭据状态暂不可用：$e';
        }
      }
      provider ??= configuredApiProviders.firstOrNull;
      if (tab == 'accounts') {
        await refreshAccounts();
      } else if (provider != null) {
        await selectProvider(provider!, force: true);
      }
    });
  }

  Json profile(Json p) {
    dynamic value = namespaces[p['settingsNs']]?['value'];
    for (final key in p['settingsPath'] as List? ?? []) {
      value = object(value)[key];
    }
    return object(value);
  }

  List<Json> get apiProviders =>
      providers.where((p) => profile(p)['authProvider'] is! String).toList();
  bool configuredProvider(Json p) =>
      namespaces.containsKey(p['settingsNs']) &&
      ((p['settingsPath'] as List? ?? []).isEmpty || profile(p).isNotEmpty);
  List<Json> get configuredApiProviders =>
      apiProviders.where(configuredProvider).toList();
  List<Json> get addableProviders => apiProviders
      .where((p) => namespaces.containsKey(p['settingsNs']))
      .where((p) => !configuredProvider(p))
      .toList();
  bool providerNeedsAttention(Json p) {
    if (p['active'] != true) return true;
    final ref = profile(p)['apiKeyEnv'];
    return ref is String &&
        ref.isNotEmpty &&
        credentials[ref]?['configured'] != true;
  }

  void resetConnection() {
    if (provider == null) return;
    base.text = '${profile(provider!)['baseURL'] ?? ''}';
    connectionName.text = '${profile(provider!)['displayName'] ?? ''}';
    connectionProtocol =
        '${profile(provider!)['api'] ?? modelProtocols(namespaces[provider!['settingsNs']]).firstOrNull ?? ''}';
    keyInput.clear();
    connectionDirty = false;
  }

  Future<void> settingsSaved() async {
    await widget.onSettingsChanged();
    final c = widget.controller,
        id = widget.controller.selectedId,
        owner = widget.controller.client;
    if (id == null || owner == null || !mounted) return;
    try {
      final value = await owner.models(id);
      if (mounted && c.client == owner && c.selectedId == id) {
        c.catalog = value;
        c.emit();
      }
    } catch (e) {
      if (mounted) setState(() => notice = '设置已保存，模型列表刷新失败：$e');
    }
  }

  Future<void> selectProvider(
    Json p, {
    bool force = false,
    bool preserveConnection = false,
    bool refreshUpstream = false,
  }) async {
    if (!force &&
        dirty &&
        !await confirmAction(
          context,
          '切换连接',
          '模型修改尚未保存，切换将放弃修改。',
          action: '放弃并切换',
        )) {
      return;
    }
    final epoch = ++generation;
    provider = p;
    advancedOpen = false;
    catalog = null;
    changes.clear();
    manual.clear();
    removed.clear();
    capacityDrafts.clear();
    invalidFields.clear();
    if (!preserveConnection) {
      connectionDirty = false;
      base.text = profile(p)['baseURL'] as String? ?? '';
      connectionName.text = '${profile(p)['displayName'] ?? ''}';
      connectionProtocol =
          '${profile(p)['api'] ?? modelProtocols(namespaces[p['settingsNs']]).firstOrNull ?? ''}';
      keyInput.clear();
    }
    if (mounted) setState(() {});
    final result = await api.request(
      refreshUpstream ? '/provider-auth/refresh' : '/provider-auth/models',
      body: {'provider': p['provider']},
      scope: scope,
    );
    if (!mounted || staleConnection || epoch != generation) return;
    setState(() => catalog = result);
  }

  Future<void> saveConnection() async {
    await run(() async {
      final p = provider!, ns = namespaces[p['settingsNs']]!;
      final path = (p['settingsPath'] as List? ?? []).cast<String>();
      final ops = <Json>[
        {
          'op': base.text.trim().isEmpty ? 'unset' : 'set',
          'path': [...path, 'baseURL'],
          if (base.text.trim().isNotEmpty) 'value': base.text.trim(),
        },
      ];
      final secret = keyInput.text.trim();
      if (p['settingsNs'] == 'llm-pi-ai') {
        ops.add({
          'op': connectionName.text.trim().isEmpty ? 'unset' : 'set',
          'path': [...path, 'displayName'],
          if (connectionName.text.trim().isNotEmpty)
            'value': connectionName.text.trim(),
        });
        if (connectionProtocol.isNotEmpty) {
          ops.add({
            'op': 'set',
            'path': [...path, 'api'],
            'value': connectionProtocol,
          });
        }
      }
      final reference = profile(p)['apiKeyEnv'];
      final keyRef = reference is String && reference.isNotEmpty
          ? reference
          : '${'${p['provider']}'.toUpperCase().replaceAll(RegExp(r'[^A-Z0-9]+'), '_')}_API_KEY';
      if (secret.isNotEmpty && reference != keyRef) {
        ops.add({
          'op': 'set',
          'path': [...path, 'apiKeyEnv'],
          'value': keyRef,
        });
      }
      final result = await api.call('settings.mutate', {
        'ns': p['settingsNs'],
        'expectedRevision': ns['revision'],
        'ops': ops,
      }, true);
      if (!mounted || staleConnection) return;
      namespaces[p['settingsNs'] as String] = result;
      if (secret.isNotEmpty) {
        await api.call('credentials.set', {
          'ref': keyRef,
          'value': secret,
        }, true);
        if (!mounted || staleConnection) return;
        credentials[keyRef] = {'configured': true, 'writable': true};
      }
      keyInput.clear();
      connectionDirty = false;
      editingConnection = false;
      addingProvider = false;
      notice = '连接已保存';
      if (connectionName.text.trim().isNotEmpty &&
          p['settingsNs'] == 'llm-pi-ai') {
        p['displayName'] = connectionName.text.trim();
      }
      if (modelDirty) {
        catalog?['namespaceRevision'] = result['revision'];
      } else {
        await selectProvider(p, force: true, refreshUpstream: true);
      }
      await settingsSaved();
    });
  }

  Future<void> saveModels() async {
    final ids = manual.map((m) => '${m['id'] ?? ''}'.trim()).toList();
    final existing = objects(catalog?['models'])
        .where((m) => !removed.contains(m['id']))
        .map((m) => m['id'])
        .toSet();
    if (invalidFields.isNotEmpty ||
        ids.any((id) => id.isEmpty || existing.contains(id)) ||
        ids.toSet().length != ids.length) {
      setState(() => error = '请修正模型 ID 或容量；容量支持正整数或 K/M。');
      return;
    }
    await run(() async {
      final old = catalog!;
      final latest = await api.request(
        '/provider-auth/models',
        body: {'provider': provider!['provider']},
        scope: scope,
      );
      if (!mounted || staleConnection) return;
      if (latest['accountScope'] != old['accountScope'] ||
          latest['namespaceRevision'] != old['namespaceRevision']) {
        throw StateError('连接配置已在其他窗口改变，请先刷新并核对草稿。');
      }
      final prefix = (latest['preferencePath'] as List).cast<String>();
      final ops = <Json>[];
      for (final entry in changes.entries) {
        for (final field in entry.value.entries) {
          ops.add({
            'op': field.value == null ? 'unset' : 'set',
            'path': [...prefix, entry.key, field.key],
            if (field.value != null) 'value': field.value,
          });
        }
      }
      final originals = objects(latest['profileModels']);
      if (manual.isNotEmpty ||
          originals.any((m) => removed.contains(m['id']))) {
        ops.add({
          'op': 'set',
          'path': [
            ...(latest['settingsPath'] as List? ??
                prefix.take(prefix.length - 2).toList()),
            'models',
          ],
          'value': [
            ...originals.where((m) => !removed.contains(m['id'])),
            ...manual.map(
              (m) => {
                for (final field in m.entries)
                  if (!field.key.startsWith('_') &&
                      field.value != null &&
                      field.value != '')
                    field.key: field.value,
                'source': 'manual',
                'accountScope': latest['accountScope'],
              },
            ),
          ],
        });
      }
      for (final id in removed) {
        if (!originals.any((m) => m['id'] == id)) {
          ops.add({
            'op': 'set',
            'path': [...prefix, id, 'removed'],
            'value': true,
          });
        }
      }
      if (ops.isNotEmpty) {
        final updated = await api.call('settings.mutate', {
          'ns': latest['settingsNs'],
          'expectedRevision': latest['namespaceRevision'],
          'ops': ops,
        }, true);
        if (!mounted || staleConnection) return;
        namespaces[latest['settingsNs'] as String] = updated;
      }
      await selectProvider(provider!, force: true, preserveConnection: true);
      notice = '模型设置已保存';
      await settingsSaved();
    });
  }

  @override
  Widget build(BuildContext context) {
    if (staleConnection) return const DshEmpty('连接已变化，请关闭后重新打开模型设置。');
    if (lastReportedDirty != dirty) {
      lastReportedDirty = dirty;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) widget.onDirtyChanged?.call(dirty);
      });
    }
    final colors = DshColors(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text(
          '模型',
          style: TextStyle(fontSize: 16, fontWeight: FontWeight.w500),
        ),
        const SizedBox(height: 12),
        Text(
          '选择连接，管理模型显示与参数；订阅登录在账号页管理。',
          style: TextStyle(fontSize: 12, color: colors.muted),
        ),
        const SizedBox(height: 12),
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final item in {
              'api': 'API 连接',
              'accounts': '订阅账号',
              'tasks': '任务分工',
            }.entries)
              DshButton(
                height: 38,
                fontSize: 14,
                onPressed: busy ? null : () => switchTab(item.key),
                active: tab == item.key,
                child: Text(item.value),
              ),
          ],
        ),
        const Divider(height: 20),
        if (error != null)
          Padding(
            padding: const EdgeInsets.only(bottom: 10),
            child: Text(
              error!,
              style: const TextStyle(color: Colors.red, fontSize: 12),
            ),
          ),
        if (notice != null)
          Text(
            notice!,
            style: const TextStyle(color: Colors.green, fontSize: 12),
          ),
        if (busy) const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: switch (tab) {
            'accounts' => accountsView(),
            'tasks' => TaskModelsPage(
              api: api,
              namespace: namespaces['task-models'],
            ),
            _ => apiView(),
          },
        ),
      ],
    );
  }

  Future<void> switchTab(String next) async {
    if (dirty &&
        !await confirmAction(
          context,
          '未保存的模型修改',
          '切换页面将放弃模型目录草稿。',
          action: '放弃修改',
        )) {
      return;
    }
    if (!mounted) return;
    setState(() {
      changes.clear();
      manual.clear();
      removed.clear();
      tab = next;
      connectionDirty = false;
      customDirty = false;
      capacityDrafts.clear();
      invalidFields.clear();
      keyInput.clear();
      declaring = false;
      addingProvider = false;
      editingConnection = false;
      advancedOpen = false;
      resetConnection();
    });
    if (next == 'accounts') {
      await run(() async {
        final result = await api.request(
          '/provider-auth/providers',
          body: {},
          scope: scope,
        );
        if (mounted) accounts = objects(result['providers']);
      });
    } else if (next == 'api' && provider != null && catalog == null) {
      await run(() => selectProvider(provider!, force: true));
    }
  }

  void updateModel(Json model, String field, Object? value) {
    setState(() {
      if (model['_draftId'] != null) {
        manual.firstWhere((m) => m['_draftId'] == model['_draftId'])[field] =
            value;
      } else {
        changes.putIfAbsent('${model['id']}', () => {})[field] = value;
      }
    });
  }

  Widget addProviderAction(String title, VoidCallback? onPressed) => SizedBox(
    height: 44,
    child: CustomPaint(
      foregroundPainter: _DashedBorderPainter(DshColors(context).border),
      child: DshButton(
        height: 44,
        icon: LucideIcons.plus,
        onPressed: onPressed,
        child: Text(title, style: const TextStyle(fontSize: 14)),
      ),
    ),
  );

  Future<void> removeModel(Json model) async {
    final key = '${model['_draftId'] ?? model['id']}';
    if (model['_draftId'] == null &&
        !await confirmAction(
          context,
          '删除模型',
          '移除 ${model['name'] ?? model['id']}，保留历史记录。',
          action: '移除',
        )) {
      return;
    }
    if (!mounted) return;
    setState(() {
      invalidFields.removeWhere((k) => k.startsWith('$key/'));
      capacityDrafts.remove(key);
      if (model['_draftId'] != null) {
        manual.removeWhere((m) => m['_draftId'] == model['_draftId']);
      } else {
        removed.add('${model['id']}');
        changes.remove('${model['id']}');
      }
    });
  }

  bool removableProvider(Json entry) {
    final path = (entry['settingsPath'] as List? ?? []).cast<String>();
    if (path.isEmpty || profile(entry)['authProvider'] != null) return false;
    bool exists(Object? root) {
      dynamic value = root;
      for (final part in path) {
        if (value is! Map || !value.containsKey(part)) return false;
        value = value[part];
      }
      return true;
    }

    final ns = namespaces[entry['settingsNs']];
    return exists(ns?['user'] ?? ns?['value']) && !exists(ns?['base']);
  }

  Future<void> removeConnection() async {
    final entry = provider;
    if (entry == null || busy || !removableProvider(entry)) return;
    final reference = profile(entry)['apiKeyEnv'];
    final managed =
        '${'${entry['provider']}'.toUpperCase().replaceAll(RegExp(r'[^A-Z0-9]+'), '_')}_API_KEY';
    final removeKey =
        reference == managed &&
        credentials[managed]?['configured'] == true &&
        credentials[managed]?['writable'] == true &&
        !apiProviders.any(
          (other) =>
              other['provider'] != entry['provider'] &&
              profile(other)['apiKeyEnv'] == managed,
        );
    if (!await confirmAction(
      context,
      '删除 API 连接',
      '删除 ${entry['displayName'] ?? entry['provider']} 的连接配置${removeKey ? '和此连接保存的 API 密钥' : ''}，保留会话记录。',
      action: '删除连接',
    )) {
      return;
    }
    if (!mounted || staleConnection) return;
    await run(() async {
      if (removeKey) {
        await api.call('credentials.unset', {'ref': managed}, true);
        if (!mounted || staleConnection) return;
      }
      await api.call('settings.mutate', {
        'ns': entry['settingsNs'],
        'expectedRevision': namespaces[entry['settingsNs']]?['revision'],
        'ops': [
          {'op': 'unset', 'path': entry['settingsPath']},
        ],
      }, true);
      if (!mounted || staleConnection) return;
      provider = null;
      catalog = null;
      changes.clear();
      manual.clear();
      removed.clear();
      capacityDrafts.clear();
      connectionDirty = false;
      keyInput.clear();
      await load();
      await settingsSaved();
    });
  }

  Widget connectionEditor() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      const SizedBox(height: 14),
      if (profile(provider!)['authProvider'] != null)
        const Text('此连接由订阅账号管理，请在订阅账号页续期或退出。')
      else ...[
        const Text('API 密钥', style: TextStyle(fontSize: 12)),
        const SizedBox(height: 6),
        DshField(
          controller: keyInput,
          hint:
              credentials[profile(provider!)['apiKeyEnv']]?['configured'] ==
                  true
              ? '已配置——输入新值可替换'
              : '输入 API 密钥',
          secret: true,
          enabled: !busy,
          onChanged: (_) => setState(() => connectionDirty = true),
        ),
        const SizedBox(height: 7),
        Divider(height: 12, color: DshColors(context).border),
        InkWell(
          onTap: () => setState(() => advancedOpen = !advancedOpen),
          child: SizedBox(
            height: 36,
            child: Row(
              children: [
                DshGlyph(
                  advancedOpen
                      ? LucideIcons.chevronDown
                      : LucideIcons.chevronRight,
                  size: 12,
                ),
                const SizedBox(width: 4),
                const Text('自定义设置', style: TextStyle(fontSize: 12)),
              ],
            ),
          ),
        ),
        const SizedBox(height: 3),
        if (advancedOpen) ...[
          if (provider!['settingsNs'] == 'llm-pi-ai') ...[
            DshField(
              controller: connectionName,
              hint: '显示名称',
              enabled: !busy,
              onChanged: (_) => setState(() => connectionDirty = true),
            ),
            const SizedBox(height: 8),
            DshSelect<String>(
              value: connectionProtocol,
              options: {
                for (final value in modelProtocols(
                  namespaces[provider!['settingsNs']],
                ))
                  value: value,
              },
              onChanged: busy
                  ? null
                  : (value) => setState(() {
                      connectionProtocol = value;
                      connectionDirty = true;
                    }),
            ),
            const SizedBox(height: 8),
          ],
          DshField(
            controller: base,
            hint: 'Base URL',
            enabled: !busy,
            onChanged: (_) => setState(() => connectionDirty = true),
          ),
          const SizedBox(height: 8),
        ],
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            DshButton(
              onPressed: busy
                  ? null
                  : () {
                      setState(() {
                        resetConnection();
                        advancedOpen = false;
                        editingConnection = false;
                        addingProvider = false;
                      });
                    },
              child: const Text('取消'),
            ),
            DshButton(
              primary: true,
              onPressed: busy ? null : saveConnection,
              child: const Text('保存'),
            ),
          ],
        ),
      ],
    ],
  );
  Future<void> openCreation(bool custom) async {
    if (busy) return;
    if (dirty &&
        !await confirmAction(
          context,
          '未保存的修改',
          '添加连接前是否放弃当前模型和连接草稿？',
          action: '放弃并继续',
        )) {
      return;
    }
    if (!mounted) return;
    setState(() {
      changes.clear();
      manual.clear();
      removed.clear();
      capacityDrafts.clear();
      invalidFields.clear();
      connectionDirty = false;
      customDirty = false;
      keyInput.clear();
      if (provider != null) {
        base.text = '${profile(provider!)['baseURL'] ?? ''}';
      }
      declaring = custom;
      addingProvider = !custom;
      editingConnection = false;
      advancedOpen = false;
    });
    if (!custom && addableProviders.isNotEmpty) {
      await run(() => selectProvider(addableProviders.first, force: true));
    }
  }

  Widget apiView() {
    final colors = DshColors(context);
    final all = [
      ...objects(catalog?['models'])
          .where((m) => !removed.contains(m['id']))
          .map((m) => {...m, ...?changes[m['id']]}),
      ...manual,
    ];
    final rows = all
        .where(
          (m) =>
              m['_draftId'] != null ||
              ('${m['name']} ${m['id']}'.toLowerCase().contains(
                    search.text.toLowerCase(),
                  ) &&
                  (visibility == 'all' ||
                      (m['enabled'] != false) == (visibility == 'shown'))),
        )
        .toList();
    final shown = [
      ...rows
          .where((m) => m['_draftId'] == null)
          .take(modelsExpanded ? modelLimit : 6),
      ...rows.where((m) => m['_draftId'] != null),
    ];
    final matches = configuredApiProviders
        .where(
          (p) =>
              (!needsAttention || providerNeedsAttention(p)) &&
              '${p['displayName']} ${p['provider']} ${objects(profile(p)['models']).map((m) => '${m['id']} ${m['name'] ?? ''}').join(' ')}'
                  .toLowerCase()
                  .contains(providerSearch.text.toLowerCase()),
        )
        .toList();
    return ListView(
      children: [
        Row(
          children: [
            Expanded(
              child: DshField(
                controller: providerSearch,
                hint: '搜索连接或模型 ID',
                onChanged: (_) => setState(() {}),
              ),
            ),
            const SizedBox(width: 10),
            DshButton(
              height: 36,
              outline: true,
              active: needsAttention,
              onPressed: () => setState(() => needsAttention = !needsAttention),
              child: const Text('仅待修复'),
            ),
          ],
        ),
        if (matches.isEmpty)
          const Padding(
            padding: EdgeInsets.only(top: 12),
            child: Text('没有匹配的提供方', style: TextStyle(fontSize: 12)),
          ),
        const SizedBox(height: 12),
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final p in matches)
              DshButton(
                height: 34,
                outline: true,
                active: provider?['provider'] == p['provider'],
                activeBorderColor: colors.text,
                activeBackgroundColor: colors.base,
                onPressed: busy ? null : () => run(() => selectProvider(p)),
                child: Text(
                  '${p['displayName'] ?? p['provider']}',
                  style: const TextStyle(fontSize: 13),
                ),
              ),
          ],
        ),
        const SizedBox(height: 10),
        if (provider != null &&
            matches.any((p) => p['provider'] == provider!['provider']))
          Container(
            padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
            decoration: BoxDecoration(
              border: Border.all(color: colors.border),
              borderRadius: BorderRadius.circular(12),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Row(
                        children: [
                          Flexible(
                            child: Text(
                              '${provider!['displayName'] ?? provider!['provider']}',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: const TextStyle(
                                fontSize: 14,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                          ),
                          if (profile(provider!)['apiKeyEnv'] is String &&
                              credentials.containsKey(
                                profile(provider!)['apiKeyEnv'],
                              )) ...[
                            const SizedBox(width: 7),
                            Tooltip(
                              message:
                                  object(
                                        credentials[profile(
                                          provider!,
                                        )['apiKeyEnv']],
                                      )['configured'] ==
                                      true
                                  ? '已配置 API 密钥'
                                  : '缺少 API 密钥',
                              child: Container(
                                width: 8,
                                height: 8,
                                decoration: BoxDecoration(
                                  color:
                                      object(
                                            credentials[profile(
                                              provider!,
                                            )['apiKeyEnv']],
                                          )['configured'] ==
                                          true
                                      ? const Color(0xff22c55e)
                                      : const Color(0xffec1313),
                                  shape: BoxShape.circle,
                                ),
                              ),
                            ),
                          ],
                        ],
                      ),
                    ),
                    DshButton(
                      height: 36,
                      outline: true,
                      onPressed: busy
                          ? null
                          : () => setState(
                              () => editingConnection = !editingConnection,
                            ),
                      child: const Text('编辑连接'),
                    ),
                    if (removableProvider(provider!)) ...[
                      const SizedBox(width: 8),
                      DshButton(
                        outline: true,
                        destructive: true,
                        onPressed: busy ? null : removeConnection,
                        child: const Text('删除连接'),
                      ),
                    ],
                  ],
                ),
                const SizedBox(height: 12),
                DshButton(
                  height: 28,
                  onPressed: () =>
                      setState(() => modelsExpanded = !modelsExpanded),
                  child: Text(
                    modelsExpanded ? '收起模型' : '展开模型',
                    style: const TextStyle(fontSize: 12),
                  ),
                ),
                if (modelsExpanded) ...[
                  const SizedBox(height: 24),
                  const Divider(height: 1),
                  const SizedBox(height: 10),
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          '模型目录 · ${all.where((m) => m['enabled'] != false).length} / ${all.length} 显示',
                          style: TextStyle(fontSize: 12, color: colors.muted),
                        ),
                      ),
                      DshButton(
                        height: 36,
                        onPressed: busy
                            ? null
                            : () => run(
                                () => selectProvider(
                                  provider!,
                                  refreshUpstream: true,
                                ),
                              ),
                        child: const Text(
                          '刷新目录',
                          style: TextStyle(fontSize: 12),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 8),
                  LayoutBuilder(
                    builder: (context, constraints) => Wrap(
                      spacing: 8,
                      runSpacing: 8,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        SizedBox(
                          width: (constraints.maxWidth * .54).clamp(180, 420),
                          child: DshField(
                            controller: search,
                            hint: '搜索模型名称或 ID',
                            onChanged: (_) => setState(() => modelLimit = 100),
                          ),
                        ),
                        DshSelect<String>(
                          value: visibility,
                          outline: true,
                          maxWidth: 100,
                          options: const {
                            'all': '全部模型',
                            'shown': '显示',
                            'hidden': '隐藏',
                          },
                          onChanged: (v) => setState(() => visibility = v),
                        ),
                        for (final enabled in [true, false])
                          DshButton(
                            height: 36,
                            outline: true,
                            padding: const EdgeInsets.symmetric(horizontal: 14),
                            onPressed: busy || rows.isEmpty
                                ? null
                                : () {
                                    for (final m in rows) {
                                      updateModel(m, 'enabled', enabled);
                                    }
                                  },
                            child: Text(
                              enabled ? '显示筛选结果' : '隐藏筛选结果',
                              style: const TextStyle(fontSize: 14),
                            ),
                          ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 16),
                  if (catalog == null)
                    const Padding(
                      padding: EdgeInsets.all(16),
                      child: Text('正在读取模型目录…'),
                    )
                  else
                    ConstrainedBox(
                      constraints: const BoxConstraints(maxHeight: 360),
                      child: ListView.builder(
                        shrinkWrap: true,
                        itemCount: shown.length,
                        itemBuilder: (context, index) {
                          final m = shown[index],
                              rowKey = '${m['_draftId'] ?? m['id']}';
                          return InlineModelRow(
                            key: ValueKey(
                              '${provider!['provider']}/$generation/$rowKey',
                            ),
                            model: m,
                            manual: m['_draftId'] != null,
                            expanded: modelsExpanded,
                            enabled: !busy,
                            rawValues: capacityDrafts[rowKey] ?? const {},
                            onDraft: (field, text) {
                              capacityDrafts.putIfAbsent(
                                rowKey,
                                () => {},
                              )[field] = text;
                            },
                            onChange: (field, value) =>
                                updateModel(m, field, value),
                            onInvalid: (field, bad) => setState(
                              () => bad
                                  ? invalidFields.add('$rowKey/$field')
                                  : invalidFields.remove('$rowKey/$field'),
                            ),
                            onRemove: () => removeModel(m),
                          );
                        },
                      ),
                    ),
                  if (shown.isEmpty && catalog != null)
                    const Text('没有匹配的模型', style: TextStyle(fontSize: 12)),
                  if (rows.length > shown.length)
                    DshButton(
                      onPressed: () => setState(() {
                        modelsExpanded = true;
                        modelLimit += 100;
                      }),
                      child: Text('还有 ${rows.length - shown.length} 个模型'),
                    ),
                  DshButton(
                    icon: LucideIcons.plus,
                    height: 30,
                    onPressed: busy || catalog == null
                        ? null
                        : () => setState(
                            () => manual.add({
                              '_draftId': 'manual-${++draftSequence}',
                              'id': '',
                              'enabled': true,
                            }),
                          ),
                    child: const Text('添加手动模型', style: TextStyle(fontSize: 12)),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    '显示开关仅控制模型选择列表；保存后生效，不会删除会话或更改运行中的任务。',
                    style: TextStyle(fontSize: 12, color: colors.muted),
                  ),
                  if (invalidFields.isNotEmpty)
                    const Text(
                      '容量必须为正整数，可使用 K/M。',
                      style: TextStyle(fontSize: 12, color: Colors.red),
                    ),
                  if (modelDirty)
                    Wrap(
                      spacing: 8,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        const Text('模型有未保存修改', style: TextStyle(fontSize: 12)),
                        DshButton(
                          onPressed: busy
                              ? null
                              : () => run(
                                  () => selectProvider(provider!, force: true),
                                ),
                          child: const Text('取消'),
                        ),
                        DshButton(
                          primary: true,
                          onPressed: busy || invalidFields.isNotEmpty
                              ? null
                              : saveModels,
                          child: const Text('保存模型'),
                        ),
                      ],
                    ),
                ],
                if (editingConnection) ...[
                  const SizedBox(height: 12),
                  Container(
                    padding: const EdgeInsets.all(16),
                    decoration: BoxDecoration(
                      color: colors.layer,
                      borderRadius: BorderRadius.circular(10),
                    ),
                    child: Material(
                      type: MaterialType.transparency,
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Row(
                            children: [
                              Text(
                                '${provider!['displayName'] ?? provider!['provider']}',
                                style: const TextStyle(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w600,
                                ),
                              ),
                              const SizedBox(width: 8),
                              Text(
                                '${provider!['provider']}',
                                style: TextStyle(
                                  fontSize: 12,
                                  color: colors.muted,
                                ),
                              ),
                            ],
                          ),
                          connectionEditor(),
                        ],
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
        const SizedBox(height: 16),
        if (addingProvider)
          Container(
            padding: const EdgeInsets.all(16),
            decoration: BoxDecoration(
              border: Border.all(color: colors.border),
              borderRadius: BorderRadius.circular(12),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text('提供方'),
                const SizedBox(height: 8),
                DshSelect<String>(
                  value: '${provider?['provider'] ?? ''}',
                  options: {
                    for (final p in addableProviders)
                      '${p['provider']}':
                          '${p['displayName'] ?? p['provider']}',
                  },
                  onChanged: busy
                      ? null
                      : (id) => run(
                          () => selectProvider(
                            providers.firstWhere((p) => p['provider'] == id),
                          ),
                        ),
                ),
                if (provider != null) connectionEditor(),
              ],
            ),
          ),
        if (declaring && namespaces['llm-pi-ai'] != null)
          Container(
            padding: const EdgeInsets.all(16),
            decoration: BoxDecoration(
              border: Border.all(color: colors.border),
              borderRadius: BorderRadius.circular(12),
            ),
            child: CustomProviderCard(
              api: api,
              namespace: namespaces['llm-pi-ai']!,
              taken: providers.map((p) => '${p['provider']}').toSet(),
              onDirtyChanged: (v) => setState(() => customDirty = v),
              isCurrent: () => mounted && !staleConnection,
              onCancel: () {
                setState(() {
                  declaring = false;
                  customDirty = false;
                });
              },
              onSaved: () async {
                setState(() {
                  declaring = false;
                  customDirty = false;
                  provider = null;
                });
                await load();
              },
            ),
          ),
        if (!addingProvider && !declaring)
          LayoutBuilder(
            builder: (context, constraints) {
              final first = addProviderAction(
                '添加提供方',
                busy || addableProviders.isEmpty
                    ? null
                    : () => openCreation(false),
              );
              final second = addProviderAction(
                '添加自定义提供方',
                busy || namespaces['llm-pi-ai'] == null
                    ? null
                    : () => openCreation(true),
              );
              if (constraints.maxWidth < 560) {
                return Column(
                  children: [first, const SizedBox(height: 8), second],
                );
              }
              return Row(
                children: [
                  Expanded(child: first),
                  const SizedBox(width: 8),
                  Expanded(child: second),
                ],
              );
            },
          ),
      ],
    );
  }

  Widget accountsView() {
    final colors = DshColors(context);
    return ListView(
      children: [
        Container(
          padding: const EdgeInsets.all(14),
          decoration: BoxDecoration(
            border: Border.all(color: colors.border),
            borderRadius: BorderRadius.circular(12),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  const Text('账号登录', style: TextStyle(fontSize: 14)),
                  const SizedBox(width: 8),
                  Text(
                    '${accounts.where((a) => a['signedIn'] == true).length} / ${accounts.length}',
                    style: TextStyle(fontSize: 12, color: colors.muted),
                  ),
                  const SizedBox(width: 12),
                  DshButton(
                    outline: true,
                    height: 32,
                    fontSize: 12,
                    onPressed: busy ? null : () => run(refreshAccounts),
                    child: const Text('刷新状态'),
                  ),
                ],
              ),
              const SizedBox(height: 12),
              Text(
                '使用供应商订阅登录，凭据保存在本机；支持续期的供应商会自动续期。',
                style: TextStyle(
                  fontSize: 12,
                  height: 1.6,
                  color: colors.muted,
                ),
              ),
              const SizedBox(height: 8),
              for (final account in accounts)
                Container(
                  decoration: BoxDecoration(
                    border: Border(bottom: BorderSide(color: colors.border)),
                  ),
                  child: Theme(
                    data: Theme.of(context)
                        .copyWith(dividerColor: Colors.transparent),
                    child: ExpansionTile(
                      key: PageStorageKey('account-${account['id']}'),
                      tilePadding: EdgeInsets.zero,
                      childrenPadding: const EdgeInsets.only(bottom: 16),
                      minTileHeight: 74,
                      controlAffinity: ListTileControlAffinity.leading,
                      leading: AnimatedRotation(
                        turns: expandedAccounts.contains('${account['id']}')
                            ? .25
                            : 0,
                        duration: const Duration(milliseconds: 120),
                        child: const DshGlyph(
                          LucideIcons.chevronRight,
                          size: 16,
                        ),
                      ),
                      onExpansionChanged: (open) => setState(() {
                        if (open) {
                          expandedAccounts.add('${account['id']}');
                        } else {
                          expandedAccounts.remove('${account['id']}');
                        }
                      }),
                      textColor: colors.text,
                      collapsedTextColor: colors.text,
                      title: Row(
                        children: [
                          Flexible(
                            child: Text(
                              '${account['name'] ?? account['id']}',
                              style: const TextStyle(
                                fontSize: 14,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                          ),
                          const SizedBox(width: 8),
                          Container(
                            padding: const EdgeInsets.symmetric(
                              horizontal: 5,
                              vertical: 2,
                            ),
                            decoration: BoxDecoration(
                              border: Border.all(color: colors.border),
                              borderRadius: BorderRadius.circular(4),
                            ),
                            child: Text(
                              '${account['signedIn'] == true ? '已连接' : '未连接'}${account['scope'] == 'subagent' ? ' · 子智能体' : ''}',
                              style: TextStyle(
                                fontSize: 11,
                                color: colors.muted,
                              ),
                            ),
                          ),
                        ],
                      ),
                      children: [
                        if (account['error'] != null)
                          Text(
                            '${account['error']}',
                            style: const TextStyle(
                              color: Colors.red,
                              fontSize: 12,
                            ),
                          ),
                        Wrap(
                          spacing: 8,
                          runSpacing: 8,
                          children: [
                            DshButton(
                              outline: true,
                              height: 32,
                              fontSize: 13,
                              onPressed: busy || account['installed'] == false
                                  ? null
                                  : account['signedIn'] == true
                                  ? () => run(() async {
                                      await api.request(
                                        '/provider-auth/connect',
                                        body: {'provider': account['id']},
                                        mutation: true,
                                        scope: scope,
                                      );
                                      await refreshAccountState();
                                    })
                                  : () => showDialog<void>(
                                      context: context,
                                      builder: (_) => AccountLoginDialog(
                                        api: api,
                                        provider: '${account['id']}',
                                        onComplete: refreshAccountState,
                                      ),
                                    ),
                              child: Text(
                                account['signedIn'] == true
                                    ? account['scope'] == 'subagent'
                                          ? '刷新'
                                          : '重新连接'
                                    : '登录',
                              ),
                            ),
                            if (account['signedIn'] == true &&
                                account['scope'] != 'subagent') ...[
                              DshButton(
                                outline: true,
                                height: 32,
                                fontSize: 13,
                                onPressed: busy || account['installed'] == false
                                    ? null
                                    : () => showDialog<void>(
                                        context: context,
                                        builder: (_) => AccountLoginDialog(
                                          api: api,
                                          provider: '${account['id']}',
                                          onComplete: refreshAccountState,
                                        ),
                                      ),
                                child: const Text('登录另一个账号'),
                              ),
                              DshButton(
                                outline: true,
                                destructive: true,
                                height: 32,
                                fontSize: 13,
                                onPressed: busy
                                    ? null
                                    : () => removeAccount(account),
                                child: const Text('退出登录'),
                              ),
                            ],
                            if (account['installed'] == false &&
                                (account['installUrl'] is String ||
                                    account['docsUrl'] is String))
                              DshButton(
                                onPressed: () async {
                                  final uri = Uri.tryParse(
                                    '${account['installUrl'] ?? account['docsUrl']}',
                                  );
                                  if (uri != null && uri.scheme == 'https') {
                                    await launchUrl(
                                      uri,
                                      mode: LaunchMode.externalApplication,
                                    );
                                  }
                                },
                                child: const Text('安装官方客户端'),
                              ),
                          ],
                        ),
                        if (account['scope'] != 'subagent')
                          for (final saved in objects(account['accounts']))
                            ListTile(
                              contentPadding: EdgeInsets.zero,
                              dense: true,
                              title: Text(
                                '${saved['label'] ?? saved['accountId'] ?? saved['accountScope']}',
                                style: const TextStyle(fontSize: 13),
                              ),
                              subtitle: saved['needsLogin'] == true
                                  ? const Text(
                                      '需要重新登录',
                                      style: TextStyle(fontSize: 12),
                                    )
                                  : null,
                              trailing: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  DshButton(
                                    height: 30,
                                    fontSize: 12,
                                    onPressed:
                                        busy ||
                                            saved['active'] == true ||
                                            saved['needsLogin'] == true
                                        ? null
                                        : () => run(() async {
                                            await api.request(
                                              '/provider-auth/switch',
                                              body: {
                                                'provider': account['id'],
                                                'accountScope':
                                                    saved['accountScope'],
                                              },
                                              mutation: true,
                                              scope: scope,
                                            );
                                            await refreshAccounts();
                                            await api.request(
                                              '/provider-auth/refresh',
                                              body: {'provider': account['id']},
                                              mutation: true,
                                              scope: scope,
                                            );
                                            await refreshAccountState();
                                          }),
                                    child: Text(
                                      saved['active'] == true ? '当前账号' : '切换',
                                    ),
                                  ),
                                  DshIcon(
                                    LucideIcons.trash2,
                                    label: '移除账号',
                                    onPressed: busy
                                        ? null
                                        : () => removeAccount(account, saved),
                                  ),
                                ],
                              ),
                            ),
                      ],
                    ),
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }

  Future<void> removeAccount(Json provider, [Json? account]) async {
    if (!await confirmAction(
      context,
      account == null ? '退出登录' : '移除账号',
      '移除本机保存的授权；已有会话记录保留。',
      action: '移除授权',
    )) {
      return;
    }
    if (!mounted) return;
    await run(() async {
      await api.request(
        '/provider-auth/logout',
        body: {
          'provider': provider['id'],
          if (account != null) 'accountScope': account['accountScope'],
        },
        scope: scope,
        mutation: true,
      );
      await refreshAccountState();
    });
  }

  Future<void> refreshAccounts() async {
    final result = await api.request(
      '/provider-auth/providers',
      body: {},
      scope: scope,
    );
    if (mounted) setState(() => accounts = objects(result['providers']));
    if (mounted) await widget.controller.loadAccounts();
  }

  Future<void> refreshAccountState() async {
    await refreshAccounts();
    if (mounted && !staleConnection) {
      await widget.controller.loadCatalogs();
    }
  }
}

class AccountLoginDialog extends StatefulWidget {
  const AccountLoginDialog({
    super.key,
    required this.api,
    required this.provider,
    required this.onComplete,
  });
  final DshClient api;
  final String provider;
  final Future<void> Function() onComplete;
  @override
  State<AccountLoginDialog> createState() => _AccountLoginDialogState();
}

class _AccountLoginDialogState extends State<AccountLoginDialog> {
  late final login = AccountLogin(widget.api, widget.provider);
  bool finishing = false;
  String? refreshError;
  @override
  void initState() {
    super.initState();
    login.addListener(changed);
    login.start();
  }

  void changed() {
    if (!mounted) return;
    setState(() {});
    if (login.complete && !finishing) {
      finishing = true;
      unawaited(finish());
    }
  }

  Future<void> finish() async {
    try {
      await widget.onComplete();
      if (mounted) Navigator.pop(context);
    } catch (e) {
      if (mounted) setState(() => refreshError = '账号已连接，但刷新列表失败：$e');
    }
  }

  @override
  void dispose() {
    login.removeListener(changed);
    login.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('连接订阅账号', style: TextStyle(fontSize: 17)),
    content: SizedBox(
      width: 450,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (login.starting)
            const Center(child: CircularProgressIndicator(strokeWidth: 2)),
          if (login.attempt != null) ...[
            Text(
              login.cli
                  ? '请在官方客户端中完成授权；此处会自动同步登录结果。'
                  : '在系统浏览器中完成授权；登录结果和账号配置由本机服务统一保存。',
              style: const TextStyle(fontSize: 14, height: 1.6),
            ),
            if (login.attempt!['userCode'] != null)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 20),
                child: SelectableText(
                  '${login.attempt!['userCode']}',
                  style: const TextStyle(fontSize: 24, letterSpacing: 3),
                ),
              ),
            const SizedBox(height: 12),
            if (login.authorizationUri != null)
              DshButton(
                primary: true,
                pill: true,
                onPressed: login.expired
                    ? null
                    : () async {
                        try {
                          if (!await launchUrl(
                            login.authorizationUri!,
                            mode: LaunchMode.externalApplication,
                          )) {
                            throw StateError('无法打开系统浏览器');
                          }
                        } catch (e) {
                          if (mounted) setState(() => refreshError = '$e');
                        }
                      },
                child: const Text('打开授权页面'),
              ),
            if (login.attempt!['userCode'] != null)
              DshButton(
                onPressed: () => Clipboard.setData(
                  ClipboardData(text: '${login.attempt!['userCode']}'),
                ),
                child: const Text('复制验证码'),
              ),
            if (!login.expired && !login.complete)
              const Padding(
                padding: EdgeInsets.only(top: 12),
                child: Text('等待授权完成…', style: TextStyle(fontSize: 12)),
              ),
          ],
          if (login.notice != null)
            Text(login.notice!, style: const TextStyle(fontSize: 12)),
          if (login.error != null || refreshError != null)
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: Text(
                refreshError ?? login.error!,
                style: const TextStyle(color: Colors.red, fontSize: 12),
              ),
            ),
        ],
      ),
    ),
    actions: [
      if (login.error != null && !login.expired && login.attempt != null)
        DshButton(
          onPressed: login.polling ? null : login.poll,
          child: const Text('重新检查'),
        ),
      if ((login.expired || login.error != null) && !login.complete)
        DshButton(
          onPressed: login.starting || login.polling ? null : login.start,
          child: const Text('重新登录'),
        ),
      DshButton(
        onPressed: () => Navigator.pop(context),
        child: Text(login.complete ? '关闭' : '取消'),
      ),
    ],
  );
}

class TaskModelsPage extends StatefulWidget {
  const TaskModelsPage({super.key, required this.api, this.namespace});
  final DshClient api;
  final Json? namespace;
  @override
  State<TaskModelsPage> createState() => _TaskModelsPageState();
}

class _TaskModelsPageState extends State<TaskModelsPage> {
  final fields = <String, TextEditingController>{};
  String? error;
  bool busy = false;
  @override
  void dispose() {
    for (final f in fields.values) {
      f.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListView(
    children: [
      const Text('为辅助任务指定模型', style: TextStyle(fontSize: 14)),
      for (final role in {
        'diagnose': '排错',
        'optimize': '优化',
        'vision': '看图',
        'image': '生图',
        'search': '搜索',
      }.entries)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(role.value),
              const SizedBox(height: 8),
              Row(
                children: [
                  for (final key in ['provider', 'model', 'reasoningEffort'])
                    Expanded(
                      child: Padding(
                        padding: const EdgeInsets.only(right: 8),
                        child: DshField(
                          controller: fields.putIfAbsent(
                            '${role.key}/$key',
                            () => TextEditingController(
                              text:
                                  object(
                                        object(
                                          widget.namespace?['value'],
                                        )[role.key],
                                      )[key]
                                      as String? ??
                                  '',
                            ),
                          ),
                          hint: {
                            'provider': '提供方',
                            'model': '模型 ID',
                            'reasoningEffort': '推理等级',
                          }[key],
                        ),
                      ),
                    ),
                ],
              ),
            ],
          ),
        ),
      if (error != null)
        Text(error!, style: const TextStyle(color: Colors.red)),
      Align(
        alignment: Alignment.centerRight,
        child: DshButton(
          primary: true,
          onPressed: busy
              ? null
              : () async {
                  setState(() => busy = true);
                  try {
                    await widget.api.call('settings.mutate', {
                      'ns': 'task-models',
                      'expectedRevision': widget.namespace?['revision'],
                      'ops': [
                        for (final entry in fields.entries)
                          {
                            'op': entry.value.text.isEmpty ? 'unset' : 'set',
                            'path': entry.key.split('/'),
                            if (entry.value.text.isNotEmpty)
                              'value': entry.value.text,
                          },
                      ],
                    }, true);
                    if (mounted) setState(() => error = '已保存');
                  } catch (e) {
                    if (mounted) setState(() => error = '$e');
                  } finally {
                    if (mounted) setState(() => busy = false);
                  }
                },
          child: const Text('保存任务模型'),
        ),
      ),
    ],
  );
}
