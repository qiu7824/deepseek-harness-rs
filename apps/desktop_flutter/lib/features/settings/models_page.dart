import '../../design/error.dart';
import '../../l10n/zh.dart';
import '../../l10n/conversation_zh.dart';

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../design/primitives.dart';
import '../../design/loading.dart';
import '../../design/select.dart';
import '../../design/motion.dart';
import 'account_login.dart';
import 'model_editor_widgets.dart';
import 'task_models_page.dart';
export 'task_models_page.dart' show TaskModelsPage;
import '../../src/controller.dart';
import '../account_menu.dart'
    show
        accountNeedsLogin,
        activeAccount,
        linkedAccountProviders,
        maskedAccountLabel;

import 'package:dsh_desktop/design/typography.dart';

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
    this.initialAccountProvider,
    this.onDirtyChanged,
  });
  final DesktopController controller;
  final Future<void> Function() onSettingsChanged;
  final String initialTab;
  final String? initialAccountProvider;
  final ValueChanged<bool>? onDirtyChanged;
  @override
  State<ModelsPage> createState() => _ModelsPageState();
}

class _ModelsPageState extends State<ModelsPage> {
  DshClient? boundApi;
  DshClient get api =>
      boundApi ?? (throw StateError(DshSettingsZh.connectFirst));
  bool get staleConnection =>
      boundApi == null || widget.controller.client != boundApi;
  void connectionChanged() {
    if (staleConnection && mounted) {
      scope.cancel();
      keyInput.clear();
      setState(() => error = DshSettingsZh.modelConnectionChanged);
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
  late final expandedAccounts = <String>{?widget.initialAccountProvider};
  final focusedAccount = GlobalKey();
  bool focusedAccountShown = false;
  late String tab = widget.initialTab;
  bool busy = false;
  bool accountLogoutOpen = false;
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
          notice = DshSettingsZh.credentialsUnavailable(detail: e);
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
    final c = widget.controller,
        owner = widget.controller.client,
        host = widget.controller.host;
    await widget.onSettingsChanged();
    if (owner == null ||
        !mounted ||
        !identical(c, widget.controller) ||
        !identical(owner, c.client) ||
        !identical(host, c.host)) {
      return;
    }
    try {
      c.invalidateModelCatalog();
      await c.refreshModels();
    } catch (e) {
      if (mounted) {
        setState(() => notice = DshSettingsZh.modelsRefreshFailed(detail: e));
      }
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
          DshSettingsZh.switchConnection,
          DshSettingsZh.discardModelHint,
          action: DshSettingsZh.discardAndSwitch,
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
      notice = DshSettingsZh.connectionSaved;
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
      setState(() => error = DshSettingsZh.modelFieldsInvalid);
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
        throw StateError(DshSettingsZh.connectionConflict);
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
      notice = DshSettingsZh.modelsSaved;
      await settingsSaved();
    });
  }

  @override
  Widget build(BuildContext context) {
    if (staleConnection) {
      return const DshEmpty(DshSettingsZh.modelConnectionChanged);
    }
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
          DshSettingsZh.models,
          style: TextStyle(
            fontSize: DshTypography.sizeComposer,
            fontWeight: FontWeight.w500,
          ),
        ),
        const SizedBox(height: 12),
        Text(
          DshSettingsZh.modelsDescription,
          style: TextStyle(
            fontSize: DshTypography.sizeCaption,
            color: colors.muted,
          ),
        ),
        const SizedBox(height: 12),
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final item in {
              'api': DshSettingsZh.apiConnections,
              'accounts': DshSettingsZh.subscriptionAccounts,
              'tasks': DshSettingsZh.taskRoles,
            }.entries)
              DshButton(
                height: 38,
                fontSize: DshTypography.sizeBody,
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
            child: DshErrorView(error: error!),
          ),
        if (notice != null)
          Text(
            notice!,
            style: TextStyle(
              color: DshTokens.of(context).success.foreground,
              fontSize: DshTypography.sizeCaption,
            ),
          ),
        if (busy) const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: busy && providers.isEmpty && namespaces.isEmpty
              ? DshListSkeleton(
                  label: DshConversationZh.loadingList(
                    name: DshSettingsZh.models,
                  ),
                )
              : switch (tab) {
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
          DshSettingsZh.unsavedModels,
          DshSettingsZh.discardCatalogHint,
          action: DshSettingsZh.discardChanges,
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
        icon: DshIcons.plus.data,
        onPressed: onPressed,
        child: Text(
          title,
          style: const TextStyle(fontSize: DshTypography.sizeBody),
        ),
      ),
    ),
  );

  Future<void> removeModel(Json model) async {
    final key = '${model['_draftId'] ?? model['id']}';
    if (model['_draftId'] == null &&
        !await confirmAction(
          context,
          DshSettingsZh.deleteModel,
          DshSettingsZh.removeModelHint(name: model['name'] ?? model['id']),
          action: DshSettingsZh.remove,
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
      DshSettingsZh.deleteApiConnection,
      DshSettingsZh.deleteConnectionHint(
        name: entry['displayName'] ?? entry['provider'],
        keyEffect: removeKey ? DshSettingsZh.deleteApiKeyAlso : '',
      ),
      action: DshSettingsZh.deleteConnection,
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
        const Text(DshSettingsZh.subscriptionManaged)
      else ...[
        const Text(
          DshSettingsZh.apiKey,
          style: TextStyle(fontSize: DshTypography.sizeCaption),
        ),
        const SizedBox(height: 6),
        DshField(
          controller: keyInput,
          hint:
              credentials[profile(provider!)['apiKeyEnv']]?['configured'] ==
                  true
              ? DshSettingsZh.replaceApiKey
              : DshSettingsZh.apiKeyHint,
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
                      ? DshIcons.chevronDown.data
                      : DshIcons.chevronRight.data,
                  size: 12,
                ),
                const SizedBox(width: 4),
                const Text(
                  DshSettingsZh.customSettings,
                  style: TextStyle(fontSize: DshTypography.sizeCaption),
                ),
              ],
            ),
          ),
        ),
        const SizedBox(height: 3),
        if (advancedOpen) ...[
          if (provider!['settingsNs'] == 'llm-pi-ai') ...[
            DshField(
              controller: connectionName,
              hint: DshSettingsZh.displayName,
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
              child: const Text(DshZh.cancel),
            ),
            DshButton(
              primary: true,
              onPressed: busy ? null : saveConnection,
              child: const Text(DshZh.save),
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
          DshSettingsZh.unsavedChanges,
          DshSettingsZh.discardConnectionHint,
          action: DshSettingsZh.discardAndContinue,
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
                hint: DshSettingsZh.searchModels,
                onChanged: (_) => setState(() {}),
              ),
            ),
            const SizedBox(width: 10),
            DshButton(
              height: 36,
              outline: true,
              active: needsAttention,
              onPressed: () => setState(() => needsAttention = !needsAttention),
              child: const Text(DshSettingsZh.repairOnly),
            ),
          ],
        ),
        if (matches.isEmpty)
          const Padding(
            padding: EdgeInsets.only(top: 12),
            child: Text(
              DshSettingsZh.noProviders,
              style: TextStyle(fontSize: DshTypography.sizeCaption),
            ),
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
                  style: const TextStyle(fontSize: DshTypography.sizeAuxiliary),
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
                                fontSize: DshTypography.sizeBody,
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
                                  ? DshSettingsZh.apiKeyConfigured
                                  : DshSettingsZh.apiKeyMissing,
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
                                      ? DshTokens.of(context).success.foreground
                                      : DshTokens.of(context).error.foreground,
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
                      child: const Text(DshSettingsZh.editConnection),
                    ),
                    if (removableProvider(provider!)) ...[
                      const SizedBox(width: 8),
                      DshButton(
                        outline: true,
                        destructive: true,
                        onPressed: busy ? null : removeConnection,
                        child: const Text(DshSettingsZh.deleteConnection),
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
                    modelsExpanded
                        ? DshSettingsZh.collapseModels
                        : DshSettingsZh.expandModels,
                    style: const TextStyle(fontSize: DshTypography.sizeCaption),
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
                          DshSettingsZh.modelCatalogCount(
                            enabled: all
                                .where((m) => m['enabled'] != false)
                                .length,
                            total: all.length,
                          ),
                          style: TextStyle(
                            fontSize: DshTypography.sizeCaption,
                            color: colors.muted,
                          ),
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
                          DshSettingsZh.refreshCatalog,
                          style: TextStyle(fontSize: DshTypography.sizeCaption),
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
                            hint: DshSettingsZh.searchModelNames,
                            onChanged: (_) => setState(() => modelLimit = 100),
                          ),
                        ),
                        DshSelect<String>(
                          value: visibility,
                          outline: true,
                          maxWidth: 100,
                          options: const {
                            'all': DshSettingsZh.allModels,
                            'shown': DshSettingsZh.show,
                            'hidden': DshSettingsZh.hide,
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
                              enabled
                                  ? DshSettingsZh.showFiltered
                                  : DshSettingsZh.hideFiltered,
                              style: const TextStyle(
                                fontSize: DshTypography.sizeBody,
                              ),
                            ),
                          ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 16),
                  if (catalog == null)
                    const DshListSkeleton(
                      rows: 3,
                      label: DshSettingsZh.loadingModels,
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
                    const Text(
                      DshSettingsZh.noModels,
                      style: TextStyle(fontSize: DshTypography.sizeCaption),
                    ),
                  if (rows.length > shown.length)
                    DshButton(
                      onPressed: () => setState(() {
                        modelsExpanded = true;
                        modelLimit += 100;
                      }),
                      child: Text(
                        DshSettingsZh.moreModels(
                          count: rows.length - shown.length,
                        ),
                      ),
                    ),
                  DshButton(
                    icon: DshIcons.plus.data,
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
                    child: const Text(
                      DshSettingsZh.addManualModel,
                      style: TextStyle(fontSize: DshTypography.sizeCaption),
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    DshSettingsZh.visibilityHint,
                    style: TextStyle(
                      fontSize: DshTypography.sizeCaption,
                      color: colors.muted,
                    ),
                  ),
                  if (invalidFields.isNotEmpty)
                    Text(
                      DshSettingsZh.invalidCapacity,
                      style: TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        color: DshTokens.of(context).error.foreground,
                      ),
                    ),
                  if (modelDirty)
                    Wrap(
                      spacing: 8,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        const Text(
                          DshSettingsZh.dirtyModels,
                          style: TextStyle(fontSize: DshTypography.sizeCaption),
                        ),
                        DshButton(
                          onPressed: busy
                              ? null
                              : () => run(
                                  () => selectProvider(provider!, force: true),
                                ),
                          child: const Text(DshZh.cancel),
                        ),
                        DshButton(
                          primary: true,
                          onPressed: busy || invalidFields.isNotEmpty
                              ? null
                              : saveModels,
                          child: const Text(DshSettingsZh.saveModels),
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
                                  fontSize: DshTypography.sizeAuxiliary,
                                  fontWeight: FontWeight.w600,
                                ),
                              ),
                              const SizedBox(width: 8),
                              Text(
                                '${provider!['provider']}',
                                style: TextStyle(
                                  fontSize: DshTypography.sizeCaption,
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
                const Text(DshSettingsZh.provider),
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
                DshSettingsZh.addModelProvider,
                busy || addableProviders.isEmpty
                    ? null
                    : () => openCreation(false),
              );
              final second = addProviderAction(
                DshSettingsZh.addProvider,
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

  Future<void> login(Json account) => showDialog<void>(
    context: context,
    builder: (_) => AccountLoginDialog(
      api: api,
      provider: '${account['id']}',
      onComplete: refreshAccountState,
    ),
  );

  Widget accountStatus(Json account) {
    final colors = DshColors(context);
    final tokens = DshTokens.of(context);
    final attention = accountNeedsLogin(account);
    final signedIn = account['signedIn'] == true;
    final tone = attention
        ? tokens.warning
        : signedIn
        ? tokens.success
        : null;
    final label = attention
        ? DshSettingsZh.loginRequired
        : signedIn
        ? DshSettingsZh.connected
        : DshSettingsZh.disconnected;
    return Container(
      key: ValueKey('account-status-${account['id']}'),
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
      decoration: BoxDecoration(
        color: tone?.background,
        border: Border.all(color: tone?.border ?? colors.border),
        borderRadius: BorderRadius.circular(4),
      ),
      child: Text(
        '$label${account['scope'] == 'subagent' ? DshSettingsZh.subagentSuffix : ''}',
        style: TextStyle(
          fontSize: DshTypography.sizeCaption,
          color: tone?.foreground ?? colors.muted,
        ),
      ),
    );
  }

  Widget accountTile(Json account) {
    final colors = DshColors(context);
    final id = '${account['id']}';
    final attention = accountNeedsLogin(account);
    final signedIn = account['signedIn'] == true;
    final label = signedIn || attention
        ? maskedAccountLabel(activeAccount(account)?['label'])
        : '';
    final unavailable = account['installed'] == false;
    // The one action a row needs stays visible without expanding it.
    final inline = unavailable || (signedIn && !attention)
        ? null
        : DshButton(
            key: ValueKey('account-inline-login-$id'),
            outline: true,
            height: 30,
            fontSize: DshTypography.sizeAuxiliary,
            onPressed: busy ? null : () => login(account),
            child: Text(
              attention ? DshSettingsZh.reloginShort : DshSettingsZh.login,
            ),
          );
    final tile = Container(
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: colors.border)),
      ),
      child: Theme(
        data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
        child: ExpansionTile(
          key: PageStorageKey('account-$id'),
          initiallyExpanded: expandedAccounts.contains(id),
          tilePadding: EdgeInsets.zero,
          childrenPadding: const EdgeInsets.only(left: 28, bottom: 16),
          expandedCrossAxisAlignment: CrossAxisAlignment.start,
          minTileHeight: 60,
          controlAffinity: ListTileControlAffinity.leading,
          leading: AnimatedRotation(
            turns: expandedAccounts.contains(id) ? .25 : 0,
            duration: DshMotion.duration(context, DshMotion.quick),
            curve: DshMotion.curve,
            child: DshGlyph(DshIcons.chevronRight.data, size: 16),
          ),
          onExpansionChanged: (open) => setState(() {
            if (open) {
              expandedAccounts.add(id);
            } else {
              expandedAccounts.remove(id);
            }
          }),
          textColor: colors.text,
          collapsedTextColor: colors.text,
          title: Row(
            children: [
              Flexible(
                child: Text(
                  '${account['name'] ?? account['id']}',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: DshTypography.sizeBody,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              accountStatus(account),
            ],
          ),
          subtitle: label.isEmpty
              ? null
              : Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
                ),
          trailing: inline,
          children: [
            if (account['error'] != null)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Text(
                  '${account['error']}',
                  style: TextStyle(
                    color: DshTokens.of(context).error.foreground,
                    fontSize: DshTypography.sizeCaption,
                  ),
                ),
              ),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                if (signedIn)
                  DshButton(
                    outline: true,
                    height: 32,
                    fontSize: DshTypography.sizeAuxiliary,
                    onPressed: busy || unavailable
                        ? null
                        : () => run(() async {
                            await api.request(
                              '/provider-auth/connect',
                              body: {'provider': account['id']},
                              mutation: true,
                              scope: scope,
                            );
                            await refreshAccountState();
                          }),
                    child: Text(
                      account['scope'] == 'subagent'
                          ? DshSettingsZh.refresh
                          : DshSettingsZh.reconnect,
                    ),
                  ),
                if (signedIn && account['scope'] != 'subagent') ...[
                  DshButton(
                    outline: true,
                    height: 32,
                    fontSize: DshTypography.sizeAuxiliary,
                    onPressed: busy || unavailable
                        ? null
                        : () => login(account),
                    child: const Text(DshSettingsZh.loginAnotherAccount),
                  ),
                  DshButton(
                    outline: true,
                    destructive: true,
                    height: 32,
                    fontSize: DshTypography.sizeAuxiliary,
                    onPressed: busy ? null : () => removeAccount(account),
                    child: const Text(DshSettingsZh.signOut),
                  ),
                ],
                if (!signedIn && !unavailable)
                  DshButton(
                    outline: true,
                    height: 32,
                    fontSize: DshTypography.sizeAuxiliary,
                    onPressed: busy ? null : () => login(account),
                    child: const Text(DshSettingsZh.login),
                  ),
                if (unavailable &&
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
                    child: const Text(DshSettingsZh.installClient),
                  ),
              ],
            ),
            if (account['scope'] != 'subagent')
              for (final saved in objects(account['accounts']))
                ListTile(
                  contentPadding: EdgeInsets.zero,
                  dense: true,
                  title: Text(
                    maskedAccountLabel(
                      saved['label'] ??
                          saved['accountId'] ??
                          saved['accountScope'],
                    ),
                    style: const TextStyle(
                      fontSize: DshTypography.sizeAuxiliary,
                    ),
                  ),
                  subtitle: saved['needsLogin'] == true
                      ? const Text(
                          DshSettingsZh.loginRequired,
                          style: TextStyle(fontSize: DshTypography.sizeCaption),
                        )
                      : null,
                  trailing: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      DshButton(
                        height: 30,
                        fontSize: DshTypography.sizeCaption,
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
                                    'accountScope': saved['accountScope'],
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
                          saved['active'] == true
                              ? DshSettingsZh.currentAccount
                              : DshSettingsZh.switchAccount,
                        ),
                      ),
                      DshIcon(
                        DshIcons.trash2.data,
                        label: DshSettingsZh.removeAccount,
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
    );
    return id == widget.initialAccountProvider
        ? KeyedSubtree(key: focusedAccount, child: tile)
        : tile;
  }

  Widget accountSection(String title, List<Json> rows) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Padding(
        padding: const EdgeInsets.only(top: 16, bottom: 2),
        child: Text(
          '$title · ${rows.length}',
          style: TextStyle(
            fontSize: DshTypography.sizeCaption,
            color: DshColors(context).muted,
          ),
        ),
      ),
      for (final account in rows) accountTile(account),
    ],
  );

  Widget accountsView() {
    final colors = DshColors(context);
    final linked = linkedAccountProviders(accounts);
    final linkedIds = {for (final row in linked) row['id']};
    final available = [
      for (final row in accounts)
        if (!linkedIds.contains(row['id'])) row,
    ];
    final focus = widget.initialAccountProvider;
    if (!focusedAccountShown &&
        focus != null &&
        accounts.any((row) => row['id'] == focus)) {
      focusedAccountShown = true;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        final target = focusedAccount.currentContext;
        if (mounted && target != null) {
          unawaited(
            // Scrolls only when the provider is below the fold.
            Scrollable.ensureVisible(
              target,
              alignmentPolicy: ScrollPositionAlignmentPolicy.keepVisibleAtEnd,
              duration: DshMotion.duration(context, DshMotion.quick),
            ),
          );
        }
      });
    }
    return ListView(
      children: [
        Container(
          padding: const EdgeInsets.all(14),
          decoration: BoxDecoration(
            border: Border.all(color: colors.border),
            borderRadius: BorderRadius.circular(12),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  const Text(
                    DshSettingsZh.accountLogin,
                    style: TextStyle(fontSize: DshTypography.sizeBody),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      DshSettingsZh.connectedCount(
                        linked.length,
                        accounts.length,
                      ),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        color: colors.muted,
                      ),
                    ),
                  ),
                  const SizedBox(width: 12),
                  DshButton(
                    outline: true,
                    height: 32,
                    fontSize: DshTypography.sizeCaption,
                    onPressed: busy ? null : () => run(refreshAccounts),
                    child: const Text(DshSettingsZh.refreshStatus),
                  ),
                ],
              ),
              const SizedBox(height: 8),
              Text(
                DshSettingsZh.subscriptionLoginHint,
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  height: 1.6,
                  color: colors.muted,
                ),
              ),
              if (linked.isNotEmpty)
                accountSection(DshSettingsZh.linkedAccounts, linked),
              if (available.isNotEmpty)
                accountSection(DshSettingsZh.availableAccounts, available),
            ],
          ),
        ),
      ],
    );
  }

  Future<void> removeAccount(Json provider, [Json? account]) async {
    if (busy ||
        accountLogoutOpen ||
        staleConnection ||
        !mounted ||
        provider['scope'] == 'subagent') {
      return;
    }
    final providerId = provider['id'] as String? ?? '';
    final selectedScope = account == null
        ? provider['accountScope'] as String?
        : account['accountScope'] as String? ?? '';
    final displayedAccount =
        account ??
        objects(provider['accounts'])
            .where((saved) => saved['accountScope'] == selectedScope)
            .firstOrNull;
    final label =
        '${displayedAccount?['label'] ?? displayedAccount?['accountId'] ?? provider['name'] ?? providerId}';
    ProviderLogoutResult? result;
    accountLogoutOpen = true;
    try {
      result = await showDialog<ProviderLogoutResult>(
        context: context,
        barrierDismissible: false,
        builder: (_) => AccountLogoutDialog(
          api: api,
          provider: providerId,
          accountScope: selectedScope,
          label: label,
          parentScope: scope,
          isCurrentConnection: () => mounted && !staleConnection,
        ),
      );
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      accountLogoutOpen = false;
    }
    if (!mounted || staleConnection || result == null) return;
    final completed = result;
    await run(() async {
      setState(() {
        notice =
            '${DshSettingsZh.accountRemoved}'
            '${completed.warning == null ? '' : ' ${completed.warning}'}';
      });
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

class AccountLogoutDialog extends StatefulWidget {
  const AccountLogoutDialog({
    super.key,
    required this.api,
    required this.provider,
    required this.label,
    required this.parentScope,
    required this.isCurrentConnection,
    this.accountScope,
  });

  final DshClient api;
  final String provider, label;
  final String? accountScope;
  final RequestScope parentScope;
  final bool Function() isCurrentConnection;

  @override
  State<AccountLogoutDialog> createState() => _AccountLogoutDialogState();
}

class _AccountLogoutDialogState extends State<AccountLogoutDialog> {
  late final auth = ProviderAuthApi(widget.api);
  final requests = RequestScope();
  late final void Function() unregister;
  late String? selectedScope = widget.accountScope;
  ProviderLogoutImpact? impact;
  String? failure;
  bool working = false;

  @override
  void initState() {
    super.initState();
    unregister = widget.parentScope.register(requests.cancel);
    readImpact();
  }

  @override
  void dispose() {
    unregister();
    requests.cancel();
    super.dispose();
  }

  void checkConnection() {
    if (requests.cancelled || !widget.isCurrentConnection()) {
      throw DshException(
        'connection-changed',
        DshSettingsZh.accountConnectionChanged,
      );
    }
  }

  Future<void> readImpact() async {
    if (working) return;
    setState(() {
      working = true;
      impact = null;
      failure = null;
    });
    try {
      checkConnection();
      final value = await auth.logoutImpact(
        widget.provider,
        accountScope: selectedScope,
        scope: requests,
      );
      checkConnection();
      if (!mounted) return;
      setState(() {
        selectedScope = value.accountScope;
        impact = value;
      });
    } catch (error) {
      if (mounted) setState(() => failure = '$error');
    } finally {
      if (mounted) setState(() => working = false);
    }
  }

  Future<void> signOut() async {
    final confirmed = impact;
    if (working || confirmed == null) return;
    setState(() {
      working = true;
      failure = null;
    });
    try {
      checkConnection();
      final result = await auth.logout(confirmed, scope: requests);
      if (mounted) Navigator.of(context).pop(result);
    } catch (error) {
      if (mounted) {
        setState(() {
          impact = null;
          failure = DshSettingsZh.logoutRecheckRequired(detail: error);
        });
      }
    } finally {
      if (mounted) setState(() => working = false);
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !working,
    child: AlertDialog(
      title: const Text(DshSettingsZh.signOutTitle),
      content: SingleChildScrollView(
        child: SizedBox(
          width: 520,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(widget.label),
              const SizedBox(height: 12),
              Text(
                impact != null
                    ? DshSettingsZh.logoutImpact(count: impact!.taskCount)
                    : working
                    ? DshSettingsZh.checkingAccount
                    : DshSettingsZh.unknownAccountImpact,
              ),
              const SizedBox(height: 12),
              const Text(DshSettingsZh.signOutHint),
              if (failure != null) ...[
                const SizedBox(height: 12),
                Text(DshSettingsZh.unknownLogoutImpact(detail: failure)),
              ],
            ],
          ),
        ),
      ),
      actions: [
        DshButton(
          outline: true,
          onPressed: working ? null : () => Navigator.of(context).pop(),
          child: const Text(DshZh.cancel),
        ),
        if (impact == null)
          DshButton(
            outline: true,
            onPressed: working ? null : readImpact,
            child: const Text(DshSettingsZh.retryAccountImpact),
          ),
        DshButton(
          destructive: true,
          onPressed: working || impact == null ? null : signOut,
          child: const Text(DshSettingsZh.confirmSignOut),
        ),
      ],
    ),
  );
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
      if (mounted) {
        setState(
          () => refreshError = DshSettingsZh.accountRefreshFailed(detail: e),
        );
      }
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
    title: const Text(
      DshSettingsZh.connectSubscription,
      style: TextStyle(fontSize: DshTypography.sizeSectionTitle),
    ),
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
                  ? DshSettingsZh.officialClientLoginHint
                  : DshSettingsZh.browserLoginHint,
              style: const TextStyle(
                fontSize: DshTypography.sizeBody,
                height: 1.6,
              ),
            ),
            if (login.attempt!['userCode'] != null)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 20),
                child: SelectableText(
                  '${login.attempt!['userCode']}',
                  style: const TextStyle(
                    fontSize: DshTypography.sizeHeadline,
                    letterSpacing: 3,
                  ),
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
                            throw StateError(DshSettingsZh.browserUnavailable);
                          }
                        } catch (e) {
                          if (mounted) setState(() => refreshError = '$e');
                        }
                      },
                child: const Text(DshSettingsZh.openAuthorization),
              ),
            if (login.attempt!['userCode'] != null)
              DshButton(
                onPressed: () => Clipboard.setData(
                  ClipboardData(text: '${login.attempt!['userCode']}'),
                ),
                child: const Text(DshSettingsZh.copyVerificationCode),
              ),
            if (!login.expired && !login.complete)
              const Padding(
                padding: EdgeInsets.only(top: 12),
                child: Text(
                  DshSettingsZh.awaitingAuthorization,
                  style: TextStyle(fontSize: DshTypography.sizeCaption),
                ),
              ),
          ],
          if (login.notice != null)
            Text(
              login.notice!,
              style: const TextStyle(fontSize: DshTypography.sizeCaption),
            ),
          if (login.error != null || refreshError != null)
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: DshErrorView(error: refreshError ?? login.error!),
            ),
        ],
      ),
    ),
    actions: [
      if (login.error != null && !login.expired && login.attempt != null)
        DshButton(
          onPressed: login.polling ? null : login.poll,
          child: const Text(DshSettingsZh.checkAgain),
        ),
      if ((login.expired || login.error != null) && !login.complete)
        DshButton(
          onPressed: login.starting || login.polling ? null : login.start,
          child: const Text(DshSettingsZh.loginAgain),
        ),
      DshButton(
        onPressed: () => Navigator.pop(context),
        child: Text(login.complete ? DshSettingsZh.close : DshZh.cancel),
      ),
    ],
  );
}
