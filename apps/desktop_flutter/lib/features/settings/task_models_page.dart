import '../../design/error.dart';
import '../../l10n/zh.dart';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';

import 'package:dsh_desktop/design/typography.dart';

class TaskModelsPage extends StatefulWidget {
  const TaskModelsPage({super.key, required this.api, this.namespace});
  final DshClient api;
  final Json? namespace;
  @override
  State<TaskModelsPage> createState() => _TaskModelsPageState();
}

class _TaskModelsPageState extends State<TaskModelsPage> {
  static const roles = {
    'diagnose': DshSettingsZh.debugRole,
    'optimize': DshSettingsZh.optimizeRole,
    'vision': DshSettingsZh.visionRole,
    'image': DshSettingsZh.imageRole,
    'search': DshSettingsZh.searchRole,
  };
  final fields = <String, TextEditingController>{};
  RequestScope scope = RequestScope();
  Json? snapshot;
  String? error, notice;
  bool busy = false;
  int generation = 0;
  @override
  void initState() {
    super.initState();
    load();
  }

  @override
  void didUpdateWidget(TaskModelsPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.api, widget.api)) {
      scope.cancel();
      scope = RequestScope();
      for (final field in fields.values) {
        field.dispose();
      }
      fields.clear();
      snapshot = null;
      busy = false;
      error = null;
      notice = null;
      load();
    }
  }

  @override
  void dispose() {
    generation++;
    scope.cancel();
    for (final field in fields.values) {
      field.dispose();
    }
    super.dispose();
  }

  Future<void> load() async {
    final current = ++generation, api = widget.api;
    try {
      final value = await api.request(
        '/task-models/describe',
        body: {},
        scope: scope,
      );
      if (!mounted || current != generation) return;
      for (final role in roles.keys) {
        final route = object(object(value['routes'])[role]);
        for (final key in ['provider', 'model', 'reasoningEffort']) {
          fields['$role/$key'] = TextEditingController(
            text: route[key] as String? ?? '',
          );
        }
      }
      setState(() {
        snapshot = value;
        error = null;
      });
    } catch (e) {
      if (mounted && current == generation) setState(() => error = '$e');
    }
  }

  List<Json> get providers => objects(snapshot?['providers']);
  Future<void> refreshCapabilities() async {
    if (busy) return;
    final current = generation;
    setState(() => busy = true);
    try {
      final value = await widget.api.request(
        '/task-models/describe',
        body: {},
        scope: scope,
      );
      if (mounted && current == generation) {
        setState(() {
          snapshot = value;
          error = null;
        });
      }
    } catch (e) {
      if (mounted && current == generation) setState(() => error = '$e');
    } finally {
      if (mounted && current == generation) setState(() => busy = false);
    }
  }

  static String stateLabel(dynamic value) => switch (value) {
    'ready' => DshSettingsZh.capabilityVerified,
    'temporary_failure' => DshSettingsZh.temporaryFailure,
    'unsupported' => DshSettingsZh.unsupported,
    'authorization_required' => DshSettingsZh.authorizationRequired,
    'unconfigured' => DshSettingsZh.unconfigured,
    'present' => DshSettingsZh.credentialsConfigured,
    'not_required' => DshSettingsZh.credentialsNotRequired,
    'missing' || 'invalid' || 'expired' => DshSettingsZh.credentialsRequired,
    _ => DshSettingsZh.capabilityUnverified,
  };
  Widget capabilityDetails(String role) {
    final p = providers
        .where((item) => item['id'] == provider(role))
        .firstOrNull;
    final data = object(object(p?['nativeCapabilities'])[role]);
    if (data.isEmpty) return const SizedBox.shrink();
    final rows = objects(data['observations']);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          '${data['registered'] == true ? DshSettingsZh.toolRegistered : DshSettingsZh.toolUnregistered} · ${stateLabel(data['authorization'])} · ${stateLabel(data['state'])}',
          key: ValueKey('native-lifecycle-$role'),
        ),
        const Text(
          DshSettingsZh.capabilityScope,
          style: TextStyle(fontSize: DshTypography.sizeCaption),
        ),
        for (final row in rows)
          Text(
            '${row['model']} / ${row['operation']}${row['driverModel'] == null ? '' : DshSettingsZh.driverModelSuffix(model: row['driverModel'])} · ${stateLabel((row['expiresAt'] is num && (row['expiresAt'] as num) * 1000 <= DateTime.now().millisecondsSinceEpoch) ? 'unverified' : row['state'])}${row['expired'] == true ? DshSettingsZh.expiredRecord : ''}',
            style: const TextStyle(fontSize: DshTypography.sizeCaption),
          ),
      ],
    );
  }

  String provider(String role) => fields['$role/provider']?.text.trim() ?? '';
  String capability(String role) {
    final id = provider(role);
    if (id.isEmpty) return 'inherited';
    final match = providers.where((item) => item['id'] == id).firstOrNull;
    if (match == null) return 'unavailable';
    return object(match['nativeTools'])[role] as String? ?? 'unknown';
  }

  bool get invalid => roles.keys.any(
    (role) =>
        capability(role) == 'unavailable' ||
        (['image', 'search'].contains(role) &&
            capability(role) == 'unsupported'),
  );
  Future<void> save() async {
    if (busy || snapshot == null || invalid) return;
    final current = generation;
    setState(() {
      busy = true;
      error = null;
      notice = null;
    });
    try {
      final routes = <String, dynamic>{...object(snapshot!['routes'])};
      for (final role in roles.keys) {
        final route = <String, dynamic>{...object(routes[role])};
        for (final key in ['provider', 'model', 'reasoningEffort']) {
          final text = fields['$role/$key']!.text.trim();
          if (text.isEmpty) {
            route.remove(key);
          } else {
            route[key] = text;
          }
        }
        if (route.isEmpty) {
          routes.remove(role);
        } else {
          routes[role] = route;
        }
      }
      final value = await widget.api.request(
        '/task-models/save',
        body: {'routes': routes, 'revision': snapshot!['revision']},
        scope: scope,
        mutation: true,
      );
      if (mounted && generation == current) {
        setState(() {
          snapshot = value;
          notice = DshSettingsZh.saved;
        });
      }
    } catch (e) {
      if (mounted && generation == current) setState(() => error = '$e');
    } finally {
      if (mounted && generation == current) setState(() => busy = false);
    }
  }

  String hint(String state) => switch (state) {
    'unsupported' => DshSettingsZh.incompatibleConnection,
    'unavailable' => DshSettingsZh.unavailableConnection,
    'compatible' => DshSettingsZh.compatibleConnection,
    'inherited' => DshSettingsZh.followConnectionHint,
    _ => DshSettingsZh.capabilityUnknown,
  };
  @override
  Widget build(BuildContext context) => ListView(
    children: [
      const Text(
        DshZh.auxiliaryModels,
        style: TextStyle(fontSize: DshTypography.sizeBody),
      ),
      if (snapshot == null && error == null) const LinearProgressIndicator(),
      if (snapshot != null)
        for (final role in roles.entries)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(role.value),
                const SizedBox(height: 8),
                DropdownButtonFormField<String>(
                  key: ValueKey('${role.key}/${provider(role.key)}'),
                  initialValue: provider(role.key),
                  isExpanded: true,
                  decoration: const InputDecoration(
                    labelText: DshSettingsZh.provider,
                  ),
                  items: [
                    const DropdownMenuItem(
                      value: '',
                      child: Text(DshSettingsZh.followSession),
                    ),
                    if (provider(role.key).isNotEmpty &&
                        !providers.any((p) => p['id'] == provider(role.key)))
                      DropdownMenuItem(
                        value: provider(role.key),
                        child: Text(
                          DshSettingsZh.unavailableProvider(
                            provider: provider(role.key),
                          ),
                        ),
                      ),
                    for (final p in providers)
                      DropdownMenuItem<String>(
                        value: p['id'] as String,
                        enabled:
                            !(['image', 'search'].contains(role.key) &&
                                object(p['nativeTools'])[role.key] ==
                                    'unsupported'),
                        child: Text(
                          '${p['name'] ?? p['id']}${['image', 'search'].contains(role.key) && object(p['nativeTools'])[role.key] == 'unsupported' ? DshSettingsZh.unsupportedSuffix : ''}',
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                  ],
                  onChanged: busy
                      ? null
                      : (value) => setState(() {
                          fields['${role.key}/provider']!.text = value ?? '';
                          notice = null;
                        }),
                ),
                const SizedBox(height: 8),
                Row(
                  children: [
                    for (final key in ['model', 'reasoningEffort'])
                      Expanded(
                        child: Padding(
                          padding: const EdgeInsets.only(right: 8),
                          child: DshField(
                            controller: fields['${role.key}/$key']!,
                            hint: key == 'model'
                                ? DshSettingsZh.modelId
                                : DshSettingsZh.reasoningLevel,
                          ),
                        ),
                      ),
                  ],
                ),
                if (['image', 'search'].contains(role.key) ||
                    capability(role.key) == 'unavailable')
                  Padding(
                    padding: const EdgeInsets.only(top: 8),
                    child: Text(
                      hint(capability(role.key)),
                      key: ValueKey('capability-${role.key}'),
                      style: TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        color:
                            [
                              'unsupported',
                              'unavailable',
                            ].contains(capability(role.key))
                            ? DshTokens.of(context).error.foreground
                            : DshColors(context).muted,
                      ),
                    ),
                  ),
                if (['image', 'search'].contains(role.key))
                  capabilityDetails(role.key),
              ],
            ),
          ),
      if (error != null) DshErrorView(error: error!),
      if (snapshot == null && error != null)
        DshButton(onPressed: load, child: const Text(DshSettingsZh.reload)),
      if (notice != null)
        Text(
          notice!,
          style: TextStyle(color: DshTokens.of(context).success.foreground),
        ),
      DshButton(
        onPressed: busy || snapshot == null ? null : refreshCapabilities,
        child: const Text(DshSettingsZh.refreshCapabilities),
      ),
      Align(
        alignment: Alignment.centerRight,
        child: DshButton(
          primary: true,
          onPressed: busy || snapshot == null || invalid ? null : save,
          child: const Text(DshSettingsZh.saveAuxiliaryModels),
        ),
      ),
    ],
  );
}
