import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../src/controller.dart';

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
        error = '连接已变化，请关闭后重新打开设置。';
      });
    } else if (session != widget.controller.selectedId) {
      session = widget.controller.selectedId;
      preview = null;
      load();
    }
  }

  DshClient currentApi() {
    if (stale) throw StateError('连接已变化，请关闭后重新打开设置。');
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
            throw StateError('自动经验目录返回的数据不完整。');
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
            throw StateError('经验预览与当前任务不匹配。');
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
          '删除自动经验记录？',
          '删除“${entry['tool'] ?? entry['code'] ?? '此记录'}”后将停止复用此条记录。',
          action: '删除',
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
    final muted = TextStyle(fontSize: 12, height: 1.6, color: colors.muted);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 20),
        const Divider(),
        Row(
          children: [
            const Expanded(
              child: Text(
                '自动经验',
                style: TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
              ),
            ),
            DshIcon(
              LucideIcons.refreshCw,
              label: '刷新自动经验',
              onPressed: disabled ? null : load,
            ),
          ],
        ),
        Text('记录工具失败与恢复，在后续任务中复用已验证的修正建议。运行诊断单独保留，不作为长期规则。', style: muted),
        if (report != null) ...[
          SwitchListTile(
            key: const ValueKey('learning-enabled'),
            contentPadding: EdgeInsets.zero,
            title: const Text('自动捕获与复用', style: TextStyle(fontSize: 13)),
            value: report!['enabled'] == true,
            onChanged: disabled || report!['memoryEnabled'] != true
                ? null
                : (enabled) => mutate('memory.learningConfigure', {
                    'enabled': enabled,
                    'expectedRevision': report!['revision'],
                  }),
          ),
          if (report!['memoryEnabled'] != true)
            Text('持久记忆已关闭，自动经验暂停捕获与复用；已有记录仍可查看。', style: muted),
        ],
        const SizedBox(height: 10),
        DshField(
          key: const ValueKey('learning-search'),
          controller: search,
          hint: '搜索工具、错误代码或修正建议',
          prefix: LucideIcons.search,
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
          options: const {'all': '全部状态', 'pending': '待验证', 'verified': '已验证'},
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
              style: const TextStyle(color: Colors.red, fontSize: 12),
            ),
          ),
        const SizedBox(height: 12),
        previewSection(muted),
        const SizedBox(height: 12),
        Text(
          '经验记录 · ${report?['total'] ?? rows.length}',
          style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w500),
        ),
        if (rows.isEmpty && !loading)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 16),
            child: Text(
              error == null ? '没有匹配的自动经验记录' : '经验目录读取失败，可刷新重试',
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
            child: const Text('显示更多经验'),
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
      title: const Text('当前任务候选经验', style: TextStyle(fontSize: 14)),
      subtitle: Text(
        session == null
            ? '选择任务后查看下次请求的候选经验'
            : '${widget.controller.selected?.title ?? session} · ${items.length} 条',
        style: muted,
      ),
      children: [
        if (previewError != null)
          SelectableText(
            previewError!,
            style: const TextStyle(color: Colors.red, fontSize: 12),
          ),
        if (value != null)
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text('${value['notice'] ?? '实际请求会重新检查匹配条件与预算。'}', style: muted),
              if (value['historyLimited'] == true)
                Text('历史预览仅覆盖有界记录，实际执行前重新读取工具目录。', style: muted),
              Text(
                '预览不会增加复用次数 · ${value['usedCharacters'] ?? 0} / ${value['budget'] ?? 0} 字符',
                style: muted,
              ),
              if (items.isEmpty)
                const Padding(
                  padding: EdgeInsets.symmetric(vertical: 12),
                  child: Text('当前没有符合条件的已验证经验。'),
                ),
              for (final item in items)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: Text(
                    '${item['tool'] ?? item['category'] ?? '经验'}：${item['suggestion'] ?? ''}',
                    style: const TextStyle(fontSize: 13, height: 1.6),
                  ),
                ),
              if (objects(value['excluded']).isNotEmpty)
                Text(
                  '另有 ${objects(value['excluded']).length} 条因工具可用性、规则或预算条件未纳入。',
                  style: muted,
                ),
              if ('${value['text'] ?? ''}'.isNotEmpty)
                ExpansionTile(
                  tilePadding: EdgeInsets.zero,
                  title: const Text(
                    '查看候选上下文内容',
                    style: TextStyle(fontSize: 13),
                  ),
                  children: [
                    SelectableText(
                      '${value['text']}',
                      style: const TextStyle(fontSize: 12, height: 1.6),
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
        ? '运行诊断'
        : entry['status'] == 'verified'
        ? '已验证'
        : '待验证';
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
                  '${entry['tool'] ?? entry['category'] ?? '经验记录'} · $state',
                  style: const TextStyle(
                    fontSize: 14,
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
            '${entry['workspaceLabel'] ?? '工作区未命名'} · 发生 ${entry['occurrences'] ?? 0} 次 · 复用 ${entry['applicationCount'] ?? 0} 次',
            style: muted,
          ),
          if (!reusable) Text('仅用于排障，不注入后续模型上下文。', style: muted),
          if (entry['verification'] == 'user-confirmed')
            Text('验证方式：用户确认', style: muted),
          if (entry['verification'] == 'recovered')
            Text('验证方式：已观察到匹配工具恢复', style: muted),
          if ('${entry['suggestion'] ?? ''}'.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                '${entry['suggestion']}',
                style: const TextStyle(fontSize: 13, height: 1.6),
              ),
            ),
          ExpansionTile(
            tilePadding: EdgeInsets.zero,
            title: Text(
              '错误代码：${entry['code'] ?? '未记录'}',
              style: const TextStyle(fontSize: 12),
            ),
            children: [
              SelectableText('${entry['message'] ?? '未记录诊断详情'}', style: muted),
              if (entry['lastSessionId'] != null)
                SelectableText('任务：${entry['lastSessionId']}', style: muted),
              if (entry['lastCallId'] != null)
                SelectableText('调用：${entry['lastCallId']}', style: muted),
              if (review.isNotEmpty) ...[
                Text(
                  entry['reviewStale'] == true ? '新增事件，需重新核对' : '已记录诊断核查结论',
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
                  child: const Text('确认修正建议'),
                ),
              DshButton(
                key: ValueKey('learning-remove-${entry['id']}'),
                onPressed: disabled ? null : () => remove(entry),
                child: const Text('删除记录'),
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
      setState(() => error = '请填写 1–1000 字的修正步骤与适用条件。');
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
      title: const Text('确认修正建议', style: TextStyle(fontSize: 17)),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                '只确认已经检查的修正建议；记录为用户确认，不代表已观察到工具恢复。',
                style: TextStyle(fontSize: 13, height: 1.6),
              ),
              const SizedBox(height: 12),
              DshField(
                controller: suggestion,
                enabled: !busy,
                maxLines: 6,
                hint: '修正步骤与适用条件',
              ),
              if (error != null)
                Text(
                  error!,
                  style: const TextStyle(color: Colors.red, fontSize: 12),
                ),
            ],
          ),
        ),
      ),
      actions: [
        DshButton(
          onPressed: busy ? null : () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        DshButton(
          primary: true,
          onPressed: busy ? null : save,
          child: Text(busy ? '保存中…' : '确认此建议'),
        ),
      ],
    ),
  );
}
