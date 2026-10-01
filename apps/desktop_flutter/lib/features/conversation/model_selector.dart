import 'dart:async';
import 'dart:math' as math;

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';
import '../../l10n/conversation_zh.dart';
import '../../src/controller.dart';
import 'reasoning_slider.dart';

ModelChoice? _currentChoice(ModelCatalog? catalog) => catalog?.choices
    .where((choice) => choice.key == catalog.currentKey)
    .firstOrNull;

String composerModelLabel(ModelCatalog? catalog) =>
    catalog?.currentName ?? DshConversationZh.chooseModel;

String composerReasoningLabel(ModelCatalog? catalog) {
  if (catalog == null) return '模型默认';
  final choice = _currentChoice(catalog);
  final effort = catalog.current['reasoningEffort'] as String?;
  final effective = effort ?? choice?.defaultReasoningEffort;
  if (effective == null) return '模型默认';
  final label = reasoningLevelLabel(
    choice?.reasoning.where((level) => level['id'] == effective).firstOrNull ??
        {'id': effective, 'name': effective},
  );
  return effort == null ? '模型默认（$label）' : label;
}

bool hasReasoning(ModelCatalog? catalog) {
  final choice = _currentChoice(catalog);
  return choice != null &&
      (choice.reasoning.isNotEmpty || choice.defaultReasoningEffort != null);
}

Object _scope(DesktopController controller) => (
  controller,
  controller.client,
  controller.host,
  controller.selectedId,
  controller.selectionRevision,
  controller.modelCatalogScope,
);

double _panelWidth(BuildContext context) =>
    math.max(80, math.min(360, MediaQuery.sizeOf(context).width - 48));
double _panelHeight(BuildContext context) =>
    math.max(80, math.min(480, MediaQuery.sizeOf(context).height - 80));

/// A compact model summary with a shared, hierarchical selection menu.
class ModelSelectionControls extends StatefulWidget {
  const ModelSelectionControls({
    super.key,
    required this.controller,
    this.onManage,
    this.onReturnFocus,
  });
  final DesktopController controller;
  final VoidCallback? onManage;
  final VoidCallback? onReturnFocus;

  @override
  State<ModelSelectionControls> createState() => _ModelSelectionControlsState();
}

enum _SelectionPage { summary, models, reasoning }

class _ModelSelectionControlsState extends State<ModelSelectionControls> {
  final menu = ShadPopoverController();
  final trigger = FocusNode();
  final menuFocus = FocusNode();
  Object? openedScope;
  Object? openedReasoningScope;
  var page = _SelectionPage.summary;
  bool keyboardActive = false;
  bool keyboardNavigated = false;
  int highlighted = 0;

  Object get scope => _scope(widget.controller);
  Object get reasoningScope =>
      (scope, widget.controller.modelPreviewCatalog?.currentKey);

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(changed);
    menu.addListener(syncKeyboard);
  }

  @override
  void didUpdateWidget(ModelSelectionControls oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, widget.controller)) {
      oldWidget.controller.removeListener(changed);
      widget.controller.addListener(changed);
    }
    changed();
  }

  void changed() {
    if (!mounted) return;
    if (!widget.controller.connected ||
        (menu.isOpen && openedScope != scope) ||
        (menu.isOpen &&
            page == _SelectionPage.reasoning &&
            openedReasoningScope != reasoningScope)) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) menu.hide();
      });
    }
    setState(() {});
  }

  void syncKeyboard() {
    if (menu.isOpen == keyboardActive) return;
    keyboardActive = menu.isOpen;
    if (keyboardActive) {
      FocusManager.instance.addEarlyKeyEventHandler(handleKey);
    } else {
      FocusManager.instance.removeEarlyKeyEventHandler(handleKey);
    }
  }

  void navigate(_SelectionPage destination) {
    setState(() {
      page = destination;
      highlighted = 0;
      keyboardNavigated = false;
      if (destination == _SelectionPage.reasoning) {
        openedReasoningScope = reasoningScope;
      }
    });
  }

  KeyEventResult handleKey(KeyEvent event) {
    if (!mounted ||
        !keyboardActive ||
        event is! KeyDownEvent ||
        ModalRoute.of(context)?.isCurrent == false) {
      return KeyEventResult.ignored;
    }
    if (event.logicalKey == LogicalKeyboardKey.escape) {
      if (page == _SelectionPage.summary) {
        dismiss();
      } else {
        navigate(_SelectionPage.summary);
      }
      return KeyEventResult.handled;
    }
    if (page == _SelectionPage.models) return KeyEventResult.ignored;
    final c = widget.controller;
    final levels = _currentChoice(c.modelPreviewCatalog)?.reasoning ?? <Json>[];
    final count = page == _SelectionPage.summary
        ? (hasReasoning(c.modelPreviewCatalog) ? 2 : 1)
        : levels.length;
    if (event.logicalKey == LogicalKeyboardKey.arrowDown ||
        event.logicalKey == LogicalKeyboardKey.arrowUp) {
      keyboardNavigated = true;
      menuFocus.requestFocus();
      if (count > 0) {
        setState(
          () => highlighted =
              (highlighted +
                  (event.logicalKey == LogicalKeyboardKey.arrowDown ? 1 : -1)) %
              count,
        );
      }
      return KeyEventResult.handled;
    }
    if (menuFocus.hasPrimaryFocus &&
        (event.logicalKey == LogicalKeyboardKey.enter ||
            event.logicalKey == LogicalKeyboardKey.numpadEnter ||
            event.logicalKey == LogicalKeyboardKey.arrowRight)) {
      if (page == _SelectionPage.summary) {
        navigate(
          highlighted == 0 ? _SelectionPage.models : _SelectionPage.reasoning,
        );
      } else if (levels.isNotEmpty) {
        chooseReasoning(
          '${levels[highlighted.clamp(0, levels.length - 1)]['id']}',
        );
      }
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.arrowLeft &&
        page != _SelectionPage.summary) {
      navigate(_SelectionPage.summary);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  void dismiss({bool restoreFocus = true}) {
    menu.hide();
    if (!restoreFocus) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || ModalRoute.of(context)?.isCurrent == false) return;
      if (widget.onReturnFocus != null) {
        widget.onReturnFocus!();
      } else {
        trigger.requestFocus();
      }
    });
  }

  bool get canChoose =>
      menu.isOpen &&
      openedScope == scope &&
      widget.controller.connected &&
      !widget.controller.sending;

  void choose(ModelChoice choice) {
    if (!canChoose) return;
    final c = widget.controller;
    dismiss();
    unawaited(c.run(() => c.chooseModel(choice)));
  }

  void chooseReasoning(String id) {
    if (!canChoose || openedReasoningScope != reasoningScope) return;
    final c = widget.controller;
    final actionScope = reasoningScope;
    dismiss();
    unawaited(() async {
      try {
        await c.setReasoning(id);
      } catch (error) {
        if (mounted && actionScope == reasoningScope) {
          c.error = error.toString();
          c.emit();
        }
      }
    }());
  }

  Widget menuRow({
    required Key key,
    required String label,
    String? value,
    String? tooltip,
    Widget? trailing,
    required VoidCallback? onPressed,
    required int index,
  }) => Focus(
    canRequestFocus: false,
    skipTraversal: true,
    onFocusChange: (focused) {
      if (focused && mounted) setState(() => highlighted = index);
    },
    child: DshButton(
      key: key,
      tooltip: tooltip ?? value ?? label,
      height: 36,
      padding: const EdgeInsets.symmetric(horizontal: 10),
      onPressed: onPressed,
      active: index == highlighted && keyboardNavigated && menuFocus.hasFocus,
      child: Expanded(
        child: Row(
          children: [
            if (value == null)
              Expanded(
                child: Text(
                  label,
                  textAlign: TextAlign.left,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              )
            else
              Text(label),
            if (value != null) const SizedBox(width: 20),
            if (value != null)
              Expanded(
                child: Text(
                  value,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  textAlign: TextAlign.right,
                  style: DshTypography.body.copyWith(
                    color: DshColors(context).muted,
                  ),
                ),
              ),
            const SizedBox(width: 8),
            trailing ??
                DshGlyph(
                  DshIcons.chevronRight.data,
                  size: 14,
                  color: DshColors(context).muted,
                ),
          ],
        ),
      ),
    ),
  );

  Widget content(BuildContext context) {
    final c = widget.controller;
    if (!menu.isOpen || openedScope != scope || !c.connected) {
      return const SizedBox.shrink();
    }
    if (page == _SelectionPage.models) {
      return ModelPicker(
        controller: c,
        onSelect: choose,
        onDismiss: () => navigate(_SelectionPage.summary),
        onBack: () => navigate(_SelectionPage.summary),
        onManage: widget.onManage == null
            ? null
            : () {
                dismiss(restoreFocus: false);
                widget.onManage!();
              },
      );
    }
    final preview = c.modelPreviewCatalog;
    final choice = _currentChoice(preview);
    final confirmed = c.availableModelCatalog;
    final value = confirmed?.currentKey == preview?.currentKey
        ? confirmed?.current['reasoningEffort'] as String?
        : null;
    final pending = c.pendingModelSelection?['reasoningEffort'] as String?;
    final colors = DshColors(context);
    final width = math.min(
      _panelWidth(context),
      (page == _SelectionPage.summary ? 304 : 240) *
          MediaQuery.textScalerOf(context).scale(1),
    );
    return Focus(
      focusNode: menuFocus,
      autofocus: true,
      child: DefaultTextStyle(
        style: DshTypography.body.copyWith(color: colors.text),
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: _panelHeight(context)),
          child: SingleChildScrollView(
            child: SizedBox(
              key: ValueKey(
                page == _SelectionPage.summary
                    ? 'model-selection-menu'
                    : 'reasoning-menu',
              ),
              width: width,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: page == _SelectionPage.summary
                    ? [
                        menuRow(
                          key: const ValueKey('model-menu-model-row'),
                          label: '模型',
                          value: composerModelLabel(preview),
                          index: 0,
                          onPressed: () => navigate(_SelectionPage.models),
                        ),
                        if (hasReasoning(preview))
                          menuRow(
                            key: const ValueKey('reasoning-picker-trigger'),
                            label: '思考等级',
                            value: composerReasoningLabel(preview),
                            index: 1,
                            onPressed: () => navigate(_SelectionPage.reasoning),
                          ),
                        _DefaultNotice(controller: c),
                      ]
                    : [
                        Row(
                          children: [
                            DshIcon(
                              DshIcons.chevronLeft.data,
                              key: const ValueKey('model-menu-back'),
                              label: '返回模型菜单',
                              onPressed: () => navigate(_SelectionPage.summary),
                            ),
                            Text(
                              '思考等级',
                              style: DshTypography.body.copyWith(
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                          ],
                        ),
                        if (value == null &&
                            choice?.defaultReasoningEffort != null)
                          Padding(
                            padding: const EdgeInsets.fromLTRB(10, 0, 10, 6),
                            child: Text(
                              composerReasoningLabel(preview),
                              style: DshTypography.caption.copyWith(
                                color: colors.muted,
                              ),
                            ),
                          ),
                        for (final (index, level)
                            in (choice?.reasoning ?? <Json>[]).indexed)
                          menuRow(
                            key: ValueKey('reasoning-level-${level['id']}'),
                            label: reasoningLevelLabel(level),
                            tooltip:
                                '${level['description'] ?? reasoningLevelLabel(level)}',
                            index: index,
                            trailing: pending == level['id']
                                ? DshGlyph(
                                    DshIcons.clock.data,
                                    key: ValueKey(
                                      'reasoning-pending-${level['id']}',
                                    ),
                                    size: 14,
                                    color: colors.muted,
                                  )
                                : (value ?? choice?.defaultReasoningEffort) ==
                                      level['id']
                                ? DshGlyph(
                                    DshIcons.check.data,
                                    key: ValueKey(
                                      'reasoning-selected-${level['id']}',
                                    ),
                                    size: 14,
                                    color: colors.text,
                                  )
                                : const SizedBox(width: 14),
                            onPressed: c.sending
                                ? null
                                : () => chooseReasoning('${level['id']}'),
                          ),
                        _DefaultNotice(controller: c),
                      ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final catalog = c.modelPreviewCatalog;
    final name = composerModelLabel(catalog);
    final choice = _currentChoice(catalog);
    final effective =
        catalog?.current['reasoningEffort'] as String? ??
        choice?.defaultReasoningEffort;
    final effort = effective == null
        ? '默认'
        : reasoningLevelLabel(
            choice?.reasoning
                    .where((level) => level['id'] == effective)
                    .firstOrNull ??
                {'id': effective, 'name': effective},
          );
    final showEffort = hasReasoning(catalog);
    final colors = DshColors(context);
    final style = DshTypography.body.copyWith(
      fontSize: 13,
      color: colors.muted,
    );
    double measure(String value) {
      final painter = TextPainter(
        text: TextSpan(text: value, style: style),
        textDirection: Directionality.of(context),
        textScaler: MediaQuery.textScalerOf(context),
        maxLines: 1,
      )..layout();
      final width = painter.width;
      painter.dispose();
      return width;
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        final maxWidth = constraints.maxWidth.isFinite
            ? constraints.maxWidth
            : 320.0;
        final width = math.min(
          maxWidth,
          measure(name) + (showEffort ? measure(effort) + 8 : 0) + 44,
        );
        return Align(
          alignment: Alignment.centerRight,
          widthFactor: 1,
          child: ShadPopover(
            controller: menu,
            padding: const EdgeInsets.all(6),
            effects: const [],
            reverseDuration: Duration.zero,
            anchor: const ShadAnchorAuto(
              targetAnchor: Alignment.topRight,
              followerAnchor: Alignment.topLeft,
              offset: Offset(0, -8),
              fallback: ShadAnchorAuto(
                targetAnchor: Alignment.bottomRight,
                followerAnchor: Alignment.bottomLeft,
                offset: Offset(0, 8),
              ),
            ),
            popover: content,
            child: SizedBox(
              width: width,
              child: DshButton(
                key: const ValueKey('model-picker-trigger'),
                focusNode: trigger,
                tooltip: showEffort
                    ? '选择模型与思考等级：$name · ${composerReasoningLabel(catalog)}'
                    : '选择模型：$name',
                height: 32,
                fontSize: 13,
                padding: const EdgeInsets.symmetric(horizontal: 8),
                trailing: DshGlyph(
                  c.pendingModelSelection != null
                      ? DshIcons.clock.data
                      : DshIcons.chevronDown.data,
                  size: 14,
                  color: colors.muted,
                ),
                onPressed: c.connected && !c.sending
                    ? () {
                        if (menu.isOpen) {
                          dismiss();
                          return;
                        }
                        openedScope = scope;
                        page = _SelectionPage.summary;
                        highlighted = 0;
                        keyboardNavigated = false;
                        menu.show();
                      }
                    : null,
                child: Flexible(
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Flexible(
                        child: Text(
                          name,
                          key: const ValueKey('model-trigger-name'),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: style,
                        ),
                      ),
                      if (showEffort) ...[
                        const SizedBox(width: 8),
                        ConstrainedBox(
                          constraints: BoxConstraints(
                            maxWidth: math.max(0, width * .35),
                          ),
                          child: Text(
                            effort,
                            key: const ValueKey('model-trigger-reasoning'),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: style,
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  @override
  void dispose() {
    widget.controller.removeListener(changed);
    menu.removeListener(syncKeyboard);
    if (keyboardActive) {
      FocusManager.instance.removeEarlyKeyEventHandler(handleKey);
    }
    menu.dispose();
    trigger.dispose();
    menuFocus.dispose();
    super.dispose();
  }
}

/// A bounded searchable catalog for an anchored model popover.
class ModelPicker extends StatefulWidget {
  const ModelPicker({
    super.key,
    required this.controller,
    this.onSelect,
    this.onManage,
    this.onDismiss,
    this.onBack,
  });
  final DesktopController controller;
  final ValueChanged<ModelChoice>? onSelect;
  final VoidCallback? onManage;
  final VoidCallback? onDismiss;
  final VoidCallback? onBack;
  @override
  State<ModelPicker> createState() => _ModelPickerState();
}

class _ModelPickerState extends State<ModelPicker> {
  final search = TextEditingController();
  final searchFocus = FocusNode();
  final scroll = ScrollController();
  late Object ownerScope;
  Object? readError;
  bool loading = false;
  int highlighted = 0;
  Object get scope => _scope(widget.controller);
  bool get current => mounted && ownerScope == scope;

  @override
  void initState() {
    super.initState();
    ownerScope = scope;
    widget.controller.addListener(changed);
    FocusManager.instance.addEarlyKeyEventHandler(searchKey);
    if (widget.controller.availableModelCatalog == null &&
        widget.controller.connected) {
      unawaited(refresh());
    }
  }

  @override
  void didUpdateWidget(ModelPicker oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.controller, widget.controller)) {
      oldWidget.controller.removeListener(changed);
      widget.controller.addListener(changed);
    }
  }

  void changed() {
    if (!mounted) return;
    setState(() {});
  }

  Future<void> refresh() async {
    if (!current || loading || !widget.controller.connected) return;
    setState(() {
      loading = true;
      readError = null;
    });
    try {
      await widget.controller.refreshModels();
    } catch (error) {
      if (current) setState(() => readError = error);
    } finally {
      if (current) setState(() => loading = false);
    }
  }

  List<ModelChoice> get choices {
    final catalog = widget.controller.availableModelCatalog;
    final query = search.text.trim().toLowerCase();
    return catalog?.choices
            .where(
              (choice) =>
                  '${choice.name} ${choice.id} ${choice.provider} ${catalog.providerNames[choice.provider]}'
                      .toLowerCase()
                      .contains(query),
            )
            .toList() ??
        const [];
  }

  void choose(ModelChoice model) {
    if (!current || !widget.controller.connected || widget.controller.sending) {
      return;
    }
    if (widget.onSelect != null) {
      widget.onSelect!(model);
    } else {
      widget.onDismiss?.call();
      unawaited(
        widget.controller.run(() => widget.controller.chooseModel(model)),
      );
    }
  }

  KeyEventResult key(KeyEvent event) {
    if (event is! KeyDownEvent || !current) return KeyEventResult.ignored;
    if (event.logicalKey == LogicalKeyboardKey.escape) {
      widget.onDismiss?.call();
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.enter ||
        event.logicalKey == LogicalKeyboardKey.numpadEnter) {
      final list = choices;
      if (list.isNotEmpty) choose(list[highlighted.clamp(0, list.length - 1)]);
      return KeyEventResult.handled;
    }
    if (event.logicalKey != LogicalKeyboardKey.arrowDown &&
        event.logicalKey != LogicalKeyboardKey.arrowUp) {
      return KeyEventResult.ignored;
    }
    final list = choices;
    if (list.isEmpty) return KeyEventResult.handled;
    setState(
      () => highlighted =
          (highlighted +
                  (event.logicalKey == LogicalKeyboardKey.arrowDown ? 1 : -1))
              .clamp(0, list.length - 1),
    );
    final extent =
        DshTokens.of(context).controlHeight(context) +
        MediaQuery.textScalerOf(context).scale(DshTypography.sizeCaption) *
            1.5 +
        8;
    if (scroll.hasClients) {
      scroll.animateTo(
        (highlighted * extent).clamp(0.0, scroll.position.maxScrollExtent),
        duration: const Duration(milliseconds: 100),
        curve: Curves.easeOut,
      );
    }
    return KeyEventResult.handled;
  }

  KeyEventResult searchKey(KeyEvent event) {
    if (!mounted ||
        !searchFocus.hasFocus ||
        ModalRoute.of(context)?.isCurrent == false) {
      return KeyEventResult.ignored;
    }
    return key(event);
  }

  @override
  Widget build(BuildContext context) {
    if (!current) return const SizedBox.shrink();
    final c = widget.controller;
    final catalog = c.availableModelCatalog;
    final list = choices;
    final colors = DshColors(context);
    final pending = c.pendingModelSelection;
    final pendingKey = pending == null
        ? null
        : '${pending['provider']}\u0000${pending['model']}';
    final scaler = MediaQuery.textScalerOf(context);
    final controlHeight = DshTokens.of(context).controlHeight(context);
    final rowHeight = math.max(
      54,
      scaler.scale(DshTypography.sizeBody) * 22 / DshTypography.sizeBody +
          scaler.scale(DshTypography.sizeCaption) * 1.5 +
          16,
    );
    final auxiliaryHeight = scaler.scale(DshTypography.sizeCaption) * 1.5;
    final notices =
        (readError != null ? auxiliaryHeight * 2 + controlHeight + 8 : 0) +
        (c.modelDefaultError != null
            ? auxiliaryHeight * 2 + controlHeight + 8
            : 0) +
        (c.modelSelectionUnconfirmed ? auxiliaryHeight * 2 : 0);
    final desiredHeight =
        controlHeight * 2 +
        12 +
        notices +
        (list.isEmpty ? 80 : list.length * rowHeight) +
        (widget.onManage != null ? controlHeight : 0);
    return DefaultTextStyle(
      style: DshTypography.body.copyWith(color: colors.text),
      child: SizedBox(
        key: const ValueKey('model-picker'),
        width: _panelWidth(context),
        height: math.min(_panelHeight(context), desiredHeight),
        child: Focus(
          onKeyEvent: (_, event) => key(event),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  if (widget.onBack != null)
                    DshIcon(
                      DshIcons.chevronLeft.data,
                      key: const ValueKey('model-menu-back'),
                      label: '返回模型菜单',
                      onPressed: widget.onBack,
                    ),
                  Expanded(
                    child: Text(
                      '选择模型',
                      style: DshTypography.body.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                  DshIcon(
                    DshIcons.refresh.data,
                    label: '刷新模型列表',
                    onPressed: loading || !c.connected
                        ? null
                        : () => unawaited(refresh()),
                  ),
                ],
              ),
              const SizedBox(height: 6),
              DshField(
                key: const ValueKey('model-picker-search'),
                controller: search,
                focusNode: searchFocus,
                hint: DshConversationZh.searchModelsHint,
                prefix: DshIcons.search.data,
                autofocus: true,
                onChanged: (_) => setState(() => highlighted = 0),
                onSubmitted: (_) {
                  if (list.isNotEmpty) {
                    choose(list[highlighted.clamp(0, list.length - 1)]);
                  }
                },
              ),
              const SizedBox(height: 6),
              if (readError != null)
                Wrap(
                  spacing: 6,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    DshTooltip(
                      message: '$readError',
                      child: Text(
                        '无法读取模型列表：$readError',
                        key: const ValueKey('model-catalog-error'),
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: DshTypography.caption.copyWith(
                          color: colors.error,
                        ),
                      ),
                    ),
                    DshButton(
                      key: const ValueKey('model-catalog-retry'),
                      onPressed: loading ? null : () => unawaited(refresh()),
                      child: const Text('重试'),
                    ),
                  ],
                ),
              Expanded(
                child: catalog == null && (loading || c.refreshingModels)
                    ? const Center(
                        child: Text(
                          '正在读取模型列表…',
                          key: ValueKey('model-catalog-loading'),
                        ),
                      )
                    : list.isEmpty
                    ? Center(
                        child: Text(
                          search.text.trim().isEmpty
                              ? '暂无可选模型，请配置连接'
                              : '没有找到匹配的模型',
                          style: DshTypography.auxiliary.copyWith(
                            color: colors.muted,
                          ),
                        ),
                      )
                    : ListView.builder(
                        key: const ValueKey('model-picker-list'),
                        controller: scroll,
                        itemCount: list.length,
                        itemBuilder: (context, index) {
                          final model = list[index];
                          final selected = model.key == catalog?.currentKey;
                          final requested = model.key == pendingKey;
                          return Material(
                            color: colors.base,
                            child: ListTile(
                              key: ValueKey('model-choice-${model.key}'),
                              dense: true,
                              selected: selected,
                              selectedTileColor: colors.selected,
                              selectedColor: colors.text,
                              tileColor:
                                  index == highlighted && searchFocus.hasFocus
                                  ? colors.hover
                                  : null,
                              title: Text(
                                model.name,
                                style: DshTypography.body,
                              ),
                              subtitle: Text(
                                '${catalog?.providerNames[model.provider] ?? model.provider} · ${model.id}',
                                style: DshTypography.caption,
                              ),
                              trailing: requested
                                  ? DshGlyph(
                                      DshIcons.clock.data,
                                      key: ValueKey(
                                        'model-pending-${model.key}',
                                      ),
                                      size: 16,
                                      color: colors.blue,
                                    )
                                  : selected
                                  ? DshGlyph(
                                      DshIcons.check.data,
                                      key: ValueKey(
                                        'model-selected-${model.key}',
                                      ),
                                      size: 16,
                                      color: colors.blue,
                                    )
                                  : null,
                              onTap: c.connected && !c.sending
                                  ? () => choose(model)
                                  : null,
                            ),
                          );
                        },
                      ),
              ),
              _DefaultNotice(controller: c),
              if (c.modelSelectionUnconfirmed)
                Text(
                  '模型状态待确认，请刷新后重试',
                  style: DshTypography.caption.copyWith(color: colors.warning),
                ),
              if (widget.onManage != null)
                Align(
                  alignment: Alignment.centerRight,
                  child: DshButton(
                    key: const ValueKey('model-picker-manage'),
                    onPressed: widget.onManage,
                    child: const Text(DshConversationZh.manageModels),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  @override
  void dispose() {
    widget.controller.removeListener(changed);
    FocusManager.instance.removeEarlyKeyEventHandler(searchKey);
    search.dispose();
    searchFocus.dispose();
    scroll.dispose();
    super.dispose();
  }
}

class _DefaultNotice extends StatelessWidget {
  const _DefaultNotice({required this.controller});
  final DesktopController controller;
  @override
  Widget build(BuildContext context) {
    final error = controller.modelDefaultError;
    if (error == null) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Wrap(
        spacing: 6,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          DshTooltip(
            message: error,
            child: Text(
              error,
              key: const ValueKey('model-default-error'),
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: DshTypography.caption.copyWith(
                color: DshColors(context).muted,
              ),
            ),
          ),
          DshButton(
            key: const ValueKey('model-default-retry'),
            onPressed: controller.savingModelDefault
                ? null
                : () => unawaited(controller.run(controller.retryModelDefault)),
            child: Text(controller.savingModelDefault ? '保存中…' : '重试保存'),
          ),
        ],
      ),
    );
  }
}
