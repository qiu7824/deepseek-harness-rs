import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/rich_content.dart';
import '../../design/select.dart';

class SubagentPanel extends StatefulWidget {
  const SubagentPanel({super.key, required this.api, required this.parent});
  final DshClient api;
  final String parent;
  @override
  State<SubagentPanel> createState() => _SubagentPanelState();
}

class _SubagentPanelState extends State<SubagentPanel>
    with WidgetsBindingObserver {
  final scope = RequestScope();
  List<Json> entries = [];
  bool loading = true, fetching = false, paused = false;
  String? error;
  Json? selected;
  Timer? timer;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    load();
    timer = Timer.periodic(const Duration(seconds: 4), (_) {
      if (!paused) load();
    });
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    paused = state != AppLifecycleState.resumed;
    if (!paused) load();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    timer?.cancel();
    scope.cancel();
    super.dispose();
  }

  Future<void> load() async {
    if (fetching) return;
    fetching = true;
    try {
      final result = await widget.api.rpc(
        'subagent.list',
        payload: {'parentSessionId': widget.parent},
        scope: scope,
      );
      if (mounted) {
        setState(() {
          entries = objects(result['entries']);
          error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      fetching = false;
      if (mounted) setState(() => loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (selected != null) {
      return SubagentConversation(
        key: ValueKey(selected!['id']),
        api: widget.api,
        parent: widget.parent,
        child: selected!,
        onBack: () => setState(() => selected = null),
      );
    }
    final colors = DshColors(context);
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  '子任务 · ${entries.length}',
                  style: const TextStyle(fontSize: 14),
                ),
              ),
              DshIcon(LucideIcons.refreshCw, label: '刷新子任务', onPressed: load),
            ],
          ),
        ),
        if (error != null)
          Padding(
            padding: const EdgeInsets.all(12),
            child: Text(
              error!,
              style: const TextStyle(color: Colors.red, fontSize: 12),
            ),
          ),
        Expanded(
          child: loading
              ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
              : entries.isEmpty
              ? const DshEmpty('当前会话没有子任务', icon: LucideIcons.users)
              : ListView.builder(
                  itemCount: entries.length,
                  itemBuilder: (context, index) {
                    final row = entries[index];
                    final diagnostic = row['kind'] == 'diagnostic';
                    final running = row['activity'] == 'running';
                    return Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 8,
                        vertical: 2,
                      ),
                      child: Material(
                        color: Colors.transparent,
                        child: InkWell(
                          borderRadius: BorderRadius.circular(8),
                          onTap: diagnostic
                              ? null
                              : () => setState(() => selected = row),
                          child: Padding(
                            padding: const EdgeInsets.all(10),
                            child: Row(
                              children: [
                                Container(
                                  width: 7,
                                  height: 7,
                                  decoration: BoxDecoration(
                                    shape: BoxShape.circle,
                                    color: running ? colors.blue : colors.muted,
                                  ),
                                ),
                                const SizedBox(width: 10),
                                Expanded(
                                  child: Column(
                                    crossAxisAlignment:
                                        CrossAxisAlignment.start,
                                    children: [
                                      Text(
                                        '${row['label'] ?? row['id']}',
                                        maxLines: 1,
                                        overflow: TextOverflow.ellipsis,
                                        style: const TextStyle(
                                          fontSize: 13,
                                          height: 18 / 13,
                                        ),
                                      ),
                                      Text(
                                        diagnostic
                                            ? switch (row['reason']) {
                                                'corrupt' => '记录损坏',
                                                'unsupported' => '暂不支持的记录格式',
                                                _ => '暂时无法读取',
                                              }
                                            : '${row['mode'] == 'continuable' ? '可继续对话' : '一次性任务'} · ${running ? '运行中' : '已停止'}',
                                        style: TextStyle(
                                          fontSize: 11,
                                          height: 16 / 11,
                                          color: colors.muted,
                                        ),
                                      ),
                                    ],
                                  ),
                                ),
                                if (!diagnostic)
                                  DshGlyph(
                                    LucideIcons.chevronRight,
                                    size: 14,
                                    color: colors.muted,
                                  ),
                              ],
                            ),
                          ),
                        ),
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class SubagentConversation extends StatefulWidget {
  const SubagentConversation({
    super.key,
    required this.api,
    required this.parent,
    required this.child,
    required this.onBack,
  });
  final DshClient api;
  final String parent;
  final Json child;
  final VoidCallback onBack;
  @override
  State<SubagentConversation> createState() => _SubagentConversationState();
}

class _SubagentConversationState extends State<SubagentConversation>
    with WidgetsBindingObserver {
  final scope = RequestScope(), input = TextEditingController();
  final window = ConversationWindow(maxBytes: 2 * 1024 * 1024, maxEvents: 2048);
  List<TranscriptItem> items = [];
  bool loading = true,
      fetching = false,
      sending = false,
      older = false,
      paused = false;
  String delivery = 'queue';
  String? error;
  Timer? timer;
  Json get address => {
    'parentSessionId': widget.parent,
    'childSessionId': widget.child['id'],
    'mode': widget.child['mode'],
  };

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    load();
    timer = Timer.periodic(const Duration(seconds: 4), (_) {
      if (!paused && !older) load();
    });
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    paused = state != AppLifecycleState.resumed;
    if (!paused && !older) load();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    timer?.cancel();
    scope.cancel();
    input.dispose();
    super.dispose();
  }

  Future<void> load({bool previous = false}) async {
    if (fetching) return;
    fetching = true;
    try {
      final result = await widget.api.rpc(
        'subagent.history',
        payload: {
          ...address,
          'maxMessages': 100,
          if (previous && window.events.isNotEmpty)
            'beforeSeq': window.events.first.startSeq,
        },
        scope: scope,
      );
      final page = HistoryPage.fromJson({
        ...result,
        'hasMoreBefore': result['hasMore'],
        'hasMoreAfter': previous,
      });
      if (!mounted) return;
      window.replace(page);
      setState(() {
        items = window.project();
        older = previous;
        error = null;
      });
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      fetching = false;
      if (mounted) setState(() => loading = false);
    }
  }

  Future<void> submit({bool interrupt = false}) async {
    final text = input.text.trim();
    if (sending || (!interrupt && text.isEmpty)) return;
    setState(() => sending = true);
    try {
      await widget.api.rpc(
        interrupt ? 'subagent.interrupt' : 'subagent.prompt',
        mutation: true,
        scope: scope,
        payload: {
          ...address,
          if (!interrupt) ...{
            'content': [
              {'type': 'text', 'text': text},
            ],
            'delivery': delivery,
            'requestId': 'desktop-${DateTime.now().microsecondsSinceEpoch}',
          },
        },
      );
      if (!mounted) return;
      if (!interrupt && input.text.trim() == text) input.clear();
      await load();
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => sending = false);
    }
  }

  @override
  Widget build(BuildContext context) => Column(
    children: [
      Padding(
        padding: const EdgeInsets.all(8),
        child: Row(
          children: [
            DshIcon(
              LucideIcons.chevronLeft,
              label: '返回子任务',
              onPressed: widget.onBack,
            ),
            Expanded(
              child: Text(
                '${widget.child['label'] ?? widget.child['id']}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(fontSize: 14),
              ),
            ),
            DshIcon(
              LucideIcons.refreshCw,
              label: '刷新子任务历史',
              onPressed: () => load(),
            ),
          ],
        ),
      ),
      if (error != null)
        Padding(
          padding: const EdgeInsets.all(12),
          child: Text(
            error!,
            style: const TextStyle(color: Colors.red, fontSize: 12),
          ),
        ),
      if (window.hasBefore || older)
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (window.hasBefore)
              DshButton(
                height: 28,
                fontSize: 12,
                onPressed: () => load(previous: true),
                child: const Text('更早记录'),
              ),
            if (older)
              DshButton(
                height: 28,
                fontSize: 12,
                onPressed: () => load(),
                child: const Text('返回最新'),
              ),
          ],
        ),
      Expanded(
        child: loading
            ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
            : items.isEmpty
            ? const DshEmpty('暂无子任务记录')
            : ListView.builder(
                padding: const EdgeInsets.all(16),
                itemCount: items.length,
                itemBuilder: (context, index) {
                  final item = items[index];
                  return Padding(
                    padding: const EdgeInsets.only(bottom: 20),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          item.kind == 'user'
                              ? '你'
                              : item.kind == 'assistant'
                              ? '助手'
                              : item.title.isNotEmpty
                              ? item.title
                              : '执行记录',
                          style: TextStyle(
                            fontSize: 12,
                            color: DshColors(context).muted,
                          ),
                        ),
                        const SizedBox(height: 6),
                        DshMarkdown(data: item.text),
                      ],
                    ),
                  );
                },
              ),
      ),
      if (widget.child['hasChildren'] == true)
        DshButton(
          onPressed: () => showDialog<void>(
            context: context,
            builder: (_) => Dialog(
              child: SizedBox(
                width: 600,
                height: 650,
                child: SubagentPanel(
                  api: widget.api,
                  parent: '${widget.child['id']}',
                ),
              ),
            ),
          ),
          child: const Text('查看下级子任务'),
        ),
      if (widget.child['mode'] == 'continuable')
        Padding(
          padding: const EdgeInsets.all(12),
          child: Column(
            children: [
              DshField(controller: input, maxLines: 3, hint: '继续向子任务发送消息…'),
              const SizedBox(height: 8),
              Row(
                children: [
                  DshSelect<String>(
                    options: const {'queue': '排队发送', 'steer': '立即引导'},
                    value: delivery,
                    onChanged: (v) => setState(() => delivery = v),
                  ),
                  const Spacer(),
                  DshButton(
                    onPressed: sending ? null : () => submit(interrupt: true),
                    child: const Text('中断'),
                  ),
                  ValueListenableBuilder(
                    valueListenable: input,
                    builder: (_, value, _) => DshButton(
                      primary: true,
                      pill: true,
                      onPressed: sending || value.text.trim().isEmpty
                          ? null
                          : submit,
                      child: const Text('发送'),
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
    ],
  );
}
