import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../design/rich_content.dart';
import '../../src/resource_diagnostics.dart';
import 'reclaimable_preview.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

class PlanSnapshot {
  const PlanSnapshot(this.id, this.sourceId, this.text, this.title);
  final int id;
  final String sourceId, text, title;
}

class PlanPreviewStore extends ChangeNotifier {
  static const maxEntries = 8, maxTextUnits = 512 * 1024;
  static const maxRetainedBytes = maxEntries * maxTextUnits * 2;
  final _items = <PlanSnapshot>[];
  Object? _host;
  String? _session;
  int _next = 0;
  int? activeId;
  List<PlanSnapshot> get items => List.unmodifiable(_items);
  PlanSnapshot? get active => _items.where((p) => p.id == activeId).firstOrNull;
  int get retainedBytes => _items.fold(0, (n, p) => n + p.text.length * 2);
  void scope(Object? host, String? session) {
    if (identical(host, _host) && session == _session) return;
    _host = host;
    _session = session;
    clear();
  }

  void open(TranscriptItem item) {
    final text = item.planText;
    if (text == null) throw StateError(DshConversationZh.planUnavailable);
    if (text.length > maxTextUnits) {
      throw StateError(DshConversationZh.planPreviewLimit);
    }
    final title = planHeading(text);
    final entry = PlanSnapshot(++_next, item.id, text, title);
    _items.add(entry);
    while (_items.length > maxEntries) {
      _items.removeAt(0);
    }
    activeId = entry.id;
    notifyListeners();
  }

  void select(int id) {
    if (_items.any((p) => p.id == id)) {
      activeId = id;
      notifyListeners();
    }
  }

  void close(int id) {
    _items.removeWhere((p) => p.id == id);
    if (activeId == id) activeId = _items.lastOrNull?.id;
    notifyListeners();
  }

  void clear() {
    _items.clear();
    activeId = null;
    notifyListeners();
  }

  @override
  void dispose() {
    _items.clear();
    super.dispose();
  }
}

class PlanPreviewPanel extends StatefulWidget {
  const PlanPreviewPanel({
    super.key,
    required this.store,
    required this.onSource,
  });
  final PlanPreviewStore store;
  final VoidCallback onSource;
  @override
  State<PlanPreviewPanel> createState() => _PlanPreviewPanelState();
}

class _PlanPreviewPanelState extends State<PlanPreviewPanel>
    with ResourceDiagnosticScope {
  String? notice;
  @override
  String get resourceScopeKind => 'plan-preview';
  @override
  Map<String, int> get resourceDiagnostics => {'planPreviewPanels': 1};
  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.store,
    builder: (_, _) {
      final active = widget.store.active, colors = DshColors(context);
      return Column(
        children: [
          if (widget.store.items.isNotEmpty)
            SizedBox(
              height: 44 + (MediaQuery.textScalerOf(context).scale(14) - 14),
              child: ListView(
                scrollDirection: Axis.horizontal,
                children: [
                  for (final plan in widget.store.items)
                    Container(
                      color: plan.id == active?.id ? colors.layer : null,
                      child: Row(
                        children: [
                          TextButton(
                            onPressed: () {
                              setState(() => notice = null);
                              widget.store.select(plan.id);
                            },
                            child: ConstrainedBox(
                              constraints: const BoxConstraints(maxWidth: 180),
                              child: Text(
                                plan.title,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                              ),
                            ),
                          ),
                          DshIcon(
                            DshIcons.close.data,
                            label: DshConversationZh.closeNamedPlan(
                              title: plan.title,
                            ),
                            size: 24,
                            onPressed: () => widget.store.close(plan.id),
                          ),
                        ],
                      ),
                    ),
                ],
              ),
            ),
          if (active == null)
            Expanded(
              child: Center(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Text(DshConversationZh.planPreviewExpired),
                    DshButton(
                      onPressed: widget.onSource,
                      child: const Text(DshConversationZh.backToSource),
                    ),
                  ],
                ),
              ),
            )
          else
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Padding(
                    padding: const EdgeInsets.all(16),
                    child: Wrap(
                      spacing: 10,
                      runSpacing: 8,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        Text(
                          active.title,
                          style: const TextStyle(fontWeight: FontWeight.w700),
                        ),
                        DshButton(
                          outline: true,
                          height: 32,
                          onPressed: () async {
                            try {
                              await Clipboard.setData(
                                ClipboardData(text: active.text),
                              );
                              if (mounted &&
                                  widget.store.active?.id == active.id) {
                                setState(
                                  () => notice = DshConversationZh.copied,
                                );
                              }
                            } catch (e) {
                              if (mounted &&
                                  widget.store.active?.id == active.id) {
                                setState(() => notice = '$e');
                              }
                            }
                          },
                          child: const Text(DshConversationZh.copy),
                        ),
                        DshButton(
                          outline: true,
                          height: 32,
                          onPressed: widget.onSource,
                          child: const Text(DshConversationZh.backToSource),
                        ),
                      ],
                    ),
                  ),
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 16),
                    child: Text(
                      notice ?? DshConversationZh.planSnapshotHint,
                      style: TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        color: colors.muted,
                      ),
                    ),
                  ),
                  Expanded(
                    child: ReclaimablePreview(
                      builder: (_) => SingleChildScrollView(
                        key: PageStorageKey(active.id),
                        padding: const EdgeInsets.all(16),
                        child: DshMarkdown(data: active.text),
                      ),
                    ),
                  ),
                ],
              ),
            ),
        ],
      );
    },
  );
}
