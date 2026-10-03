import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../design/loading.dart';
import '../../design/rich_content.dart';
import '../../design/select.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

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
  RequestScope readScope = RequestScope();
  List<Json> entries = [];
  bool loading = true, fetching = false;
  bool panelVisible = true, appVisible = true;
  bool get paused => !panelVisible || !appVisible;
  Object? error;
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
    updateVisibility(app: state == AppLifecycleState.resumed);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    updateVisibility(panel: TickerMode.valuesOf(context).enabled);
  }

  void updateVisibility({bool? app, bool? panel}) {
    final wasPaused = paused;
    appVisible = app ?? appVisible;
    panelVisible = panel ?? panelVisible;
    if (paused == wasPaused) return;
    if (paused) {
      readScope.cancel();
    } else {
      readScope = RequestScope();
      fetching = false;
      load();
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    timer?.cancel();
    scope.cancel();
    readScope.cancel();
    super.dispose();
  }

  Future<void> load() async {
    if (fetching || paused) return;
    fetching = true;
    final requestScope = readScope;
    try {
      final result = await widget.api.rpc(
        'subagent.list',
        payload: {'parentSessionId': widget.parent},
        scope: requestScope,
      );
      if (mounted && !requestScope.cancelled) {
        setState(() {
          entries = objects(result['entries']);
          error = null;
        });
      }
    } catch (e) {
      if (mounted && !requestScope.cancelled) setState(() => error = e);
    } finally {
      if (identical(requestScope, readScope)) fetching = false;
      if (mounted && !requestScope.cancelled) {
        setState(() => loading = false);
      }
    }
  }

  Future<void> retry() async {
    readScope.cancel();
    readScope = RequestScope();
    fetching = false;
    setState(() {
      error = null;
      loading = entries.isEmpty;
    });
    await load();
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
                  DshConversationZh.subagentCount(count: entries.length),
                  style: const TextStyle(fontSize: DshTypography.sizeBody),
                ),
              ),
              DshIcon(
                DshIcons.refreshCw.data,
                label: DshConversationZh.refreshSubagents,
                onPressed: load,
              ),
            ],
          ),
        ),
        if (error != null)
          Padding(
            padding: const EdgeInsets.all(12),
            child: DshErrorView(
              error: error!,
              onRetry: fetching || paused ? null : retry,
            ),
          ),
        Expanded(
          child: loading && entries.isEmpty
              ? DshListSkeleton(
                  label: DshConversationZh.loadingList(
                    name: DshConversationZh.subagents,
                  ),
                )
              : entries.isEmpty
              ? DshEmpty(
                  DshConversationZh.noSubagents,
                  icon: DshIcons.users.data,
                )
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
                                          fontSize: DshTypography.sizeAuxiliary,
                                          height: 18 / 13,
                                        ),
                                      ),
                                      Text(
                                        diagnostic
                                            ? switch (row['reason']) {
                                                'corrupt' =>
                                                  DshConversationZh
                                                      .corruptRecord,
                                                'unsupported' =>
                                                  DshConversationZh
                                                      .unsupportedRecordFormat,
                                                _ =>
                                                  DshConversationZh
                                                      .recordUnavailable,
                                              }
                                            : '${row['mode'] == 'continuable' ? DshConversationZh.conversationAllowed : DshConversationZh.oneShotTask} · ${running ? DshConversationZh.running : DshConversationZh.stopped}',
                                        style: TextStyle(
                                          fontSize: DshTypography.sizeCaption,
                                          height: 16 / 11,
                                          color: colors.muted,
                                        ),
                                      ),
                                    ],
                                  ),
                                ),
                                if (!diagnostic)
                                  DshGlyph(
                                    DshIcons.chevronRight.data,
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
  RequestScope readScope = RequestScope();
  final window = ConversationWindow(maxBytes: 2 * 1024 * 1024, maxEvents: 2048);
  List<TranscriptItem> items = [];
  bool loading = true, fetching = false, sending = false, older = false;
  bool panelVisible = true, appVisible = true;
  bool get paused => !panelVisible || !appVisible;
  String delivery = 'queue';
  Object? error;
  bool mutationFailed = false, retryPrevious = false;
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
    updateVisibility(app: state == AppLifecycleState.resumed);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    updateVisibility(panel: TickerMode.valuesOf(context).enabled);
  }

  void updateVisibility({bool? app, bool? panel}) {
    final wasPaused = paused;
    appVisible = app ?? appVisible;
    panelVisible = panel ?? panelVisible;
    if (paused == wasPaused) return;
    if (paused) {
      readScope.cancel();
    } else {
      readScope = RequestScope();
      fetching = false;
      if (!older) load();
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    timer?.cancel();
    scope.cancel();
    readScope.cancel();
    input.dispose();
    super.dispose();
  }

  Future<void> load({bool previous = false}) async {
    if (fetching || paused) return;
    fetching = true;
    retryPrevious = previous;
    final requestScope = readScope;
    try {
      final result = await widget.api.rpc(
        'subagent.history',
        payload: {
          ...address,
          'maxMessages': 100,
          if (previous && window.events.isNotEmpty)
            'beforeSeq': window.events.first.startSeq,
        },
        scope: requestScope,
      );
      final page = HistoryPage.fromJson({
        ...result,
        'hasMoreBefore': result['hasMore'],
        'hasMoreAfter': previous,
      });
      if (!mounted || requestScope.cancelled) return;
      window.replace(page);
      setState(() {
        items = window.project();
        older = previous;
        if (!mutationFailed) error = null;
      });
    } catch (e) {
      if (mounted && !requestScope.cancelled && !mutationFailed) {
        setState(() => error = e);
      }
    } finally {
      if (identical(requestScope, readScope)) fetching = false;
      if (mounted && !requestScope.cancelled) {
        setState(() => loading = false);
      }
    }
  }

  Future<void> retryHistory() async {
    readScope.cancel();
    readScope = RequestScope();
    fetching = false;
    setState(() {
      error = null;
      loading = items.isEmpty;
    });
    await load(previous: retryPrevious);
  }

  Future<void> submit({bool interrupt = false}) async {
    final text = input.text.trim();
    if (sending || (!interrupt && text.isEmpty)) return;
    setState(() {
      sending = true;
      error = null;
      mutationFailed = false;
    });
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
      if (mounted) {
        setState(() {
          error = e;
          mutationFailed = true;
        });
      }
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
              DshIcons.chevronLeft.data,
              label: DshConversationZh.backToSubagents,
              onPressed: widget.onBack,
            ),
            Expanded(
              child: Text(
                '${widget.child['label'] ?? widget.child['id']}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(fontSize: DshTypography.sizeBody),
              ),
            ),
            DshIcon(
              DshIcons.refreshCw.data,
              label: DshConversationZh.refreshChildHistory,
              onPressed: () => load(),
            ),
          ],
        ),
      ),
      if (error != null)
        Padding(
          padding: const EdgeInsets.all(12),
          child: DshErrorView(
            error: error!,
            onRetry: fetching || paused || mutationFailed ? null : retryHistory,
            onDismiss: () => setState(() {
              error = null;
              mutationFailed = false;
            }),
          ),
        ),
      if (window.hasBefore || older)
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (window.hasBefore)
              DshButton(
                height: 28,
                fontSize: DshTypography.sizeCaption,
                onPressed: () => load(previous: true),
                child: const Text(DshConversationZh.earlierHistory),
              ),
            if (older)
              DshButton(
                height: 28,
                fontSize: DshTypography.sizeCaption,
                onPressed: () => load(),
                child: const Text(DshConversationZh.backToLatest),
              ),
          ],
        ),
      Expanded(
        child: loading
            ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
            : items.isEmpty
            ? const DshEmpty(DshConversationZh.noChildHistory)
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
                              ? DshConversationZh.you
                              : item.kind == 'assistant'
                              ? DshConversationZh.assistant
                              : item.title.isNotEmpty
                              ? item.title
                              : DshConversationZh.executionRecords,
                          style: TextStyle(
                            fontSize: DshTypography.sizeCaption,
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
          child: const Text(DshConversationZh.viewNestedSubagents),
        ),
      if (widget.child['mode'] == 'continuable')
        Padding(
          padding: const EdgeInsets.all(12),
          child: Column(
            children: [
              DshField(
                controller: input,
                maxLines: 3,
                hint: DshConversationZh.childMessageHint,
              ),
              const SizedBox(height: 8),
              Row(
                children: [
                  DshSelect<String>(
                    options: const {
                      'queue': DshConversationZh.queueMessage,
                      'steer': DshConversationZh.steerExecution,
                    },
                    value: delivery,
                    onChanged: (v) => setState(() => delivery = v),
                  ),
                  const Spacer(),
                  DshButton(
                    onPressed: sending ? null : () => submit(interrupt: true),
                    child: const Text(DshConversationZh.interrupt),
                  ),
                  ValueListenableBuilder(
                    valueListenable: input,
                    builder: (_, value, _) => DshButton(
                      primary: true,
                      pill: true,
                      onPressed: sending || value.text.trim().isEmpty
                          ? null
                          : submit,
                      child: const Text(DshConversationZh.send),
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
