import '../../design/error.dart';
import '../../l10n/zh.dart';
import '../../l10n/settings_form_zh.dart';

import 'dart:convert';
import 'dart:math';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:dsh_client/dsh_client.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../design/shortcuts.dart';
import '../../src/controller.dart';
import '../../src/desktop_updates.dart';
import 'models_page.dart';
import 'resource_page.dart';
import 'schedule_panel.dart';
import 'update_panel.dart';

import 'package:dsh_desktop/design/typography.dart';

final settingsPages = <({String id, String title, IconData icon})>[
  (id: 'general', title: DshSettingsZh.general, icon: DshIcons.settings.data),
  (id: 'models', title: DshSettingsZh.models, icon: DshIcons.database.data),
  (id: 'plugins', title: DshSettingsZh.plugins, icon: DshIcons.plugins.data),
  (
    id: 'environment',
    title: DshSettingsZh.runtimeEnvironment,
    icon: DshIcons.folderCog.data,
  ),
  (id: 'memory', title: DshSettingsZh.memoryContext, icon: DshIcons.brain.data),
  (
    id: 'presets',
    title: DshSettingsZh.agentPresets,
    icon: DshIcons.workflow.data,
  ),
  (
    id: 'collaboration',
    title: DshSettingsZh.collaboration,
    icon: DshIcons.users.data,
  ),
  (
    id: 'security',
    title: DshSettingsZh.security,
    icon: DshIcons.shieldCheck.data,
  ),
  (id: 'skills', title: DshSettingsZh.skillsMcp, icon: DshIcons.briefcase.data),
  (
    id: 'discovery',
    title: DshSettingsZh.toolDiscovery,
    icon: DshIcons.wrench.data,
  ),
  (
    id: 'archive',
    title: DshSettingsZh.archiveManagement,
    icon: DshIcons.archive.data,
  ),
  (id: 'schedule', title: DshSettingsZh.reminders, icon: DshIcons.clock.data),
  (
    id: 'menu',
    title: DshSettingsZh.menuSettings,
    icon: DshIcons.listFilter.data,
  ),
  (id: 'trash', title: DshSettingsZh.trash, icon: DshIcons.trash2.data),
  (id: 'updates', title: '更新', icon: DshIcons.download.data),
];

class SettingsShell extends StatefulWidget {
  const SettingsShell({
    super.key,
    required this.controller,
    this.initialPage = 'general',
    this.initialModelTab = 'api',
    this.onOpenPlugin,
    this.updates,
  });
  final DesktopController controller;
  final String initialPage;
  final String initialModelTab;
  final ValueChanged<Json>? onOpenPlugin;
  final DesktopUpdateController? updates;
  @override
  State<SettingsShell> createState() => _SettingsShellState();
}

class _SettingsShellState extends State<SettingsShell> {
  DesktopController get c => widget.controller;
  late String page = settingsPages.any((p) => p.id == widget.initialPage)
      ? widget.initialPage
      : 'general';
  final namespaces = <String, Json>{}, edits = <String, List<Json>>{};
  final scope = RequestScope();
  DshClient? boundApi;
  bool staleConnection = false;
  void connectionChanged() {
    if (!mounted || identical(boundApi, c.client)) return;
    scope.cancel();
    setState(() {
      staleConnection = true;
      loading = false;
      error = DshSettingsZh.settingsConnectionChanged;
    });
  }

  final keyboardFocus = FocusNode();
  bool loading = true, saving = false;
  bool modelDirty = false;
  double? bodyFontDraft;
  String pageQuery = '';
  String? error;
  String? saved;
  bool get formDirty =>
      edits.values.any((e) => e.isNotEmpty) ||
      (bodyFontDraft != null && bodyFontDraft != c.bodyFontSize);
  bool get dirty => formDirty || modelDirty;
  Future<void> selectPage(String next) async {
    if (next == page) return;
    if (modelDirty &&
        !await confirmAction(
          context,
          DshSettingsZh.unsavedModels,
          DshSettingsZh.discardModelConnectionHint,
          action: DshSettingsZh.discardAndSwitch,
        )) {
      return;
    }
    if (mounted) {
      setState(() {
        page = next;
        modelDirty = false;
      });
    }
  }

  @override
  void initState() {
    super.initState();
    boundApi = c.client;
    c.addListener(connectionChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) keyboardFocus.requestFocus();
    });
    load();
  }

  @override
  void didUpdateWidget(SettingsShell oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != c) {
      oldWidget.controller.removeListener(connectionChanged);
      c.addListener(connectionChanged);
      connectionChanged();
    }
  }

  @override
  void dispose() {
    c.removeListener(connectionChanged);
    scope.cancel();
    keyboardFocus.dispose();
    super.dispose();
  }

  Future<void> load() async {
    if (staleConnection) return;
    final api = boundApi;
    try {
      if (api == null) throw StateError(DshSettingsZh.connectFirst);
      final value = await api.rpc('settings.describe', scope: scope);
      if (!mounted || !identical(api, c.client)) return;
      setState(() {
        for (final row in objects(value['namespaces'])) {
          namespaces[row['ns'] as String] = row;
        }
        loading = false;
        error = null;
      });
    } catch (e) {
      if (mounted && !staleConnection) {
        setState(() {
          error = '$e';
          loading = false;
        });
      }
    }
  }

  void edit(String ns, List<String> path, Object? value) {
    setState(() {
      saved = null;
      final list = edits.putIfAbsent(ns, () => []);
      list.removeWhere((op) => jsonEncode(op['path']) == jsonEncode(path));
      list.add({'op': 'set', 'path': path, 'value': value});
    });
  }

  Future<void> save() async {
    if (saving || staleConnection) return;
    final owner = boundApi;
    setState(() {
      saving = true;
      error = null;
      saved = null;
    });
    try {
      for (final ns in edits.keys.toList()) {
        final ops = List<Json>.of(edits[ns]!);
        if (ops.isEmpty) continue;
        if (owner == null || !identical(owner, c.client)) {
          throw StateError(DshSettingsZh.settingsConnectionChanged);
        }
        final updated = await owner.call('settings.mutate', {
          'ns': ns,
          'expectedRevision': namespaces[ns]?['revision'],
          'ops': ops,
        }, true);
        if (!mounted || !identical(owner, c.client)) return;
        setState(() {
          namespaces[ns] = updated;
          edits[ns]?.removeWhere(ops.contains);
          if (edits[ns]?.isEmpty == true) edits.remove(ns);
        });
      }
      if (bodyFontDraft != null && bodyFontDraft != c.bodyFontSize) {
        await c.setBodyFontSize(bodyFontDraft!);
      }
      if (!mounted || staleConnection || !identical(owner, c.client)) return;
      await c.loadCatalogs();
      if (mounted && !staleConnection && identical(owner, c.client)) {
        setState(() => saved = formDirty ? null : DshSettingsZh.settingsSaved);
      }
    } catch (e) {
      if (mounted && !staleConnection) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => saving = false);
    }
  }

  Future<void> close() async {
    if (dirty &&
        !await confirmAction(
          context,
          DshSettingsZh.discardTitle,
          DshSettingsZh.discardHint,
          action: DshSettingsZh.discardChanges,
        )) {
      return;
    }
    if (mounted) Navigator.pop(context);
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final small = MediaQuery.sizeOf(context).width <= 768;
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: Focus(
        focusNode: keyboardFocus,
        autofocus: true,
        child: PopScope(
          canPop: !dirty,
          onPopInvokedWithResult: (didPop, _) {
            if (!didPop) close();
          },
          child: BackdropFilter(
            filter: ui.ImageFilter.blur(sigmaX: 2, sigmaY: 2),
            child: ColoredBox(
              color: Colors.black.withValues(alpha: colors.dark ? .5 : .24),
              child: Dialog(
                insetPadding: small
                    ? EdgeInsets.zero
                    : const EdgeInsets.all(24),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(small ? 0 : 16),
                ),
                child: SizedBox(
                  key: const ValueKey('settings-panel'),
                  width: 1040,
                  height: small
                      ? MediaQuery.sizeOf(context).height
                      : min(800, MediaQuery.sizeOf(context).height - 48),
                  child: Column(
                    children: [
                      Padding(
                        padding: const EdgeInsets.fromLTRB(24, 20, 14, 6),
                        child: Row(
                          children: [
                            const Text(
                              DshSettingsZh.settings,
                              style: TextStyle(
                                fontSize: DshTypography.sizeComposer,
                                height: 24 / 16,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                            const Spacer(),
                            DshButton(
                              outline: true,
                              height: 28,
                              pill: true,
                              fontSize: DshTypography.sizeCaption,
                              padding: const EdgeInsets.symmetric(
                                horizontal: 10,
                              ),
                              onPressed: () => c.run(() async {
                                await c.client!.call(
                                  'settings.openDocument',
                                  {},
                                  true,
                                );
                              }),
                              child: const Text(
                                DshSettingsZh.openConfig,
                                style: TextStyle(
                                  fontSize: DshTypography.sizeCaption,
                                ),
                              ),
                            ),
                            const SizedBox(width: 12),
                            DshIcon(
                              DshIcons.close.data,
                              label: DshSettingsZh.close,
                              onPressed: saving ? null : close,
                            ),
                          ],
                        ),
                      ),
                      Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 24,
                          vertical: 8,
                        ),
                        child: DshField(
                          key: const Key('settings-page-search'),
                          hint: DshSettingsZh.searchSettings,
                          prefix: DshIcons.search.data,
                          onChanged: (value) =>
                              setState(() => pageQuery = value),
                        ),
                      ),
                      if (small)
                        SizedBox(
                          height: max(
                            42,
                            MediaQuery.textScalerOf(context).scale(42),
                          ),
                          child: ListView(
                            scrollDirection: Axis.horizontal,
                            children: [
                              for (final item in settingsPages.where(
                                (item) => '${item.title} ${item.id}'
                                    .toLowerCase()
                                    .contains(pageQuery.toLowerCase()),
                              ))
                                DshButton(
                                  onPressed: () => selectPage(item.id),
                                  child: Text(item.title),
                                ),
                            ],
                          ),
                        ),
                      Expanded(
                        child: Row(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            if (!small)
                              SizedBox(
                                width: 188,
                                child: ListView(
                                  padding: const EdgeInsets.fromLTRB(
                                    12,
                                    10,
                                    12,
                                    12,
                                  ),
                                  children: [
                                    for (final item in settingsPages.where(
                                      (item) => '${item.title} ${item.id}'
                                          .toLowerCase()
                                          .contains(pageQuery.toLowerCase()),
                                    ))
                                      Padding(
                                        padding: const EdgeInsets.only(
                                          bottom: 4,
                                        ),
                                        child: Material(
                                          color: page == item.id
                                              ? colors.hover
                                              : Colors.transparent,
                                          borderRadius: BorderRadius.circular(
                                            8,
                                          ),
                                          child: InkWell(
                                            borderRadius: BorderRadius.circular(
                                              8,
                                            ),
                                            onTap: () => selectPage(item.id),
                                            child: Padding(
                                              padding:
                                                  const EdgeInsets.symmetric(
                                                    horizontal: 12,
                                                    vertical: 9,
                                                  ),
                                              child: Row(
                                                children: [
                                                  DshGlyph(
                                                    item.icon,
                                                    size:
                                                        [
                                                          'environment',
                                                          'trash',
                                                          'memory',
                                                          'collaboration',
                                                          'security',
                                                          'skills',
                                                          'archive',
                                                        ].contains(item.id)
                                                        ? 18
                                                        : 16,
                                                    asset: const {
                                                      'environment': 'assets/icons/web-nav-runtime-paths.svg',
                                                      'memory': 'assets/icons/web-nav-memory.svg',
                                                      'collaboration': 'assets/icons/web-nav-subagent.svg',
                                                      'security': 'assets/icons/web-nav-security.svg',
                                                      'skills': 'assets/icons/web-nav-capabilities.svg',
                                                      'archive': 'assets/icons/web-nav-archived-sessions.svg',
                                                      'trash': 'assets/icons/web-nav-trash.svg',
                                                      'discovery': 'assets/icons/web-IconSettingsOutline16.svg',
                                                      'menu': 'assets/icons/web-IconSettingsOutline16.svg',
                                                    }[item.id],
                                                  ),
                                                  const SizedBox(width: 11),
                                                  Expanded(
                                                    child: Text(
                                                      item.title,
                                                      style: const TextStyle(
                                                        fontSize: DshTypography
                                                            .sizeBody,
                                                        height: 22 / 14,
                                                      ),
                                                    ),
                                                  ),
                                                ],
                                              ),
                                            ),
                                          ),
                                        ),
                                      ),
                                  ],
                                ),
                              ),
                            Expanded(
                              child: Padding(
                                padding: const EdgeInsets.fromLTRB(
                                  24,
                                  0,
                                  32,
                                  24,
                                ),
                                child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    if (error != null && page != 'updates')
                                      DshErrorView(
                                        error: error!,
                                        onRetry: saving || staleConnection
                                            ? null
                                            : load,
                                      ),
                                    if (saved != null)
                                      Text(
                                        saved!,
                                        style: TextStyle(
                                          color: DshTokens.of(context)
                                              .success
                                              .foreground,
                                          fontSize: DshTypography.sizeCaption,
                                        ),
                                      ),
                                    Expanded(
                                      child: loading && page != 'updates'
                                          ? const Center(
                                              child: CircularProgressIndicator(
                                                strokeWidth: 2,
                                              ),
                                            )
                                          : body(),
                                    ),
                                    if (formDirty)
                                      Container(
                                        padding: const EdgeInsets.only(top: 12),
                                        decoration: BoxDecoration(
                                          border: Border(
                                            top: BorderSide(
                                              color: colors.border,
                                            ),
                                          ),
                                        ),
                                        child: Row(
                                          children: [
                                            const Text(
                                              DshSettingsZh.unsaved,
                                              style: TextStyle(
                                                fontSize:
                                                    DshTypography.sizeCaption,
                                              ),
                                            ),
                                            const Spacer(),
                                            DshButton(
                                              onPressed: saving
                                                  ? null
                                                  : () {
                                                      setState(edits.clear);
                                                      load();
                                                    },
                                              child: const Text(DshZh.cancel),
                                            ),
                                            const SizedBox(width: 8),
                                            DshButton(
                                              primary: true,
                                              onPressed:
                                                  saving || staleConnection
                                                  ? null
                                                  : save,
                                              child: Text(
                                                saving
                                                    ? DshZh.saving
                                                    : DshZh.save,
                                              ),
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
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget body() {
    if (page == 'updates') {
      return SettingsUpdatePanel(controller: c, updates: widget.updates);
    }
    if (page == 'schedule') return SchedulePanel(controller: c);
    if (page == 'models') {
      return ModelsPage(
        controller: c,
        onSettingsChanged: load,
        initialTab: widget.initialModelTab,
        onDirtyChanged: (value) {
          if (mounted && modelDirty != value) {
            setState(() => modelDirty = value);
          }
        },
      );
    }
    if ([
      'plugins',
      'presets',
      'skills',
      'discovery',
      'archive',
      'memory',
    ].contains(page)) {
      return SettingsResourcePage(
        key: ValueKey(page),
        controller: c,
        page: page,
        onOpenPlugin: widget.onOpenPlugin,
        footer: page == 'memory' ? forms(['memory', 'system-prompt']) : null,
      );
    }
    if (page == 'general') {
      return general();
    }
    final selection = switch (page) {
      'environment' => [
        'storage-paths',
        'workspace-scratch-paths',
        'uu-remote',
        'computer-use',
      ],
      'collaboration' => ['agent-teams', 'subagent'],
      'security' => ['security', 'permission'],
      'menu' => ['mini-menu', 'dsh-better-sidebar'],
      'trash' => ['workspace-scratch'],
      _ => <String>[],
    };
    return SingleChildScrollView(
      key: const ValueKey('settings-form-scroll'),
      padding: const EdgeInsets.only(right: 20, bottom: 20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            settingsPages.firstWhere((p) => p.id == page).title,
            style: const TextStyle(
              fontSize: DshTypography.sizeComposer,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: 20),
          forms(selection),
        ],
      ),
    );
  }

  Object? setting(String ns, String field, Object? fallback) {
    final pending = edits[ns]
        ?.where((op) => jsonEncode(op['path']) == jsonEncode([field]))
        .lastOrNull;
    return pending == null
        ? object(namespaces[ns]?['value'])[field] ?? fallback
        : pending['value'];
  }

  Widget generalSelect(
    String ns,
    String field,
    String fallback,
    Map<String, String> options,
  ) => DshSelect<String>(
    options: options,
    value: '${setting(ns, field, fallback)}',
    onChanged: saving || namespaces[ns] == null
        ? null
        : (value) => edit(ns, [field], value),
  );

  Widget generalRow(String title, String hint, Widget control) => Container(
    constraints: const BoxConstraints(minHeight: 69),
    decoration: BoxDecoration(
      border: Border(bottom: BorderSide(color: DshColors(context).border)),
    ),
    padding: const EdgeInsets.symmetric(vertical: 16),
    child: LayoutBuilder(
      builder: (context, constraints) {
        final label = Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              title,
              style: const TextStyle(
                fontSize: DshTypography.sizeBody,
                height: 22 / 14,
              ),
            ),
            if (hint.isNotEmpty) ...[
              const SizedBox(height: 5),
              Text(
                hint,
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  height: 1.5,
                  color: DshColors(context).muted,
                ),
              ),
            ],
          ],
        );
        if (constraints.maxWidth < 480 ||
            MediaQuery.textScalerOf(context).scale(1) > 1.5) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [label, const SizedBox(height: 12), control],
          );
        }
        return Row(
          children: [
            Expanded(child: label),
            const SizedBox(width: 20),
            control,
          ],
        );
      },
    ),
  );

  Widget general() {
    final colors = DshColors(context);
    return ListView(
      children: [
        generalRow(
          DshSettingsZh.conversationFontSize,
          DshSettingsZh.conversationFontHint,
          DshSelect<double>(
            key: const Key('conversation-font-size'),
            value: bodyFontDraft ?? c.bodyFontSize,
            options: {
              14: '14',
              15: DshSettingsZh.defaultFontSize,
              16: '16',
              17: '17',
              18: '18',
            },
            onChanged: saving
                ? null
                : (value) => setState(() {
                    bodyFontDraft = value;
                    saved = null;
                  }),
          ),
        ),
        generalRow(
          DshSettingsZh.agentPresets,
          DshSettingsZh.defaultPresetHint,
          generalSelect('agent-presets', 'default', c.preset, {
            for (final p in c.presets)
              if (p['broken'] == null) '${p['id']}': '${p['name'] ?? p['id']}',
          }),
        ),
        generalRow(
          DshSettingsZh.permission,
          DshSettingsZh.defaultPermissionHint,
          generalSelect(
            'permission',
            'defaultPreset',
            'workspace-write',
            permissionDefaults(),
          ),
        ),
        generalRow(
          DshSettingsZh.language,
          '',
          generalSelect('locale', 'preference', 'zh', const {
            'zh': DshSettingsZh.chinese,
            'en': 'English',
          }),
        ),
        Container(
          padding: const EdgeInsets.symmetric(vertical: 16),
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: colors.border)),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                DshSettingsZh.appearance,
                style: TextStyle(
                  fontSize: DshTypography.sizeBody,
                  height: 22 / 14,
                ),
              ),
              const SizedBox(height: 8),
              Row(
                children: [
                  for (final dark in [false, true]) ...[
                    if (dark) const SizedBox(width: 8),
                    Expanded(
                      child: Semantics(
                        checked: c.preferences.dark == dark,
                        label: dark ? DshSettingsZh.dark : DshSettingsZh.light,
                        child: Material(
                          color: c.preferences.dark == dark
                              ? colors.layer
                              : colors.dark
                              ? DshTokens.of(context).layer
                              : colors.base,
                          shape: RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(18),
                            side: BorderSide(
                              color: c.preferences.dark == dark
                                  ? colors.muted.withValues(alpha: .6)
                                  : colors.border,
                            ),
                          ),
                          child: InkWell(
                            borderRadius: BorderRadius.circular(18),
                            onTap: () {
                              setState(() => c.preferences.dark = dark);
                              c.emit();
                              c.run(c.preferences.save);
                            },
                            child: SizedBox(
                              height: 82,
                              child: Column(
                                mainAxisAlignment: MainAxisAlignment.center,
                                children: [
                                  DshGlyph(
                                    dark
                                        ? DshIcons.moon.data
                                        : DshIcons.sun.data,
                                    size: 18,
                                  ),
                                  const SizedBox(height: 4),
                                  Text(
                                    dark
                                        ? DshSettingsZh.dark
                                        : DshSettingsZh.light,
                                    style: const TextStyle(
                                      fontSize: DshTypography.sizeBody,
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ],
                ],
              ),
            ],
          ),
        ),
        generalRow(
          DshSettingsZh.busyEnter,
          DshSettingsZh.busyEnterHint,
          generalSelect('ui-conversation', 'busyEnter', 'queue', const {
            'queue': DshZh.queueMessage,
            'steer': DshZh.steerExecution,
          }),
        ),
        generalRow(
          DshSettingsZh.replyHints,
          DshSettingsZh.replyHintsDescription,
          generalSelect('ui-conversation', 'hintDisplay', 'both', const {
            'both': DshSettingsZh.iconAndText,
            'icons': DshSettingsZh.iconsOnly,
            'text': DshSettingsZh.textOnly,
          }),
        ),
        generalRow(
          DshSettingsZh.composerTips,
          DshSettingsZh.composerTipsHint,
          generalSelect('ui-conversation', 'composerTips', 'on', const {
            'on': DshSettingsZh.on,
            'off': DshSettingsZh.close,
          }),
        ),
        generalRow(
          DshSettingsZh.directoryPicker,
          DshSettingsZh.directoryPickerHint,
          const Text(
            DshSettingsZh.systemDirectoryPicker,
            style: TextStyle(fontSize: DshTypography.sizeBody),
          ),
        ),
        generalRow(
          DshSettingsZh.shortcuts,
          DshSettingsZh.shortcutsHint,
          DshButton(
            outline: true,
            pill: true,
            onPressed: () => showDialog<void>(
              context: context,
              builder: (_) => ShortcutEditor(controller: c),
            ),
            child: const Text(DshSettingsZh.shortcutBindings),
          ),
        ),
      ],
    );
  }

  Map<String, String> permissionDefaults() {
    final schema = object(namespaces['permission']?['schema']),
        refs = object(object(namespaces['permission']?['schema'])['refs']);
    final root = object(refs['${schema['uid']}']);
    final field = object(refs['${object(root['dict'])['defaultPreset']}']);
    final options = <String, String>{
      for (final ref in field['list'] as List? ?? [])
        if (object(refs['$ref'])['value'] is String)
          object(refs['$ref'])['value'] as String: permissionName(
            object(refs['$ref'])['value'] as String,
          ),
    };
    final current =
        '${setting('permission', 'defaultPreset', 'workspace-write')}';
    return options.isEmpty ? {current: permissionName(current)} : options;
  }

  Widget forms(List<String> names) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      for (final ns in names)
        if (namespaces[ns] != null) ...[
          if (namespaceTitles[ns] case (:final title, :final hint)) ...[
            const SizedBox(height: 12),
            Text(
              title,
              style: const TextStyle(
                fontSize: DshTypography.sizeConversation,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: 4),
            Text(
              hint,
              style: TextStyle(
                fontSize: DshTypography.sizeCaption,
                color: DshColors(context).muted,
              ),
            ),
          ],
          NamespaceForm(
            key: ValueKey('$ns:${namespaces[ns]!['revision']}'),
            namespace: namespaces[ns]!,
            pending: edits[ns] ?? [],
            onChange: (path, value) => edit(ns, path, value),
          ),
        ],
    ],
  );
}

/// Headings for namespaces that share a settings page with others.
const namespaceTitles = <String, ({String title, String hint})>{
  'computer-use': (title: 'Computer Use', hint: DshSettingsZh.computerUseHint),
};

Widget _row(
  BuildContext context,
  String title,
  String hint,
  Widget control, {
  required Key key,
  bool expandControl = false,
}) => Padding(
  key: key,
  padding: const EdgeInsets.symmetric(vertical: 14),
  child: LayoutBuilder(
    builder: (context, constraints) {
      final label = Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(title, style: const TextStyle(fontSize: DshTypography.sizeBody)),
          if (hint.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 5),
              child: Text(
                hint,
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: DshTokens.of(context).muted,
                ),
              ),
            ),
        ],
      );
      final stacked =
          constraints.maxWidth < 600 ||
          MediaQuery.textScalerOf(context).scale(1) > 1.5;
      if (stacked) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            label,
            const SizedBox(height: 10),
            if (expandControl)
              SizedBox(width: constraints.maxWidth, child: control)
            else
              control,
          ],
        );
      }
      return Row(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          Expanded(flex: 2, child: label),
          const SizedBox(width: 24),
          if (expandControl) Expanded(flex: 3, child: control) else control,
        ],
      );
    },
  ),
);

/// Keys may be qualified as `namespace.field` where a bare field name is
/// ambiguous across namespaces.
const fieldLabels = <String, String>{
  'computer-use.enabled': DshSettingsZh.enableComputerUse,
  'computer-use.nativeProtocol': DshSettingsZh.nativeComputerProtocol,
  'computer-use.nativeTarget': DshSettingsZh.nativeControlTarget,
  'computer-use.adapter': DshSettingsZh.executionAdapter,
  'computer-use.command': DshSettingsZh.externalControlCommand,
  'computer-use.browserExecutable': DshSettingsZh.browserExecutable,
  'computer-use.browserHeadless': DshSettingsZh.headlessBrowser,
  'computer-use.maxBrowserSessions': DshSettingsZh.maxBrowserSessions,
  'computer-use.timeoutSeconds': DshSettingsZh.operationTimeout,
  'default': DshSettingsZh.agentPresets,
  'defaultPreset': DshSettingsZh.permission,
  'busyEnter': DshSettingsZh.busyEnter,
  'composerTips': DshSettingsZh.composerTips,
  'hintDisplay': DshSettingsZh.replyHints,
  'enabled': DshSettingsZh.enable,
  'width': DshSettingsZh.workbenchWidth,
  'rememberWidth': DshSettingsZh.rememberWidth,
  'fullscreenOnOpen': DshSettingsZh.openFullscreen,
  'showFiles': DshSettingsZh.showFiles,
  'showGit': DshSettingsZh.showGit,
  'showBrowser': DshSettingsZh.showWeb,
  'showTerminal': DshSettingsZh.showTerminal,
  'dataDirectory': DshSettingsZh.dataDirectory,
  'cacheDirectory': DshSettingsZh.cacheDirectory,
  'environmentDirectory': DshSettingsZh.runtimeDirectory,
  'testDirectory': DshSettingsZh.testDirectory,
  'maxMembers': DshSettingsZh.maxTeamMembers,
  'showButton': DshSettingsZh.showTeamButton,
  'defaultMode': DshSettingsZh.defaultCollaboration,
  'maxParallel': DshSettingsZh.parallelAgents,
  'maxDepth': DshSettingsZh.nestingDepth,
  'maxTurns': DshSettingsZh.turnLimit,
  'timeoutSeconds': DshSettingsZh.timeout,
  'defaultProvider': DshSettingsZh.defaultProvider,
  'defaultModel': DshSettingsZh.defaultModel,
  'defaultReasoningEffort': DshSettingsZh.reasoningEffort,
  'defaultMaxTokens': DshSettingsZh.outputTokenLimit,
  'toolCallMode': DshSettingsZh.toolPresentation,
  'serviceTier': DshSettingsZh.serviceTier,
  'apiRetryCount': DshSettingsZh.requestRetries,
  'approvalTimeoutSeconds': DshSettingsZh.approvalTimeout,
  'unattendedPolicy': DshSettingsZh.unattendedPolicy,
  'riskToolPolicy': DshSettingsZh.highRiskPolicy,
  'outsideWritePolicy': DshSettingsZh.outsideWorkspacePolicy,
  'sensitiveReadPolicy': DshSettingsZh.sensitiveFilePolicy,
  'credentialShellPolicy': DshSettingsZh.credentialCommandPolicy,
  'userProfileEnabled': DshSettingsZh.userProfile,
  'memoryBudget': DshSettingsZh.memoryBudget,
  'profileBudget': DshSettingsZh.profileBudget,
  'provider': DshSettingsZh.provider,
  'contextEngine': DshSettingsZh.contextEngine,
  'autoCompact': DshSettingsZh.autoCompaction,
  'compactThreshold': DshSettingsZh.compactionThreshold,
  'compactTarget': DshSettingsZh.compactionTarget,
  'protectRecentMessages': DshSettingsZh.recentMessages,
  'persona': DshSettingsZh.roleDescription,
  'personaSuffix': DshSettingsZh.roleSupplement,
  'includeHarnessIdentity': DshSettingsZh.includeHarnessIdentity,
  'includeRuntimeContext': DshSettingsZh.includeEnvironment,
  'autoClean': DshSettingsZh.autoCleanup,
  'reduceContext': DshSettingsZh.compactContext,
  'keepDays': DshSettingsZh.retainedDays,
  'failedDays': DshSettingsZh.failedArtifactDays,
  'recoveryDays': DshSettingsZh.recoveryDays,
  'softLimitGib': DshSettingsZh.storageSoftLimit,
  'location': DshSettingsZh.location,
  'cliPath': DshSettingsZh.clientPath,
  'account': DshSettingsZh.account,
  'deviceId': DshSettingsZh.deviceId,
  'trajectory': DshSettingsZh.trace,
  'artifacts': DshSettingsZh.artifacts,
  'code-graph': DshSettingsZh.codeGraph,
  'context': DshSettingsZh.context,
};
const optionLabels = <String, String>{
  'computer-use.local': DshSettingsZh.localDesktop,
  'computer-use.browser': DshSettingsZh.isolatedBrowser,
  'computer-use.native-browser': DshSettingsZh.builtInBrowser,
  'computer-use.native-desktop': DshSettingsZh.nativeDesktop,
  'computer-use.uu-desktop': DshSettingsZh.remoteDesktop,
  'computer-use.command': DshSettingsZh.externalCommand,
  'queue': DshZh.queueMessage,
  'steer': DshSettingsZh.steerExecution,
  'on': DshSettingsZh.on,
  'off': DshSettingsZh.close,
  'both': DshSettingsZh.iconAndText,
  'text': DshSettingsZh.textOnly,
  'icons': DshSettingsZh.iconsOnly,
  'read-only': DshSettingsZh.readOnly,
  'workspace-write': DshSettingsZh.workspaceWrite,
  'full-access': DshSettingsZh.fullAccess,
  'ask': DshSettingsZh.ask,
  'deny': DshSettingsZh.deny,
  'allow': DshSettingsZh.allow,
  'auto': DshSettingsZh.auto,
  'standard': DshSettingsZh.standardMode,
  'code': DshSettingsZh.codeMode,
  'blank': DshSettingsZh.blankMode,
  'native': DshSettingsZh.native,
  'inherit': DshSettingsZh.inherit,
  'zh-CN': DshSettingsZh.chinese,
  'en-US': 'English',
};

class NamespaceForm extends StatefulWidget {
  const NamespaceForm({
    super.key,
    required this.namespace,
    required this.pending,
    required this.onChange,
  });
  final Json namespace;
  final List<Json> pending;
  final void Function(List<String>, Object?) onChange;
  @override
  State<NamespaceForm> createState() => _NamespaceFormState();
}

class _NamespaceFormState extends State<NamespaceForm> {
  final inputs = <String, TextEditingController>{};
  @override
  void dispose() {
    for (final input in inputs.values) {
      input.dispose();
    }
    super.dispose();
  }

  Object? current(List<String> path, Object? fallback) {
    for (final op in widget.pending.reversed) {
      if (jsonEncode(op['path']) == jsonEncode(path)) return op['value'];
    }
    return fallback;
  }

  Object? topLevel(String key, [Object? fallback]) =>
      current([key], object(widget.namespace['value'])[key] ?? fallback);

  bool visibleField(String key) {
    if (widget.namespace['ns'] == 'mini-menu' && key == 'tasks') return false;
    if (widget.namespace['ns'] != 'computer-use') return true;
    final adapter = '${topLevel('adapter', 'auto')}'.trim();
    final auto = adapter.isEmpty || adapter == 'auto';
    if (key == 'command') return auto || adapter == 'command';
    if (const {
      'browserExecutable',
      'browserHeadless',
      'maxBrowserSessions',
    }.contains(key)) {
      return adapter == 'native-browser' ||
          (auto && '${topLevel('command', '')}'.trim().isEmpty);
    }
    return true;
  }

  @override
  Widget build(BuildContext context) {
    final schema = object(widget.namespace['schema']),
        refs = object(schema['refs']);
    final node = object(refs['${schema['uid']}']);
    final fields = object(node['dict']);
    final value = object(widget.namespace['value']);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final key in {...fields.keys, ...value.keys})
          if (visibleField(key) &&
              ![
                'sessionLayouts',
                'providers',
                'models',
                'modelPreferences',
                'profiles',
              ].contains(key))
            field(
              context,
              [key],
              current([key], value[key]),
              object(refs['${fields[key]}']),
              refs,
            ),
        if (widget.namespace['applies'] == 'restart')
          Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: Text(
              DshSettingsZh.settingsRestartRequired,
              style: TextStyle(
                fontSize: DshTypography.sizeCaption,
                color: DshTokens.of(context).muted,
              ),
            ),
          ),
      ],
    );
  }

  Widget field(
    BuildContext context,
    List<String> path,
    Object? value,
    Json schema,
    Json refs,
  ) {
    final ns = widget.namespace['ns'], key = path.last;
    final rowKey = ValueKey('settings-field-$ns-${path.join('/')}');
    final nativeTargetInactive =
        ns == 'computer-use' &&
        key == 'nativeTarget' &&
        topLevel('nativeProtocol', false) != true;
    final hint = nativeTargetInactive
        ? DshSettingsFormZh.nativeTargetInactive
        : ns == 'computer-use' &&
              key == 'adapter' &&
              {'', 'auto'}.contains('${topLevel('adapter', 'auto')}'.trim())
        ? DshSettingsFormZh.autoAdapterHint
        : '';
    final label =
        fieldLabels['$ns.$key'] ??
        fieldLabels[key] ??
        '${object(schema['meta'])['description'] ?? key}';
    var options = <Object?>[];
    if (schema['type'] == 'union') {
      options = (schema['list'] as List? ?? [])
          .map((id) => object(refs['$id']))
          .where((n) => n['type'] == 'const')
          .map((n) => n['value'])
          .toList();
    }
    if (schema['type'] == 'boolean' || value is bool) {
      return _row(
        context,
        label,
        hint,
        DshSwitch(
          value: value == true,
          onChanged: (v) => widget.onChange(path, v),
        ),
        key: rowKey,
      );
    }
    if (options.isNotEmpty) {
      return _row(
        context,
        label,
        hint,
        DshSelect<Object>(
          value: options.contains(value) ? value : null,
          options: {
            for (final item in options)
              ?item:
                  optionLabels['$ns.$item'] ?? optionLabels['$item'] ?? '$item',
          },
          onChanged: nativeTargetInactive
              ? null
              : (v) => widget.onChange(path, v),
        ),
        key: rowKey,
      );
    }
    if (value is Map) {
      final dict = object(schema['dict']);
      return ExpansionTile(
        key: rowKey,
        title: Text(
          label,
          style: const TextStyle(fontSize: DshTypography.sizeBody),
        ),
        tilePadding: EdgeInsets.zero,
        children: [
          for (final child in value.keys)
            field(
              context,
              [...path, '$child'],
              current([...path, '$child'], value[child]),
              object(refs['${dict[child]}']),
              refs,
            ),
        ],
      );
    }
    if (value is List) {
      return Padding(
        key: rowKey,
        padding: const EdgeInsets.symmetric(vertical: 10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              label,
              style: const TextStyle(fontSize: DshTypography.sizeBody),
            ),
            const SizedBox(height: 8),
            for (final (index, item) in value.indexed)
              if (item is String)
                Padding(
                  padding: const EdgeInsets.only(bottom: 6),
                  child: DshField(
                    controller: inputs.putIfAbsent(
                      '${path.join('/')}/$index',
                      () => TextEditingController(text: displayPath(item)),
                    ),
                    onChanged: (text) {
                      final list = List<dynamic>.of(value);
                      list[index] = text;
                      widget.onChange(path, list);
                    },
                  ),
                ),
            DshButton(
              icon: DshIcons.plus.data,
              onPressed: () => widget.onChange(path, [...value, '']),
              child: const Text(DshSettingsZh.add),
            ),
          ],
        ),
      );
    }
    final input = inputs.putIfAbsent(
      path.join('/'),
      () => TextEditingController(
        text: value == null ? '' : displayPath('$value'),
      ),
    );
    final isNumber = schema['type'] == 'number' || value is num;
    return _row(
      context,
      label,
      hint,
      DshField(
        controller: input,
        hint: object(schema['meta'])['default']?.toString(),
        onChanged: (text) {
          if (isNumber) {
            final number = num.tryParse(text);
            if (number != null) widget.onChange(path, number);
          } else {
            widget.onChange(path, text);
          }
        },
      ),
      key: rowKey,
      expandControl: true,
    );
  }
}
