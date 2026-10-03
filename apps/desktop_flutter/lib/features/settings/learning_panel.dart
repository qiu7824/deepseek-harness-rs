import '../../design/error.dart';
import '../../l10n/zh.dart';

import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../src/controller.dart';

import 'package:dsh_desktop/design/typography.dart';

class LearningPanel extends StatefulWidget {
  const LearningPanel({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<LearningPanel> createState() => _LearningPanelState();
}

class _LearningPanelState extends State<LearningPanel> {
  late final DshClient? api = widget.controller.client;
  final search = TextEditingController(), mutationScope = RequestScope();
  RequestScope readScope = RequestScope();
  Timer? debounce;
  Json? report, preview;
  String? error, previewError;
  late String? session = widget.controller.selectedId;
  String status = 'all';
  int generation = 0, limit = 50;
  bool loading = true, busy = false;
  bool get stale => api == null || api != widget.controller.client;
  bool get disabled => loading || busy || stale;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(changed);
    load();
  }

  @override
  void dispose() {
    widget.controller.removeListener(changed);
    debounce?.cancel();
    readScope.cancel();
    mutationScope.cancel();
    search.dispose();
    super.dispose();
  }

  void changed() {
    if (stale) {
      generation++;
      readScope.cancel();
      mutationScope.cancel();
      debounce?.cancel();
      setState(() {
        preview = null;
        loading = false;
        error = DshSettingsZh.connectionChanged;
      });
    } else if (session != widget.controller.selectedId) {
      session = widget.controller.selectedId;
      preview = null;
      load();
    }
  }

  DshClient currentApi() {
    if (stale) throw StateError(DshSettingsZh.connectionChanged);
    return api!;
  }

  Future<void> load() async {
    if (!mounted || stale) return;
    debounce?.cancel();
    final version = ++generation, selected = session;
    readScope.cancel();
    final scope = readScope = RequestScope();
    bool current() => mounted && !stale && version == generation;
    setState(() {
      loading = true;
      error = null;
      previewError = null;
      preview = null;
    });
    await Future.wait([
      () async {
        try {
          final value = await api!.rpc(
            'memory.learningList',
            payload: {
              'limit': limit,
              if (search.text.trim().isNotEmpty) 'query': search.text.trim(),
              if (status != 'all') 'status': status,
            },
            scope: scope,
          );
          if (value['items'] is! List ||
              value['revision'] is! num ||
              value['enabled'] is! bool ||
              value['memoryEnabled'] is! bool) {
            throw StateError(DshSettingsZh.learningResponseInvalid);
          }
          if (current()) setState(() => report = value);
        } catch (e) {
          if (current()) setState(() => error = '$e');
        }
      }(),
      () async {
        if (selected == null) {
          if (current()) setState(() => preview = null);
          return;
        }
        try {
          final value = await api!.rpc(
            'memory.learningPreview',
            payload: {'sessionId': selected},
            scope: scope,
          );
          if (value['items'] is! List ||
              value['text'] is! String ||
              value['sessionId'] != selected) {
            throw StateError(DshSettingsZh.learningPreviewMismatch);
          }
          if (current()) setState(() => preview = value);
        } catch (e) {
          if (current()) {
            setState(() {
              preview = null;
              previewError = '$e';
            });
          }
        }
      }(),
    ]);
    if (current()) setState(() => loading = false);
  }

  Future<void> mutate(String method, Json payload) async {
    if (disabled) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await currentApi().rpc(
        method,
        payload: payload,
        mutation: true,
        scope: mutationScope,
      );
      if (mounted && !stale) await load();
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> confirm(Json entry) async {
    if (disabled || entry['reusableRule'] != true) return;
    final saved = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (_) => LearningSuggestionEditor(
        initial: '${entry['suggestion'] ?? ''}',
        onSave: (suggestion) async {
          await currentApi().rpc(
            'memory.learningConfirm',
            payload: {
              'id': entry['id'],
              'expectedRevision': entry['revision'],
              'confirmed': true,
              'suggestion': suggestion,
            },
            mutation: true,
            scope: mutationScope,
          );
        },
      ),
    );
    if (saved == true && mounted && !stale) await load();
  }

  Future<void> remove(Json entry) async {
    if (disabled) return;
    if (await confirmAction(
          context,
          DshSettingsZh.deleteLearningTitle,
          DshSettingsZh.deleteLearningHint(
            title: entry['tool'] ?? entry['code'] ?? DshSettingsZh.thisRecord,
          ),
          action: DshSettingsZh.delete,
        ) &&
        mounted) {
      await mutate('memory.learningRemove', {
        'id': entry['id'],
        'expectedRevision': entry['revision'],
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final rows = objects(report?['items']), colors = DshColors(context);
    final muted = TextStyle(
      fontSize: DshTypography.sizeCaption,
      height: 1.6,
      color: colors.muted,
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 20),
        const Divider(),
        Row(
          children: [
            const Expanded(
              child: Text(
                DshSettingsZh.learning,
                style: TextStyle(
                  fontSize: DshTypography.sizeComposer,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            DshIcon(
              DshIcons.refreshCw.data,
              label: DshSettingsZh.refreshLearning,
              onPressed: disabled ? null : load,
            ),
          ],
        ),
        Text(DshSettingsZh.learningDescription, style: muted),
        if (report != null) ...[
          SwitchListTile(
            key: const ValueKey('learning-enabled'),
            contentPadding: EdgeInsets.zero,
            title: const Text(
              DshSettingsZh.captureLearning,
              style: TextStyle(fontSize: DshTypography.sizeAuxiliary),
            ),
            value: report!['enabled'] == true,
            onChanged: disabled || report!['memoryEnabled'] != true
                ? null
                : (enabled) => mutate('memory.learningConfigure', {
                    'enabled': enabled,
                    'expectedRevision': report!['revision'],
                  }),
          ),
          if (report!['memoryEnabled'] != true)
            Text(DshSettingsZh.memoryDisabledLearning, style: muted),
        ],
        const SizedBox(height: 10),
        DshField(
          key: const ValueKey('learning-search'),
          controller: search,
          hint: DshSettingsZh.searchLearning,
          prefix: DshIcons.search.data,
          enabled: !busy && !stale,
          onChanged: (_) {
            debounce?.cancel();
            generation++;
            readScope.cancel();
            setState(() => loading = true);
            debounce = Timer(const Duration(milliseconds: 300), () {
              limit = 50;
              load();
            });
          },
        ),
        const SizedBox(height: 10),
        DshSelect<String>(
          value: status,
          options: const {
            'all': DshSettingsZh.allStatuses,
            'pending': DshSettingsZh.unverified,
            'verified': DshSettingsZh.verified,
          },
          onChanged: busy || stale
              ? null
              : (value) {
                  setState(() {
                    status = value;
                    limit = 50;
                  });
                  load();
                },
        ),
        if (loading || busy)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 12),
            child: LinearProgressIndicator(minHeight: 2),
          ),
        for (final message in [
          error,
          report?['lastError'] as String?,
        ].whereType<String>())
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: SelectableText(
              message,
              style: TextStyle(
                color: DshTokens.of(context).error.foreground,
                fontSize: DshTypography.sizeCaption,
              ),
            ),
          ),
        const SizedBox(height: 12),
        previewSection(muted),
        const SizedBox(height: 12),
        Text(
          DshSettingsZh.learningRecordCount(
            count: report?['total'] ?? rows.length,
          ),
          style: const TextStyle(
            fontSize: DshTypography.sizeBody,
            fontWeight: FontWeight.w500,
          ),
        ),
        if (rows.isEmpty && !loading)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 16),
            child: Text(
              error == null
                  ? DshSettingsZh.noLearningMatches
                  : DshSettingsZh.learningLoadFailed,
              style: muted,
            ),
          ),
        for (final entry in rows) entryCard(entry, muted),
        if (report != null && (report!['total'] as num? ?? 0) > rows.length)
          DshButton(
            outline: true,
            onPressed: disabled
                ? null
                : () {
                    limit += 50;
                    load();
                  },
            child: const Text(DshSettingsZh.moreLearning),
          ),
        const SizedBox(height: 20),
      ],
    );
  }

  Widget previewSection(TextStyle muted) {
    final value = preview, items = objects(preview?['items']);
    return ExpansionTile(
      key: ValueKey(('learning-preview', session)),
      tilePadding: EdgeInsets.zero,
      childrenPadding: const EdgeInsets.only(bottom: 12),
      title: const Text(
        DshSettingsZh.currentLearning,
        style: TextStyle(fontSize: DshTypography.sizeBody),
      ),
      subtitle: Text(
        session == null
            ? DshSettingsZh.selectLearningSession
            : DshSettingsZh.candidateCount(
                title: widget.controller.selected?.title ?? session,
                count: items.length,
              ),
        style: muted,
      ),
      children: [
        if (previewError != null) DshErrorView(error: previewError!),
        if (value != null)
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                '${value['notice'] ?? DshSettingsZh.learningBudgetHint}',
                style: muted,
              ),
              if (value['historyLimited'] == true)
                Text(DshSettingsZh.historicalLearningHint, style: muted),
              Text(
                DshSettingsZh.learningBudget(
                  used: value['usedCharacters'] ?? 0,
                  budget: value['budget'] ?? 0,
                ),
                style: muted,
              ),
              if (items.isEmpty)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 12),
                  child: Text(DshSettingsZh.noVerifiedLearning),
                ),
              for (final item in items)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: Text(
                    '${item['tool'] ?? item['category'] ?? DshSettingsZh.learningShort}：${item['suggestion'] ?? ''}',
                    style: const TextStyle(
                      fontSize: DshTypography.sizeAuxiliary,
                      height: 1.6,
                    ),
                  ),
                ),
              if (objects(value['excluded']).isNotEmpty)
                Text(
                  DshSettingsZh.excludedLearning(
                    count: objects(value['excluded']).length,
                  ),
                  style: muted,
                ),
              if ('${value['text'] ?? ''}'.isNotEmpty)
                ExpansionTile(
                  tilePadding: EdgeInsets.zero,
                  title: const Text(
                    DshSettingsZh.viewCandidateContext,
                    style: TextStyle(fontSize: DshTypography.sizeAuxiliary),
                  ),
                  children: [
                    SelectableText(
                      '${value['text']}',
                      style: const TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        height: 1.6,
                      ),
                    ),
                  ],
                ),
            ],
          ),
      ],
    );
  }

  Widget entryCard(Json entry, TextStyle muted) {
    final reusable = entry['reusableRule'] == true;
    final review = object(entry['review']);
    final state = !reusable
        ? DshSettingsZh.diagnostics
        : entry['status'] == 'verified'
        ? DshSettingsZh.verified
        : DshSettingsZh.unverified;
    return Container(
      key: ValueKey('learning-entry-${entry['id']}'),
      margin: const EdgeInsets.only(top: 12),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        border: Border.all(color: DshColors(context).border),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  '${entry['tool'] ?? entry['category'] ?? DshSettingsZh.learningRecords} · $state',
                  style: const TextStyle(
                    fontSize: DshTypography.sizeBody,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
              if (reusable)
                DshSwitch(
                  key: ValueKey('learning-toggle-${entry['id']}'),
                  value: entry['enabled'] == true,
                  onChanged: disabled
                      ? null
                      : (enabled) => mutate('memory.learningToggle', {
                          'id': entry['id'],
                          'enabled': enabled,
                          'expectedRevision': entry['revision'],
                        }),
                ),
            ],
          ),
          const SizedBox(height: 6),
          Text(
            DshSettingsZh.learningUsage(
              workspace:
                  entry['workspaceLabel'] ?? DshSettingsZh.unnamedWorkspace,
              occurrences: entry['occurrences'] ?? 0,
              applications: entry['applicationCount'] ?? 0,
            ),
            style: muted,
          ),
          if (!reusable) Text(DshSettingsZh.diagnosticOnly, style: muted),
          if (entry['verification'] == 'user-confirmed')
            Text(DshSettingsZh.userVerified, style: muted),
          if (entry['verification'] == 'recovered')
            Text(DshSettingsZh.recoveredTool, style: muted),
          if ('${entry['suggestion'] ?? ''}'.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                '${entry['suggestion']}',
                style: const TextStyle(
                  fontSize: DshTypography.sizeAuxiliary,
                  height: 1.6,
                ),
              ),
            ),
          ExpansionTile(
            tilePadding: EdgeInsets.zero,
            title: Text(
              DshSettingsZh.errorCode(
                code: entry['code'] ?? DshSettingsZh.noRecord,
              ),
              style: const TextStyle(fontSize: DshTypography.sizeCaption),
            ),
            children: [
              SelectableText(
                '${entry['message'] ?? DshSettingsZh.noDiagnosticDetails}',
                style: muted,
              ),
              if (entry['lastSessionId'] != null)
                SelectableText(
                  DshSettingsZh.diagnosticSession(id: entry['lastSessionId']),
                  style: muted,
                ),
              if (entry['lastCallId'] != null)
                SelectableText(
                  DshSettingsZh.diagnosticCall(id: entry['lastCallId']),
                  style: muted,
                ),
              if (review.isNotEmpty) ...[
                Text(
                  entry['reviewStale'] == true
                      ? DshSettingsZh.diagnosticChanged
                      : DshSettingsZh.diagnosticChecked,
                  style: muted,
                ),
                SelectableText('${review['summary'] ?? ''}', style: muted),
                for (final evidence in review['evidence'] as List? ?? const [])
                  SelectableText('$evidence', style: muted),
              ],
            ],
          ),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              if (reusable && entry['status'] == 'pending')
                DshButton(
                  outline: true,
                  onPressed: disabled ? null : () => confirm(entry),
                  child: const Text(DshSettingsZh.confirmLearningTitle),
                ),
              DshButton(
                key: ValueKey('learning-remove-${entry['id']}'),
                onPressed: disabled ? null : () => remove(entry),
                child: const Text(DshSettingsZh.deleteRecord),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class LearningSuggestionEditor extends StatefulWidget {
  const LearningSuggestionEditor({
    super.key,
    required this.initial,
    required this.onSave,
  });
  final String initial;
  final Future<void> Function(String) onSave;
  @override
  State<LearningSuggestionEditor> createState() =>
      _LearningSuggestionEditorState();
}

class _LearningSuggestionEditorState extends State<LearningSuggestionEditor> {
  late final suggestion = TextEditingController(text: widget.initial);
  bool busy = false;
  String? error;
  @override
  void dispose() {
    suggestion.dispose();
    super.dispose();
  }

  Future<void> save() async {
    if (busy) return;
    final value = suggestion.text.trim();
    if (value.isEmpty || value.runes.length > 1000) {
      setState(() => error = DshSettingsZh.correctionLengthInvalid);
      return;
    }
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await widget.onSave(value);
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
    child: AlertDialog(
      title: const Text(
        DshSettingsZh.confirmLearningTitle,
        style: TextStyle(fontSize: DshTypography.sizeSectionTitle),
      ),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                DshSettingsZh.confirmLearningHint,
                style: TextStyle(
                  fontSize: DshTypography.sizeAuxiliary,
                  height: 1.6,
                ),
              ),
              const SizedBox(height: 12),
              DshField(
                controller: suggestion,
                enabled: !busy,
                maxLines: 6,
                hint: DshSettingsZh.correctionSteps,
              ),
              if (error != null) DshErrorView(error: error!),
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
          child: Text(busy ? DshZh.saving : DshSettingsZh.confirmCorrection),
        ),
      ],
    ),
  );
}
