import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';

int? modelCapacity(String text) {
  final match = RegExp(
    r'^(\d+(?:\.\d+)?)\s*([km])?$',
    caseSensitive: false,
  ).firstMatch(text.trim());
  if (match == null) return null;
  final n = double.tryParse(match[1]!);
  final v =
      (n ?? 0) *
      (match[2]?.toLowerCase() == 'm'
          ? 1000000
          : match[2]?.toLowerCase() == 'k'
          ? 1000
          : 1);
  return v.isFinite && v > 0 && v <= 9007199254740991 && v == v.roundToDouble()
      ? v.toInt()
      : null;
}

List<String> modelProtocols(Json? namespace) {
  final schema = object(namespace?['schema']),
      refs = object(object(namespace?['schema'])['refs']);
  Json node(Object? ref) => ref is Map ? object(ref) : object(refs['$ref']);
  var current = node(schema['uid']);
  current = node(object(current['dict'])['providers']);
  current = node(current['inner']);
  current = node(object(current['dict'])['api']);
  return [
    for (final ref in current['list'] as List? ?? [])
      if (node(ref)['value'] is String) node(ref)['value'] as String,
  ];
}

class InlineModelRow extends StatelessWidget {
  const InlineModelRow({
    super.key,
    required this.model,
    required this.manual,
    required this.expanded,
    required this.enabled,
    required this.onChange,
    required this.onInvalid,
    required this.onRemove,
    this.rawValues = const {},
    this.onDraft,
  });
  final Json model;
  final bool manual, expanded, enabled;
  final void Function(String, Object?) onChange;
  final void Function(String, bool) onInvalid;
  final VoidCallback onRemove;
  final Map<String, String> rawValues;
  final void Function(String, String)? onDraft;
  Widget field(String name, String title) => SizedBox(
    width: 170,
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(title, style: const TextStyle(fontSize: 12)),
        const SizedBox(height: 4),
        TextFormField(
          key: ValueKey('model-field-$name'),
          initialValue: rawValues[name] ?? '${model[name] ?? ''}',
          enabled: enabled,
          style: const TextStyle(fontSize: 13),
          decoration: const InputDecoration(
            isDense: true,
            contentPadding: EdgeInsets.symmetric(horizontal: 10, vertical: 9),
            border: OutlineInputBorder(),
          ),
          onChanged: (text) {
            if (name == 'contextWindow' || name == 'maxTokens') {
              onDraft?.call(name, text);
              final value = modelCapacity(text);
              final valid = text.trim().isEmpty || value != null;
              onInvalid(name, !valid);
              if (valid) onChange(name, value);
            } else {
              onChange(name, text.trim());
            }
          },
        ),
      ],
    ),
  );
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.fromLTRB(7, 5, 16, 5),
      decoration: manual
          ? BoxDecoration(
              border: Border.all(color: colors.border),
              borderRadius: BorderRadius.circular(8),
            )
          : null,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      manual ? '新手动模型' : '${model['name'] ?? model['id']}',
                      style: const TextStyle(fontSize: 13, height: 18 / 13),
                    ),
                    if (!manual)
                      Text(
                        '${model['id']}',
                        style: TextStyle(
                          fontSize: 11,
                          height: 16 / 11,
                          color: colors.muted,
                        ),
                      ),
                  ],
                ),
              ),
              Semantics(
                label: '显示模型 ${model['id']}',
                checked: model['enabled'] != false,
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    SizedBox(
                      width: 26,
                      height: 26,
                      child: Checkbox(
                        activeColor: colors.blue,
                        checkColor: Colors.white,
                        value: model['enabled'] != false,
                        onChanged: enabled
                            ? (v) => onChange('enabled', v)
                            : null,
                        visualDensity: VisualDensity.compact,
                        materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      ),
                    ),
                    Text(
                      model['enabled'] == false ? '隐藏' : '显示',
                      style: const TextStyle(fontSize: 12),
                    ),
                  ],
                ),
              ),
              if (!manual)
                DshIcon(
                  LucideIcons.trash2,
                  label: '删除模型 ${model['id']}',
                  size: 28,
                  onPressed: enabled ? onRemove : null,
                ),
            ],
          ),
          if (manual)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [field('id', '模型 ID'), field('name', '显示名称')],
              ),
            ),
          if (expanded)
            Padding(
              padding: const EdgeInsets.only(top: 11),
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
                decoration: BoxDecoration(
                  color: colors.layer,
                  borderRadius: BorderRadius.circular(8),
                ),
                child: ExpansionTile(
                  minTileHeight: 28,
                  visualDensity: VisualDensity.compact,
                  shape: const Border(),
                  collapsedShape: const Border(),
                  tilePadding: EdgeInsets.zero,
                  childrenPadding: const EdgeInsets.only(bottom: 8),
                  dense: true,
                  title: const Text('容量', style: TextStyle(fontSize: 12)),
                  trailing: const SizedBox.shrink(),
                  children: [
                    Align(
                      alignment: Alignment.centerLeft,
                      child: Wrap(
                        spacing: 8,
                        runSpacing: 8,
                        children: [
                          if (!manual) field('name', '显示名称'),
                          field('contextWindow', '上下文长度'),
                          field('maxTokens', '最大输出 Token'),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          if (manual)
            DshButton(
              height: 28,
              onPressed: enabled ? onRemove : null,
              child: const Text('移除草稿', style: TextStyle(fontSize: 12)),
            ),
        ],
      ),
    );
  }
}

class CustomProviderCard extends StatefulWidget {
  const CustomProviderCard({
    super.key,
    required this.api,
    required this.namespace,
    required this.taken,
    required this.onSaved,
    required this.onCancel,
    this.onDirtyChanged,
    this.isCurrent,
  });
  final DshClient api;
  final Json namespace;
  final Set<String> taken;
  final Future<void> Function() onSaved;
  final VoidCallback onCancel;
  final ValueChanged<bool>? onDirtyChanged;
  final bool Function()? isCurrent;
  @override
  State<CustomProviderCard> createState() => _CustomProviderCardState();
}

class _CustomProviderCardState extends State<CustomProviderCard> {
  final route = TextEditingController(),
      name = TextEditingController(),
      base = TextEditingController(),
      keyInput = TextEditingController();
  final models = <Json>[];
  List<Json> presets = [];
  final invalid = <String>{};
  String protocol = '', preset = '';
  String? error;
  bool keyless = false, busy = false, committed = false;
  int nextModel = 0;
  @override
  void initState() {
    super.initState();
    protocol = modelProtocols(widget.namespace).firstOrNull ?? '';
    loadPresets();
  }

  Future<void> loadPresets() async {
    final value = jsonDecode(
      await rootBundle.loadString('assets/provider-presets.json'),
    );
    if (mounted) setState(() => presets = objects(value));
  }

  @override
  void dispose() {
    for (final input in [route, name, base, keyInput]) {
      input.dispose();
    }
    super.dispose();
  }

  void changed() {
    widget.onDirtyChanged?.call(true);
  }

  Future<void> save() async {
    if (busy || widget.isCurrent?.call() == false) return;
    final id = route.text
        .trim()
        .toLowerCase()
        .replaceAll(RegExp(r'[^a-z0-9]+'), '-')
        .replaceAll(RegExp(r'^-+|-+$'), '');
    final uri = Uri.tryParse(base.text.trim());
    final ids = models.map((m) => '${m['id'] ?? ''}'.trim()).toList();
    if (!RegExp(r'^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$').hasMatch(id) ||
        (!committed && widget.taken.contains(id))) {
      setState(() => error = '请使用不重复、以字母开头的提供方 ID。');
      return;
    }
    if (uri == null ||
        !['http', 'https'].contains(uri.scheme) ||
        uri.host.isEmpty ||
        protocol.isEmpty ||
        ids.isEmpty ||
        ids.any((v) => v.isEmpty) ||
        ids.toSet().length != ids.length ||
        invalid.isNotEmpty) {
      setState(() => error = '请填写有效 URL、协议和不重复的模型 ID；容量支持正整数或 K/M。');
      return;
    }
    final secret = keyInput.text.trim(),
        ref =
            '${id.toUpperCase().replaceAll(RegExp(r'[^A-Z0-9]+'), '_')}_API_KEY';
    setState(() {
      busy = true;
      error = null;
    });
    try {
      if (!committed) {
        await widget.api.call('settings.mutate', {
          'ns': 'llm-pi-ai',
          'expectedRevision': widget.namespace['revision'],
          'ops': [
            {
              'op': 'set',
              'path': ['providers', id],
              'value': {
                if (name.text.trim().isNotEmpty)
                  'displayName': name.text.trim(),
                'api': protocol,
                'baseURL': base.text.trim(),
                if (keyless) 'keyless': true else 'apiKeyEnv': ref,
                'models': [
                  for (final model in models)
                    {
                      for (final entry in model.entries)
                        if (!entry.key.startsWith('_') &&
                            entry.value != null &&
                            entry.value != '')
                          entry.key: entry.value,
                    },
                ],
              },
            },
          ],
        }, true);
        if (!mounted || widget.isCurrent?.call() == false) return;
        committed = true;
      }
      if (!mounted || widget.isCurrent?.call() == false) return;
      if (!keyless && secret.isNotEmpty) {
        await widget.api.call('credentials.set', {
          'ref': ref,
          'value': secret,
        }, true);
      }
      if (!mounted || widget.isCurrent?.call() == false) return;
      keyInput.clear();
      widget.onDirtyChanged?.call(false);
      await widget.onSaved();
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Widget field(
    String label,
    TextEditingController input, {
    bool secret = false,
  }) => Padding(
    padding: const EdgeInsets.only(bottom: 12),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: const TextStyle(fontSize: 12)),
        const SizedBox(height: 6),
        DshField(
          key: ValueKey('provider-field-$label'),
          controller: input,
          secret: secret,
          enabled: !busy && (secret || !committed),
          onChanged: (_) => changed(),
        ),
      ],
    ),
  );
  @override
  Widget build(BuildContext context) {
    final protocols = modelProtocols(widget.namespace);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text(
          '添加自定义提供方',
          style: TextStyle(fontSize: 14, fontWeight: FontWeight.w500),
        ),
        const SizedBox(height: 14),
        DshSelect<String>(
          value: preset,
          options: {
            '': '自定义提供方',
            for (final p in presets.where(
              (p) =>
                  !widget.taken.contains(p['id']) &&
                  protocols.contains(p['api']),
            ))
              '${p['id']}': '${p['name']}',
          },
          onChanged: busy || committed
              ? null
              : (value) {
                  final p = presets.where((p) => p['id'] == value).firstOrNull;
                  setState(() {
                    preset = value;
                    if (p != null) {
                      route.text = '${p['id']}';
                      name.text = '${p['name']}';
                      base.text = '${p['baseURL']}';
                      protocol = '${p['api']}';
                      keyless = p['keyless'] == true;
                      keyInput.clear();
                      models
                        ..clear()
                        ..addAll(
                          objects(p['models']).map(
                            (m) => {...m, '_draftId': 'new-${++nextModel}'},
                          ),
                        );
                    }
                  });
                  changed();
                },
        ),
        CheckboxListTile(
          contentPadding: EdgeInsets.zero,
          controlAffinity: ListTileControlAffinity.leading,
          title: const Text('无需 API 密钥', style: TextStyle(fontSize: 13)),
          value: keyless,
          onChanged: busy || committed
              ? null
              : (v) {
                  setState(() {
                    keyless = v == true;
                    if (keyless) keyInput.clear();
                  });
                  changed();
                },
        ),
        field('提供方 ID', route),
        field('显示名称', name),
        field('Base URL', base),
        const Text('接口协议', style: TextStyle(fontSize: 12)),
        const SizedBox(height: 6),
        DshSelect<String>(
          value: protocol,
          options: {for (final p in protocols) p: p},
          onChanged: busy || committed
              ? null
              : (p) {
                  setState(() => protocol = p);
                  changed();
                },
        ),
        const SizedBox(height: 12),
        if (!keyless) field('API 密钥', keyInput, secret: true),
        for (final model in models)
          InlineModelRow(
            key: ValueKey(model['_draftId']),
            model: model,
            manual: true,
            expanded: true,
            enabled: !busy && !committed,
            onChange: (field, value) {
              setState(() => model[field] = value);
              changed();
            },
            onInvalid: (field, bad) {
              setState(
                () => bad
                    ? invalid.add('${model['_draftId']}/$field')
                    : invalid.remove('${model['_draftId']}/$field'),
              );
            },
            onRemove: () {
              setState(() {
                invalid.removeWhere(
                  (k) => k.startsWith('${model['_draftId']}/'),
                );
                models.remove(model);
              });
              changed();
            },
          ),
        DshButton(
          icon: LucideIcons.plus,
          onPressed: busy || committed
              ? null
              : () {
                  setState(
                    () => models.add({
                      '_draftId': 'new-${++nextModel}',
                      'id': '',
                      'enabled': true,
                    }),
                  );
                  changed();
                },
          child: const Text('添加模型'),
        ),
        if (error != null)
          Text(error!, style: const TextStyle(color: Colors.red, fontSize: 12)),
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            DshButton(
              onPressed: busy ? null : widget.onCancel,
              child: const Text('取消'),
            ),
            DshButton(
              primary: true,
              onPressed: busy ? null : save,
              child: Text(
                busy
                    ? '保存中…'
                    : committed
                    ? '重试保存密钥'
                    : '添加',
              ),
            ),
          ],
        ),
      ],
    );
  }
}
