import '../../design/error.dart';
import '../../l10n/zh.dart';
import '../../l10n/conversation_zh.dart';
import '../../l10n/plugin_settings_zh.dart';

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:dsh_client/dsh_client.dart';

import '../../design/primitives.dart';
import '../../design/loading.dart';
import '../../design/select.dart';
import '../../src/controller.dart';
import 'learning_panel.dart';
import 'plugin_operations_panel.dart';
import 'plugin_inventory_row.dart';
import 'time_context_panel.dart';
import 'skill_revisions_page.dart';

import 'package:dsh_desktop/design/typography.dart';

class SettingsResourcePage extends StatefulWidget {
  const SettingsResourcePage({
    super.key,
    required this.controller,
    required this.page,
    this.footer,
    this.onOpenPlugin,
    this.workspace = false,
  });
  final DesktopController controller;
  final String page;
  final Widget? footer;
  final ValueChanged<Json>? onOpenPlugin;
  final bool workspace;
  @override
  State<SettingsResourcePage> createState() => _SettingsResourcePageState();
}

class _SettingsResourcePageState extends State<SettingsResourcePage> {
  late final DesktopController boundController;
  DshClient? boundApi;
  HostInfo? boundHost;
  bool get staleConnection =>
      boundApi == null ||
      !identical(widget.controller, boundController) ||
      !identical(widget.controller.client, boundApi) ||
      !identical(widget.controller.host, boundHost);
  DshClient get api {
    if (staleConnection) throw StateError(DshSettingsZh.connectionChanged);
    return boundApi!;
  }

  final scope = RequestScope(), search = TextEditingController();
  Json data = {};
  bool loading = true, busy = false;
  bool pluginManagerOpen = false;
  String? error, notice;
  int loadGeneration = 0;
  @override
  void initState() {
    super.initState();
    boundController = widget.controller;
    boundApi = widget.controller.client;
    boundHost = widget.controller.host;
    widget.controller.addListener(connectionChanged);
    load();
  }

  @override
  void didUpdateWidget(SettingsResourcePage oldWidget) {
    super.didUpdateWidget(oldWidget);
    connectionChanged();
  }

  void connectionChanged() {
    if (staleConnection && mounted) {
      scope.cancel();
      loadGeneration++;
      setState(() {
        loading = false;
        error = DshSettingsZh.connectionChanged;
      });
    }
  }

  @override
  void dispose() {
    boundController.removeListener(connectionChanged);
    scope.cancel();
    search.dispose();
    super.dispose();
  }

  Future<void> load() async {
    if (staleConnection) {
      connectionChanged();
      return;
    }
    final generation = ++loadGeneration;
    if (mounted) setState(() => loading = true);
    try {
      Json result;
      switch (widget.page) {
        case 'archive':
          await widget.controller.refreshSessions();
          result = {'entries': widget.controller.archivedSessions};
        case 'discovery':
          result = await api.request('/__dsh-tool-discovery', scope: scope);
        default:
          result = await api.rpc(switch (widget.page) {
            'plugins' => 'pluginInventory.list',
            'presets' => 'agentPreset.list',
            'skills' => 'capabilities.list',
            _ => 'memory.list',
          }, scope: scope);
      }
      if (mounted && !staleConnection && generation == loadGeneration) {
        setState(() {
          data = result;
          loading = false;
          error = null;
        });
      }
    } catch (e) {
      if (mounted && !staleConnection && generation == loadGeneration) {
        setState(() {
          error = '$e';
          loading = false;
        });
      }
    }
  }

  Future<void> action(Future<void> Function() work) async {
    if (busy || !mounted || staleConnection) return;
    setState(() {
      busy = true;
      error = null;
      notice = null;
    });
    try {
      await work();
      await load();
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final title = {
      'plugins': DshSettingsZh.plugins,
      'presets': DshSettingsZh.agentPresets,
      'skills': DshSettingsZh.skillsMcp,
      'archive': DshSettingsZh.archivedSessions,
      'memory': DshSettingsZh.memoryContext,
      'discovery': DshSettingsZh.toolDiscovery,
    }[widget.page]!;
    final query = search.text.toLowerCase();
    final inventory = objects(data['entries'] ?? data['presets'] ?? data['skills'])
        .where(
          (entry) =>
              widget.page != 'plugins' ||
              !DshPluginSettingsZh.retired(
                '${entry['moduleName'] ?? entry['id'] ?? entry['entryId'] ?? ''}',
              ),
        )
        .toList();
    bool matches(Json entry) =>
        '${entry['name']} ${entry['title']} ${entry['moduleName']} '
                '${entry['entryId']} ${entry['id']} ${entry['description']} '
                '${DshPluginSettingsZh.title('${entry['moduleName'] ?? ''}', '')}'
            .toLowerCase()
            .contains(query);
    final entries = inventory.where(matches).toList();
    if (widget.page == 'plugins' && widget.workspace) {
      return pluginWorkspace(inventory, entries, matches);
    }
    final visibleRows = widget.page == 'plugins' ? inventory : entries;
    Key pluginKey(Json entry) => ValueKey(
      'plugin-${entry['entryId'] ?? entry['id'] ?? entry['moduleName']}',
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                title,
                style: const TextStyle(
                  fontSize: DshTypography.sizeComposer,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
            DshIcon(
              DshIcons.refreshCw.data,
              label: DshSettingsZh.refresh,
              onPressed: loading || busy || staleConnection ? null : load,
            ),
            if (widget.page == 'plugins')
              DshButton(
                outline: true,
                icon: DshIcons.puzzle.data,
                onPressed: busy || pluginManagerOpen || staleConnection
                    ? null
                    : openPluginManager,
                child: const Text(DshSettingsZh.installationMaintenance),
              ),
            if (widget.page == 'skills')
              DshButton(
                outline: true,
                onPressed: loading || busy || staleConnection
                    ? null
                    : () => showDialog<void>(
                        context: context,
                        builder: (dialogContext) => Dialog(
                          child: SizedBox(
                            width: 880,
                            height: 680,
                            child: Padding(
                              padding: const EdgeInsets.all(20),
                              child: SkillRevisionsPage(
                                api: api,
                                controller: widget.controller,
                                onClose: () => Navigator.pop(dialogContext),
                              ),
                            ),
                          ),
                        ),
                      ),
                child: const Text(DshSettingsZh.revisionsValidation),
              ),
            if (widget.page == 'skills')
              DshButton(
                outline: true,
                icon: DshIcons.plus.data,
                onPressed: loading || busy ? null : () => editSkill(),
                child: const Text(DshSettingsZh.addSkill),
              ),
          ],
        ),
        const SizedBox(height: 16),
        if (widget.page == 'archive')
          Text(
            DshSettingsZh.archiveHint,
            style: TextStyle(
              fontSize: DshTypography.sizeAuxiliary,
              height: 1.6,
              color: DshColors(context).muted,
            ),
          )
        else
          DshField(
            controller: search,
            hint: DshSettingsZh.searchItems(title: title),
            prefix: DshIcons.search.data,
            onChanged: (_) => setState(() {}),
          ),
        const SizedBox(height: 12),
        if (error != null) DshErrorView(error: error!),
        if (notice != null)
          Text(
            notice!,
            style: TextStyle(
              color: DshColors(context).blue,
              fontSize: DshTypography.sizeCaption,
            ),
          ),
        if (busy || loading && data.isNotEmpty)
          const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: loading && data.isEmpty
              ? DshListSkeleton(
                  label: DshConversationZh.loadingList(name: title),
                )
              : ListView.builder(
                  padding: const EdgeInsets.only(right: 16, bottom: 16),
                  findChildIndexCallback: widget.page != 'plugins'
                      ? null
                      : (key) {
                          final index = inventory.indexWhere(
                            (entry) => pluginKey(entry) == key,
                          );
                          return index < 0 ? null : index;
                        },
                  itemCount:
                      visibleRows.length +
                      (widget.page == 'memory' ? 1 : 0) +
                      (widget.page == 'skills' ? 1 : 0) +
                      (widget.footer != null ? 1 : 0) +
                      (widget.page == 'discovery' ? 1 : 0),
                  itemBuilder: (context, i) {
                    if (i < visibleRows.length) {
                      final entry = visibleRows[i];
                      if (widget.page != 'plugins') return row(entry);
                      // Visited configuration editors retain their own state;
                      // ordinary rows still use the list's lazy lifecycle.
                      return Offstage(
                        key: pluginKey(entry),
                        offstage: !matches(entry),
                        child: row(entry),
                      );
                    }
                    if (widget.page == 'memory' && i == entries.length) {
                      return LearningPanel(controller: widget.controller);
                    }
                    if (widget.page == 'skills' && i == entries.length) {
                      return serverList();
                    }
                    if (widget.page == 'discovery') return discovery();
                    return widget.footer ?? const SizedBox();
                  },
                ),
        ),
        if (entries.isEmpty &&
            !loading &&
            widget.footer == null &&
            widget.page != 'memory' &&
            widget.page != 'skills' &&
            widget.page != 'discovery')
          Padding(
            padding: const EdgeInsets.all(20),
            child: Text(
              DshSettingsZh.noRecords,
              style: TextStyle(color: DshTokens.of(context).muted),
            ),
          ),
      ],
    );
  }

  Widget pluginWorkspace(
    List<Json> inventory,
    List<Json> entries,
    bool Function(Json) matches,
  ) {
    final colors = DshColors(context);
    Key pluginKey(Json entry) => ValueKey(
      'plugin-${entry['entryId'] ?? entry['id'] ?? entry['moduleName']}',
    );
    final heading = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(DshSettingsZh.plugins, style: DshTypography.headline),
        const SizedBox(height: 6),
        Text(
          DshPluginSettingsZh.pageHint,
          style: DshTypography.auxiliary.copyWith(color: colors.muted),
        ),
      ],
    );
    final actions = Wrap(
      spacing: 8,
      runSpacing: 8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        DshIcon(
          DshIcons.refreshCw.data,
          label: DshSettingsZh.refresh,
          onPressed: loading || busy || staleConnection ? null : load,
        ),
        FilledButton.icon(
          key: const ValueKey('plugin-add'),
          onPressed: busy || pluginManagerOpen || staleConnection
              ? null
              : openPluginManager,
          style: FilledButton.styleFrom(
            backgroundColor: colors.text,
            foregroundColor: colors.base,
            textStyle: DshTypography.body.copyWith(fontWeight: FontWeight.w500),
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(8),
            ),
          ),
          icon: DshGlyph(DshIcons.plus.data, color: colors.base, size: 16),
          label: const Text(DshPluginSettingsZh.addPlugin),
        ),
      ],
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        LayoutBuilder(
          builder: (context, constraints) {
            final stacked =
                constraints.maxWidth < 560 ||
                MediaQuery.textScalerOf(context).scale(26) > 36;
            if (stacked) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [heading, const SizedBox(height: 14), actions],
              );
            }
            return Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(child: heading),
                actions,
              ],
            );
          },
        ),
        const SizedBox(height: 24),
        Align(
          alignment: Alignment.centerLeft,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 320),
            child: DshField(
              controller: search,
              hint: DshPluginSettingsZh.searchPlugins,
              prefix: DshIcons.search.data,
              onChanged: (_) => setState(() {}),
            ),
          ),
        ),
        const SizedBox(height: 24),
        Row(
          children: [
            Expanded(
              child: Text(
                DshPluginSettingsZh.currentHost,
                style: DshTypography.body.copyWith(fontWeight: FontWeight.w500),
              ),
            ),
            Text(
              DshPluginSettingsZh.configuredCount(entries.length),
              style: DshTypography.caption.copyWith(color: colors.muted),
            ),
          ],
        ),
        const SizedBox(height: 8),
        if (error != null) DshErrorView(error: error!),
        if (busy || loading && data.isNotEmpty)
          const LinearProgressIndicator(minHeight: 2),
        Expanded(
          child: loading && data.isEmpty
              ? const DshListSkeleton(label: DshSettingsZh.plugins)
              : ListView.builder(
                  padding: const EdgeInsets.only(bottom: 24),
                  findChildIndexCallback: (key) {
                    final index = inventory.indexWhere(
                      (entry) => pluginKey(entry) == key,
                    );
                    return index < 0 ? null : index;
                  },
                  itemCount: inventory.length + (widget.footer == null ? 0 : 1),
                  itemBuilder: (context, index) {
                    if (index == inventory.length) return widget.footer!;
                    final entry = inventory[index];
                    return Offstage(
                      key: pluginKey(entry),
                      offstage: !matches(entry),
                      child: pluginWorkspaceRow(entry),
                    );
                  },
                ),
        ),
        if (!loading && entries.isEmpty)
          Padding(
            padding: const EdgeInsets.all(20),
            child: Text(
              DshSettingsZh.noRecords,
              style: DshTypography.auxiliary.copyWith(color: colors.muted),
            ),
          ),
      ],
    );
  }

  Widget pluginWorkspaceRow(Json entry) {
    final moduleName = DshPluginSettingsZh.canonical(
      '${entry['moduleName'] ?? entry['id'] ?? entry['entryId'] ?? ''}',
    );
    final canOpen =
        widget.onOpenPlugin != null &&
        entry['enabled'] == true &&
        const {
          'dsh-artifacts',
          'dsh-context-jump',
          'dsh-better-sidebar',
          'dsh-sidebar-workbench-suite',
          'dsh-voice-input',
        }.contains(moduleName);
    Widget inventoryRow({VoidCallback? configure, bool expanded = false}) =>
        PluginInventoryRow(
          key: ValueKey('plugin-row-${entry['entryId'] ?? moduleName}'),
          entry: entry,
          expanded: expanded,
          onConfigure: configure,
          onOpen: canOpen && !staleConnection
              ? () {
                  if (!staleConnection) widget.onOpenPlugin!(entry);
                }
              : null,
          onEnabledChanged:
              busy || staleConnection || entry['entryId'] is! String
              ? null
              : (value) => action(() async {
                  await api.rpc(
                    'pluginInventory.setEnabled',
                    payload: {'entryId': entry['entryId'], 'enabled': value},
                    mutation: true,
                    scope: scope,
                  );
                  if (!staleConnection) await widget.controller.loadPlugins();
                }),
        );
    if (moduleName == 'dsh-time-context' && entry['entryId'] is String) {
      return _TimeContextPluginSection(
        controller: widget.controller,
        entryId: entry['entryId'] as String,
        enabled: entry['enabled'] == true,
        canConfigure: () => mounted && !staleConnection,
        workspace: true,
        entry: inventoryRow(),
        entryBuilder: (configure, expanded) =>
            inventoryRow(configure: configure, expanded: expanded),
      );
    }
    return inventoryRow();
  }

  Widget row(Json row) {
    if (widget.page == 'archive') return archiveRow(row);
    final rawTitle =
        '${row['name'] ?? row['title'] ?? row['moduleName'] ?? row['id'] ?? row['sessionId']}';
    final plugin = widget.page == 'plugins';
    final moduleName =
        '${row['moduleName'] ?? row['id'] ?? row['entryId'] ?? ''}';
    final title = plugin
        ? DshPluginSettingsZh.title(moduleName, rawTitle)
        : rawTitle;
    final description = displayPathText(
      '${row['description'] ?? row['content'] ?? row['cwd'] ?? row['trust'] ?? ''}',
    );
    final entry = Container(
      padding: const EdgeInsets.symmetric(vertical: 12),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: DshColors(context).border)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: const TextStyle(
                    fontSize: DshTypography.sizeBody,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                if (plugin && moduleName.isNotEmpty && moduleName != title)
                  Padding(
                    padding: const EdgeInsets.only(top: 4),
                    child: SelectableText(
                      moduleName,
                      style: TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        color: DshColors(context).muted,
                      ),
                    ),
                  ),
                if (plugin)
                  Padding(
                    padding: const EdgeInsets.only(top: 5),
                    child: Wrap(
                      spacing: 12,
                      runSpacing: 4,
                      children: [
                        Text(
                          DshPluginSettingsZh.configured(
                            row['enabled'] == true,
                          ),
                        ),
                        Text(
                          DshPluginSettingsZh.runtimeStatus(row['fiberPhase']),
                        ),
                      ],
                    ),
                  ),
                if (description.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(top: 5),
                    child: Text(
                      description,
                      maxLines: 3,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        height: 1.5,
                        color: DshColors(context).muted,
                      ),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          ...switch (widget.page) {
            'plugins' => [
              if (widget.onOpenPlugin != null &&
                  row['enabled'] == true &&
                  const {
                    'dsh-artifacts',
                    'dsh-context-jump',
                    'dsh-better-sidebar',
                    'dsh-sidebar-workbench-suite',
                    'dsh-voice-input',
                  }.contains(row['moduleName'] ?? row['entryId'] ?? row['id']))
                DshButton(
                  outline: true,
                  height: 28,
                  padding: const EdgeInsets.symmetric(horizontal: 9),
                  onPressed: staleConnection
                      ? null
                      : () {
                          if (!staleConnection) widget.onOpenPlugin!(row);
                        },
                  child: Text(
                    '${row['id'] ?? row['entryId'] ?? ''}'.contains('voice')
                        ? DshSettingsZh.voiceInput
                        : DshSettingsZh.openPlugin,
                    style: const TextStyle(fontSize: DshTypography.sizeCaption),
                  ),
                ),
              Semantics(
                label: DshPluginSettingsZh.toggle(title),
                child: DshSwitch(
                  value: row['enabled'] == true,
                  onChanged: busy || staleConnection
                      ? null
                      : (v) => action(() async {
                          await api.rpc(
                            'pluginInventory.setEnabled',
                            payload: {'entryId': row['entryId'], 'enabled': v},
                            mutation: true,
                            scope: scope,
                          );
                          if (!staleConnection) {
                            await widget.controller.loadPlugins();
                          }
                        }),
                ),
              ),
            ],
            'skills' => [
              DshSwitch(
                value: row['enabled'] == true,
                onChanged: busy
                    ? null
                    : (v) => action(() async {
                        await api.call('capabilities.skillToggle', {
                          'name': row['name'],
                          'enabled': v,
                          'expectedRevision': data['revision'],
                        }, true);
                      }),
              ),
              if (row['managed'] == true)
                DshIcon(
                  DshIcons.pencil.data,
                  label: DshSettingsZh.editSkill,
                  onPressed: busy ? null : () => editSkill(row),
                ),
              if (row['managed'] == true)
                DshIcon(
                  DshIcons.trash2.data,
                  label: DshSettingsZh.removeSkill,
                  onPressed: busy
                      ? null
                      : () => removeCapability(row, skill: true),
                ),
            ],
            'presets' => [
              DshIcon(
                DshIcons.fileText.data,
                label: DshSettingsZh.viewPreset,
                onPressed: () => viewPreset(row),
              ),
              DshIcon(
                DshIcons.copy.data,
                label: DshSettingsZh.copyPreset,
                onPressed: () => copyPreset(row),
              ),
            ],
            'memory' => [
              DshSwitch(
                value: row['enabled'] == true,
                onChanged: busy
                    ? null
                    : (v) => action(() async {
                        await api.call('memory.upsert', {
                          'entry': {...row, 'enabled': v},
                          'expectedRevision': row['revision'],
                        }, true);
                      }),
              ),
              DshIcon(
                DshIcons.pencil.data,
                label: DshSettingsZh.editMemory,
                onPressed: busy ? null : () => editMemory(row),
              ),
              DshIcon(
                DshIcons.trash2.data,
                label: DshSettingsZh.deleteMemory,
                onPressed: busy
                    ? null
                    : () async {
                        if (await confirmAction(
                          context,
                          DshSettingsZh.deleteMemory,
                          title,
                          action: DshSettingsZh.delete,
                        )) {
                          await action(() async {
                            await api.call('memory.remove', {
                              'id': row['id'],
                              'expectedRevision': row['revision'],
                            }, true);
                          });
                        }
                      },
              ),
            ],
            _ => <Widget>[],
          },
        ],
      ),
    );
    if (widget.page == 'plugins' &&
        const {
          'dsh-time-context',
          '@deepseek-ai/dsh-time-context',
        }.contains(row['moduleName']) &&
        row['entryId'] is String) {
      return _TimeContextPluginSection(
        controller: widget.controller,
        entryId: row['entryId'] as String,
        enabled: row['enabled'] == true,
        canConfigure: () => mounted && !staleConnection,
        entry: entry,
      );
    }
    return entry;
  }

  Future<void> openPluginManager() async {
    if (!mounted || staleConnection || pluginManagerOpen || busy) return;
    final capturedApi = boundApi;
    setState(() => pluginManagerOpen = true);
    try {
      await showDialog<void>(
        context: context,
        builder: (dialogContext) => Dialog(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 720, maxHeight: 700),
            child: Padding(
              padding: const EdgeInsets.all(20),
              child: Column(
                children: [
                  Row(
                    children: [
                      const Expanded(
                        child: Text(
                          DshSettingsZh.pluginMaintenance,
                          style: TextStyle(
                            fontSize: DshTypography.sizeSectionTitle,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                      DshIcon(
                        DshIcons.close.data,
                        label: DshSettingsZh.closePluginManagement,
                        onPressed: () => Navigator.of(dialogContext).pop(),
                      ),
                    ],
                  ),
                  const SizedBox(height: 16),
                  Expanded(
                    child: SingleChildScrollView(
                      child: PluginOperationsPanel(
                        controller: widget.controller,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      );
    } finally {
      if (mounted) setState(() => pluginManagerOpen = false);
    }
    if (mounted && capturedApi == widget.controller.client) await load();
  }

  Widget archiveRow(Json row) {
    final colors = DshColors(context);
    final cwd = '${row['cwd'] ?? ''}'.replaceAll('\\', '/');
    final workspace = widget.controller.workspaces
        .where((w) => w['path'] == row['cwd'])
        .firstOrNull;
    final label =
        '${workspace?['title'] ?? cwd.split('/').where((s) => s.isNotEmpty).lastOrNull ?? DshSettingsZh.ungrouped}';
    final updated = (row['updatedAt'] as num?)?.toInt() ?? 0;
    final time = updated > 0
        ? DateTime.fromMillisecondsSinceEpoch(updated)
              .toLocal()
              .toString()
              .split('.')
              .first
        : '';
    return Container(
      margin: const EdgeInsets.only(bottom: 12),
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        border: Border.all(color: colors.border),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '${row['title'] ?? DshSettingsZh.unnamedSession}',
                  style: const TextStyle(
                    fontSize: DshTypography.sizeBody,
                    height: 22 / 14,
                  ),
                ),
                const SizedBox(height: 5),
                Text(
                  '$label${time.isEmpty ? '' : DshSettingsZh.updatedAtSuffix(time: time)}',
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    height: 1.5,
                    color: colors.muted,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          DshButton(
            primary: true,
            pill: true,
            height: 32,
            fontSize: DshTypography.sizeAuxiliary,
            onPressed: busy
                ? null
                : () => action(
                    () => widget.controller.archive(
                      '${row['sessionId']}',
                      restore: true,
                    ),
                  ),
            child: const Text(DshSettingsZh.restore),
          ),
          const SizedBox(width: 8),
          DshButton(
            outline: true,
            destructive: true,
            pill: true,
            height: 32,
            fontSize: DshTypography.sizeAuxiliary,
            onPressed: busy
                ? null
                : () async {
                    if (await confirmAction(
                      context,
                      DshSettingsZh.permanentDeleteTitle(
                        title: row['title'] ?? DshSettingsZh.unnamedSession,
                      ),
                      DshSettingsZh.permanentDeleteHint,
                      action: DshSettingsZh.permanentDelete,
                    )) {
                      await action(() async {
                        await api.call('workspace.deleteArchivedSession', {
                          'sessionId': row['sessionId'],
                        }, true);
                      });
                    }
                  },
            child: const Text(DshSettingsZh.delete),
          ),
        ],
      ),
    );
  }

  Widget serverList() => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      const SizedBox(height: 24),
      Row(
        children: [
          const Expanded(
            child: Text(
              DshSettingsZh.mcpServers,
              style: TextStyle(fontWeight: FontWeight.w600),
            ),
          ),
          DshButton(
            outline: true,
            onPressed: loading || busy ? null : () => editServer(),
            icon: DshIcons.plus.data,
            child: const Text(DshSettingsZh.addServer),
          ),
        ],
      ),
      for (final server in objects(data['servers']).where(
        (server) =>
            '${server['name']} ${server['command']} ${server['endpoint']}'
                .toLowerCase()
                .contains(search.text.toLowerCase()),
      ))
        ListTile(
          contentPadding: EdgeInsets.zero,
          title: Text(
            '${server['name']}',
            style: const TextStyle(fontSize: DshTypography.sizeBody),
          ),
          subtitle: Text(
            DshSettingsZh.serverStatus(
              status:
                  server['error'] ??
                  const {
                    'connected': DshSettingsZh.connected,
                    'disabled': DshSettingsZh.disabled,
                    'error': DshSettingsZh.connectionFailed,
                    'pending': DshSettingsZh.awaitingConnection,
                  }[server['status']] ??
                  server['transport'],
              tools: server['toolCount'] ?? 0,
              hasSecrets: server['hasSecrets'] == true,
            ),
            style: const TextStyle(fontSize: DshTypography.sizeCaption),
          ),
          trailing: Wrap(
            children: [
              DshButton(
                onPressed: busy
                    ? null
                    : () => action(() async {
                        final result = await api.call(
                          'capabilities.serverTest',
                          {'name': server['name']},
                          true,
                        );
                        if (result['status'] == 'error' ||
                            result['error'] != null) {
                          throw DshException(
                            'mcp-connection',
                            '${result['error'] ?? DshSettingsZh.connectionFailed}',
                          );
                        }
                        if (mounted) {
                          setState(
                            () => notice = DshSettingsZh.mcpTestSucceeded(
                              name: server['name'],
                              count: result['toolCount'] ?? 0,
                            ),
                          );
                        }
                      }),
                child: const Text(DshSettingsZh.testConnection),
              ),
              DshIcon(
                DshIcons.pencil.data,
                label: DshSettingsZh.editMcpServer,
                onPressed: busy ? null : () => editServer(server),
              ),
              DshIcon(
                DshIcons.trash2.data,
                label: DshSettingsZh.removeMcpServer,
                onPressed: busy ? null : () => removeCapability(server),
              ),
              DshSwitch(
                value: server['enabled'] == true,
                onChanged: busy
                    ? null
                    : (v) => action(() async {
                        await api.call('capabilities.serverToggle', {
                          'name': server['name'],
                          'enabled': v,
                          'expectedRevision': data['revision'],
                        }, true);
                      }),
              ),
            ],
          ),
        ),
    ],
  );
  Widget discovery() {
    final config = object(data['configuration']),
        runtime = object(data['runtime']);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: const Text(
            DshSettingsZh.discoverToolsOnDemand,
            style: TextStyle(fontSize: DshTypography.sizeBody),
          ),
          value: config['enabled'] == true,
          onChanged: busy
              ? null
              : (v) => action(() async {
                  await api.request(
                    '/__dsh-tool-discovery',
                    body: {
                      'configuration': {...config, 'enabled': v},
                      'expectedRevision': data['revision'],
                    },
                    mutation: true,
                  );
                }),
        ),
        if (data['restartRequired'] == true)
          Text(
            DshSettingsZh.restartRequired,
            style: TextStyle(
              fontSize: DshTypography.sizeCaption,
              color: DshTokens.of(context).warning.foreground,
            ),
          ),
        const SizedBox(height: 12),
        for (final entry in runtime.entries)
          if (entry.value is num ||
              entry.value is bool ||
              entry.value is String)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Text(
                '${entry.key}：${entry.value}',
                style: const TextStyle(fontSize: DshTypography.sizeCaption),
              ),
            ),
      ],
    );
  }

  Future<void> editSkill([Json? skill]) async {
    if (busy || loading) return;
    final revision = data['revision'];
    var content = '';
    if (skill != null) {
      await action(() async {
        content =
            '${(await api.call('capabilities.skillRead', {'name': skill['name']}))['content']}';
      });
      if (error != null) return;
    }
    if (!mounted) return;
    final saved = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (_) => TextResourceEditor(
        title: skill == null ? DshSettingsZh.addSkill : DshSettingsZh.editSkill,
        name: skill?['name'] as String? ?? '',
        content: content,
        nameReadOnly: skill != null,
        onSave: (value) async {
          await api.call('capabilities.skillSave', {
            'name': value['name'],
            'content': value['content'],
            'overwrite': skill != null,
            'expectedRevision': revision,
          }, true);
        },
      ),
    );
    if (saved == true && mounted) await load();
  }

  Future<void> viewPreset(Json preset) async {
    await action(() async {
      final result = await api.call('agentPreset.read', {
        'agentPreset': preset['id'],
      });
      if (!mounted) return;
      await showDialog<void>(
        context: context,
        builder: (_) => TextResourceEditor(
          title: DshSettingsZh.agentPresets,
          name: '${preset['id']}',
          content: '${result['content']}',
          readOnly: true,
        ),
      );
    });
  }

  Future<void> copyPreset(Json preset) async {
    final name = await editTextDialog(
      context,
      DshSettingsZh.copyPreset,
      '${preset['id']}-copy',
    );
    if (name == null) return;
    await action(() async {
      await api.call('agentPreset.copy', {
        'source': preset['id'],
        'target': name,
      }, true);
    });
  }

  Future<void> editMemory(Json row) async {
    if (busy || loading) return;
    final saved = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (_) => TextResourceEditor(
        title: DshSettingsZh.editMemory,
        name: '${row['title']}',
        content: '${row['content']}',
        onSave: (value) async {
          await api.call('memory.upsert', {
            'entry': {
              ...row,
              'title': value['name'],
              'content': value['content'],
            },
            'expectedRevision': row['revision'],
          }, true);
        },
      ),
    );
    if (saved == true && mounted) await load();
  }

  Future<void> editServer([Json? server]) async {
    if (busy || loading) return;
    final revision = data['revision'];
    final result = await showDialog<Json>(
      context: context,
      barrierDismissible: false,
      builder: (_) => McpServerDialog(
        initial: server,
        onSave: (value) {
          if (server == null &&
              objects(data['servers'])
                  .any((entry) => entry['name'] == value['name'])) {
            throw DshException('duplicate', DshSettingsZh.duplicateMcpServer);
          }
          return api.call('capabilities.serverSave', {
            'server': value,
            'expectedRevision': revision,
          }, true);
        },
      ),
    );
    if (result != null && mounted) {
      await load();
      if (mounted && (result['status'] == 'error' || result['error'] != null)) {
        setState(
          () => error = DshSettingsZh.mcpSavedConnectionFailed(
            detail: result['error'] ?? DshSettingsZh.connectionFailed,
          ),
        );
      }
    }
  }

  Future<void> removeCapability(Json entry, {bool skill = false}) async {
    if (busy || loading) return;
    final revision = data['revision'];
    if (!await confirmAction(
          context,
          DshSettingsZh.removeResourceTitle(
            kind: skill ? DshSettingsZh.skill : DshSettingsZh.mcpServers,
            name: entry['name'],
          ),
          skill ? DshSettingsZh.removeSkillHint : DshSettingsZh.removeMcpHint,
          action: DshSettingsZh.remove,
        ) ||
        !mounted) {
      return;
    }
    await action(() async {
      await api.call(
        skill ? 'capabilities.skillRemove' : 'capabilities.serverRemove',
        {'name': entry['name'], 'expectedRevision': revision},
        true,
      );
    });
  }
}

class TextResourceEditor extends StatefulWidget {
  const TextResourceEditor({
    super.key,
    required this.title,
    required this.name,
    required this.content,
    this.readOnly = false,
    this.nameReadOnly = false,
    this.onSave,
  });
  final String title, name, content;
  final bool readOnly, nameReadOnly;
  final Future<void> Function(Json)? onSave;
  @override
  State<TextResourceEditor> createState() => _TextResourceEditorState();
}

class _TextResourceEditorState extends State<TextResourceEditor> {
  late final name = TextEditingController(text: widget.name),
      content = TextEditingController(text: widget.content);
  bool busy = false;
  String? error;
  @override
  void dispose() {
    name.dispose();
    content.dispose();
    super.dispose();
  }

  Future<void> save() async {
    if (busy || widget.readOnly) return;
    final value = {'name': name.text.trim(), 'content': content.text};
    if (widget.onSave == null) {
      Navigator.pop(context, value);
      return;
    }
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await widget.onSave!(value);
      if (mounted) Navigator.pop(context, true);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: Dialog(
      child: SizedBox(
        width: 780,
        height: 600,
        child: Padding(
          padding: const EdgeInsets.all(22),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                widget.title,
                style: const TextStyle(
                  fontSize: DshTypography.sizeSectionTitle,
                ),
              ),
              const SizedBox(height: 18),
              DshField(
                controller: name,
                hint: DshSettingsZh.name,
                enabled: !busy && !widget.readOnly && !widget.nameReadOnly,
              ),
              const SizedBox(height: 12),
              Expanded(
                child: TextField(
                  controller: content,
                  readOnly: busy || widget.readOnly,
                  expands: true,
                  maxLines: null,
                  minLines: null,
                  style: TextStyle(
                    fontFamily: DshTypography.monospaceFamily,
                    fontFamilyFallback: DshTypography.monospaceFallback,
                    fontSize: DshTypography.sizeCaption,
                    height: 1.5,
                  ),
                  decoration: const InputDecoration(
                    border: OutlineInputBorder(),
                    hintText: DshSettingsZh.content,
                  ),
                ),
              ),
              if (error != null)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: DshErrorView(error: error!),
                ),
              const SizedBox(height: 16),
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  DshButton(
                    onPressed: busy ? null : () => Navigator.pop(context),
                    child: const Text(DshSettingsZh.close),
                  ),
                  if (!widget.readOnly)
                    DshButton(
                      primary: true,
                      onPressed: busy ? null : save,
                      child: Text(busy ? DshZh.saving : DshZh.save),
                    ),
                ],
              ),
            ],
          ),
        ),
      ),
    ),
  );
}

class _TimeContextPluginSection extends StatefulWidget {
  const _TimeContextPluginSection({
    required this.controller,
    required this.entryId,
    required this.enabled,
    required this.canConfigure,
    required this.entry,
    this.workspace = false,
    this.entryBuilder,
  });

  final DesktopController controller;
  final String entryId;
  final bool enabled;
  final bool Function() canConfigure;
  final Widget entry;
  final bool workspace;
  final Widget Function(VoidCallback? onConfigure, bool expanded)? entryBuilder;

  @override
  State<_TimeContextPluginSection> createState() =>
      _TimeContextPluginSectionState();
}

class _TimeContextPluginSectionState extends State<_TimeContextPluginSection>
    with AutomaticKeepAliveClientMixin {
  bool expanded = false;
  bool visited = false;

  @override
  bool get wantKeepAlive => visited;

  @override
  Widget build(BuildContext context) {
    super.build(context);
    void toggle() {
      // A queued invocation must stay on the Host that owns this draft.
      if (!widget.canConfigure()) return;
      setState(() {
        expanded = !expanded;
        visited = true;
        updateKeepAlive();
      });
    }

    final configuration = visited
        ? Visibility(
            visible: expanded,
            maintainState: true,
            child: Padding(
              padding: EdgeInsets.all(widget.workspace ? 20 : 0),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    widget.enabled
                        ? DshPluginSettingsZh.enabledHint
                        : DshPluginSettingsZh.inactiveHint,
                    style: TextStyle(color: DshColors(context).muted),
                  ),
                  const SizedBox(height: 12),
                  TimeContextPanel(
                    key: ValueKey('time-context-${widget.entryId}'),
                    controller: widget.controller,
                    entryId: widget.entryId,
                  ),
                ],
              ),
            ),
          )
        : const SizedBox.shrink();
    if (widget.workspace) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          widget.entryBuilder!(widget.canConfigure() ? toggle : null, expanded),
          if (visited)
            Container(
              margin: EdgeInsets.only(bottom: expanded ? 12 : 0),
              decoration: BoxDecoration(
                color: expanded ? DshColors(context).layer : null,
                border: expanded
                    ? Border.all(color: DshColors(context).border)
                    : null,
                borderRadius: BorderRadius.circular(12),
              ),
              child: configuration,
            ),
        ],
      );
    }
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 8),
      padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
      decoration: BoxDecoration(
        color: DshColors(context).layer,
        border: Border.all(color: DshColors(context).border),
        borderRadius: BorderRadius.circular(
          DshTokens.of(context).radiusControl,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          widget.entry,
          const SizedBox(height: 8),
          Align(
            alignment: Alignment.centerLeft,
            child: DshButton(
              key: ValueKey('plugin-config-toggle-${widget.entryId}'),
              icon: expanded
                  ? DshIcons.chevronUp.data
                  : DshIcons.chevronDown.data,
              onPressed: !widget.canConfigure() ? null : toggle,
              child: Text(
                expanded
                    ? DshPluginSettingsZh.collapseConfiguration
                    : DshPluginSettingsZh.expandConfiguration,
              ),
            ),
          ),
          if (visited)
            Padding(
              padding: const EdgeInsets.only(top: 12),
              child: configuration,
            ),
        ],
      ),
    );
  }
}

class McpServerDialog extends StatefulWidget {
  const McpServerDialog({super.key, this.initial, this.onSave});
  final Json? initial;
  final Future<Json> Function(Json)? onSave;
  @override
  State<McpServerDialog> createState() => _McpServerDialogState();
}

class _McpServerDialogState extends State<McpServerDialog> {
  late final name = TextEditingController(
        text: widget.initial?['name'] as String? ?? '',
      ),
      command = TextEditingController(
        text: widget.initial?['command'] as String? ?? '',
      ),
      args = TextEditingController(
        text: jsonEncode(widget.initial?['args'] ?? []),
      ),
      cwd = TextEditingController(
        text: widget.initial?['cwd'] as String? ?? '',
      ),
      endpoint = TextEditingController(
        text: widget.initial?['endpoint'] as String? ?? '',
      ),
      env = TextEditingController(),
      headers = TextEditingController();
  late String transport = widget.initial?['transport'] as String? ?? 'stdio';
  late bool enabled = widget.initial?['enabled'] == true;
  bool busy = false;
  String? error;
  @override
  void dispose() {
    name.dispose();
    command.dispose();
    args.dispose();
    cwd.dispose();
    endpoint.dispose();
    env.dispose();
    headers.dispose();
    super.dispose();
  }

  Map<String, String>? parseSecrets(TextEditingController field, String label) {
    if (field.text.trim().isEmpty) return null;
    final value = jsonDecode(field.text);
    if (value is! Map || value.values.any((entry) => entry is! String)) {
      throw FormatException(DshSettingsZh.stringMapRequired(label: label));
    }
    return Map<String, String>.from(value);
  }

  Future<void> save() async {
    if (busy) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      if (!RegExp(r'^[a-zA-Z0-9_-]{1,32}$').hasMatch(name.text.trim())) {
        throw const FormatException(DshSettingsZh.mcpNameInvalid);
      }
      final parsedArgs = args.text.trim().isEmpty
          ? <String>[]
          : jsonDecode(args.text);
      if (parsedArgs is! List || parsedArgs.any((entry) => entry is! String)) {
        throw const FormatException(DshSettingsZh.mcpArgumentsInvalid);
      }
      if (transport == 'stdio' && command.text.trim().isEmpty) {
        throw const FormatException(DshSettingsZh.executableRequired);
      }
      if (transport == 'http') {
        final uri = Uri.tryParse(endpoint.text.trim());
        if (uri == null ||
            !['http', 'https'].contains(uri.scheme) ||
            uri.host.isEmpty) {
          throw const FormatException(DshSettingsZh.serverUrlInvalid);
        }
      }
      final envValue = parseSecrets(env, DshSettingsZh.environmentVariables),
          headerValue = parseSecrets(headers, DshSettingsZh.requestHeaders);
      final value = <String, dynamic>{
        'name': name.text.trim(),
        'transport': transport,
        'command': command.text.trim(),
        'args': parsedArgs,
        'cwd': cwd.text.trim(),
        'endpoint': endpoint.text.trim(),
        'enabled': enabled,
        'env': ?envValue,
        'headers': ?headerValue,
      };
      final result = widget.onSave == null
          ? value
          : await widget.onSave!(value);
      if (mounted) Navigator.pop(context, result);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Widget field(
    TextEditingController controller,
    String label, {
    String? hint,
    int lines = 1,
    bool secret = false,
    bool locked = false,
  }) => Padding(
    padding: const EdgeInsets.only(bottom: 12),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          label,
          style: const TextStyle(fontSize: DshTypography.sizeAuxiliary),
        ),
        const SizedBox(height: 6),
        DshField(
          key: ValueKey('mcp-$label'),
          controller: controller,
          hint: hint,
          maxLines: lines,
          secret: secret,
          enabled: !busy && !locked,
        ),
      ],
    ),
  );

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: AlertDialog(
      title: Text(
        widget.initial == null
            ? DshSettingsZh.addMcpServer
            : DshSettingsZh.editMcpServer,
        style: const TextStyle(fontSize: DshTypography.sizeSectionTitle),
      ),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              field(name, DshSettingsZh.name, locked: widget.initial != null),
              DshSelect<String>(
                options: const {
                  'stdio': DshSettingsZh.localCommand,
                  'http': 'HTTP / HTTPS',
                },
                value: transport,
                onChanged: busy
                    ? null
                    : (value) => setState(() => transport = value),
              ),
              const SizedBox(height: 16),
              if (transport == 'stdio') ...[
                field(
                  command,
                  DshSettingsZh.executable,
                  hint: DshSettingsZh.executableHint,
                ),
                field(
                  args,
                  DshSettingsZh.argumentsJson,
                  hint: '["server.js"]',
                  lines: 2,
                ),
                field(
                  cwd,
                  DshSettingsZh.workingDirectory,
                  hint: DshSettingsZh.defaultWorkingDirectory,
                ),
                field(
                  env,
                  DshSettingsZh.environmentJson,
                  hint: widget.initial == null
                      ? '{"API_KEY":"..."}'
                      : DshSettingsZh.retainSecretHint,
                  secret: true,
                ),
              ] else ...[
                field(
                  endpoint,
                  DshSettingsZh.serverUrl,
                  hint: 'https://example.com/mcp',
                ),
                field(
                  headers,
                  DshSettingsZh.headersJson,
                  hint: widget.initial == null
                      ? '{"Authorization":"Bearer ..."}'
                      : DshSettingsZh.retainSecretHint,
                  secret: true,
                ),
              ],
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                title: const Text(
                  DshSettingsZh.enableServer,
                  style: TextStyle(fontSize: DshTypography.sizeAuxiliary),
                ),
                value: enabled,
                onChanged: busy
                    ? null
                    : (value) => setState(() => enabled = value),
              ),
              Text(
                DshSettingsZh.serverSaveHint,
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: DshColors(context).muted,
                ),
              ),
              if (error != null)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: DshErrorView(error: error!),
                ),
            ],
          ),
        ),
      ),
      actions: [
        DshButton(
          onPressed: busy ? null : () => Navigator.pop(context),
          child: const Text(DshZh.cancel),
        ),
        DshButton(
          primary: true,
          onPressed: busy ? null : save,
          child: Text(busy ? DshZh.saving : DshZh.save),
        ),
      ],
    ),
  );
}
