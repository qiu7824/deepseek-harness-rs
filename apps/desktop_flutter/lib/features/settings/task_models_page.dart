import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';

class TaskModelsPage extends StatefulWidget {
  const TaskModelsPage({super.key, required this.api, this.namespace});
  final DshClient api;
  final Json? namespace;
  @override
  State<TaskModelsPage> createState() => _TaskModelsPageState();
}

class _TaskModelsPageState extends State<TaskModelsPage> {
  static const roles = {
    'diagnose': '排错',
    'optimize': '优化',
    'vision': '看图',
    'image': '生图',
    'search': '搜索',
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
          notice = '已保存';
        });
      }
    } catch (e) {
      if (mounted && generation == current) setState(() => error = '$e');
    } finally {
      if (mounted && generation == current) setState(() => busy = false);
    }
  }

  String hint(String state) => switch (state) {
    'unsupported' => '此连接不支持该图像或托管搜索工具，请选择兼容连接或清空分工。',
    'unavailable' => '该连接已不可用，请重新选择。',
    'compatible' => '连接协议兼容；具体模型能力和账号权限仍需验证。',
    'inherited' => '跟随当前会话连接；分工留空不会增加该连接的工具能力。',
    _ => '当前服务尚未提供兼容性信息，工具可用性未验证。',
  };
  @override
  Widget build(BuildContext context) => ListView(
    children: [
      const Text('为辅助任务指定模型', style: TextStyle(fontSize: 14)),
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
                  decoration: const InputDecoration(labelText: '提供方'),
                  items: [
                    const DropdownMenuItem(value: '', child: Text('跟随当前会话')),
                    if (provider(role.key).isNotEmpty &&
                        !providers.any((p) => p['id'] == provider(role.key)))
                      DropdownMenuItem(
                        value: provider(role.key),
                        child: Text('${provider(role.key)} · 不可用'),
                      ),
                    for (final p in providers)
                      DropdownMenuItem<String>(
                        value: p['id'] as String,
                        enabled:
                            !(['image', 'search'].contains(role.key) &&
                                object(p['nativeTools'])[role.key] ==
                                    'unsupported'),
                        child: Text(
                          '${p['name'] ?? p['id']}${['image', 'search'].contains(role.key) && object(p['nativeTools'])[role.key] == 'unsupported' ? ' · 不支持' : ''}',
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
                            hint: key == 'model' ? '模型 ID' : '推理等级',
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
                        fontSize: 12,
                        color:
                            [
                              'unsupported',
                              'unavailable',
                            ].contains(capability(role.key))
                            ? Colors.red
                            : DshColors(context).muted,
                      ),
                    ),
                  ),
              ],
            ),
          ),
      if (error != null)
        Text(error!, style: const TextStyle(color: Colors.red)),
      if (snapshot == null && error != null)
        DshButton(onPressed: load, child: const Text('重新加载')),
      if (notice != null)
        Text(notice!, style: const TextStyle(color: Colors.green)),
      Align(
        alignment: Alignment.centerRight,
        child: DshButton(
          primary: true,
          onPressed: busy || snapshot == null || invalid ? null : save,
          child: const Text('保存任务模型'),
        ),
      ),
    ],
  );
}
