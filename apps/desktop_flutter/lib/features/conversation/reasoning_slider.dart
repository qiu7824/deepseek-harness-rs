import 'dart:math' as math;

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';

import 'package:dsh_desktop/l10n/conversation_zh.dart';

/// Same level names as the Web model menu (`effort.level.*`).
const reasoningLevelNames = {
  'none': DshConversationZh.reasoningNone,
  'off': DshConversationZh.close,
  'minimal': DshConversationZh.reasoningMinimal,
  'low': DshConversationZh.reasoningLow,
  'medium': DshConversationZh.reasoningMedium,
  'high': DshConversationZh.reasoningHigh,
  'xhigh': DshConversationZh.reasoningExtraHigh,
  'max': DshConversationZh.reasoningMaximum,
};

const _aliases = {
  'none': ['none'],
  'off': ['off', 'disabled'],
  'minimal': ['minimal'],
  'low': ['low'],
  'medium': ['medium'],
  'high': ['high'],
  'xhigh': ['xhigh', 'extrahigh'],
  'max': ['max', 'maximum'],
};

/// Localized label for one provider-declared effort; a custom name that is
/// not one of the standard levels is shown as declared.
String reasoningLevelLabel(Json effort) {
  final id = '${effort['id'] ?? ''}';
  final name = '${effort['name'] ?? id}';
  final normalized = name.toLowerCase().replaceAll(RegExp(r'[ -]'), '');
  return (_aliases[id]?.contains(normalized) ?? false)
      ? reasoningLevelNames[id]!
      : name;
}

/// Finite reasoning options. Confirmed and pending choices stay distinct;
/// a later click may replace an earlier pending intent in the controller.
/// The legacy name is retained for callers of the original slider widget.
class ReasoningSlider extends StatefulWidget {
  const ReasoningSlider({
    super.key,
    required this.levels,
    required this.value,
    required this.onChanged,
    this.enabled = true,
    this.showHeading = true,
    this.scope,
    this.pendingValue,
  });

  final List<Json> levels;
  final String? value;
  final Future<void> Function(String id) onChanged;
  final bool enabled;
  final bool showHeading;

  /// Model, conversation and Host identity supplied by the owner.
  final Object? scope;

  /// Latest requested level, without claiming that it has been applied.
  final String? pendingValue;

  @override
  State<ReasoningSlider> createState() => _ReasoningSliderState();
}

class _ReasoningSliderState extends State<ReasoningSlider> {
  String? localPending;
  String? failure;
  int revision = 0;

  String identity(List<Json> levels) =>
      levels.map((level) => level['id']).join('\u0000');

  @override
  void didUpdateWidget(ReasoningSlider oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.scope != widget.scope ||
        identity(oldWidget.levels) != identity(widget.levels)) {
      revision++;
      localPending = null;
      failure = null;
    } else if (oldWidget.value != widget.value) {
      failure = null;
      if (localPending == widget.value) {
        localPending = null;
      } else if (localPending != null && widget.pendingValue != localPending) {
        revision++;
        localPending = null;
      }
    }
    if (oldWidget.pendingValue != widget.pendingValue &&
        widget.pendingValue != null &&
        localPending != null &&
        widget.pendingValue != localPending) {
      revision++;
      localPending = null;
      failure = null;
    }
  }

  String? get pending => widget.pendingValue ?? localPending;

  String labelFor(String id) => reasoningLevelLabel(
    widget.levels.where((level) => '${level['id']}' == id).firstOrNull ??
        {'id': id, 'name': id},
  );

  Future<void> commit(String id) async {
    if (!widget.enabled ||
        !widget.levels.any((level) => '${level['id']}' == id) ||
        id == pending ||
        id == widget.value && pending == null) {
      return;
    }
    final action = ++revision;
    final scope = widget.scope;
    final levels = identity(widget.levels);
    setState(() {
      localPending = id;
      failure = null;
    });
    try {
      await widget.onChanged(id);
    } catch (error) {
      if (mounted &&
          action == revision &&
          scope == widget.scope &&
          levels == identity(widget.levels) &&
          widget.value != id &&
          (widget.pendingValue == null || widget.pendingValue == id)) {
        setState(() => failure = '无法更新思考等级：$error');
      }
    } finally {
      if (mounted && action == revision && scope == widget.scope) {
        setState(() => localPending = null);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final levels = widget.levels;
    final selected = levels.indexWhere(
      (level) => '${level['id']}' == widget.value,
    );
    final current = selected >= 0
        ? reasoningLevelLabel(levels[selected])
        : widget.value == null
        ? '模型默认'
        : '${reasoningLevelNames[widget.value] ?? widget.value}（未列出）';
    final pendingId = pending;
    return Column(
      key: const Key('reasoning-slider'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (widget.showHeading)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Wrap(
              spacing: 12,
              runSpacing: 4,
              children: [
                Text(
                  DshConversationZh.reasoningStrength,
                  style: DshTypography.auxiliary.copyWith(color: colors.muted),
                ),
                Text(
                  current,
                  key: const ValueKey('reasoning-confirmed-value'),
                  style: DshTypography.auxiliary.copyWith(
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
            ),
          )
        else if (selected < 0 && levels.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Text(
              current,
              key: const ValueKey('reasoning-confirmed-value'),
              style: DshTypography.auxiliary.copyWith(color: colors.muted),
            ),
          ),
        if (levels.isEmpty)
          Text(
            '当前模型未提供可调思考等级',
            key: const ValueKey('reasoning-unavailable'),
            style: DshTypography.auxiliary.copyWith(color: colors.muted),
          )
        else
          LayoutBuilder(
            builder: (context, constraints) {
              final lineHeight =
                  MediaQuery.textScalerOf(context)
                      .scale(DshTypography.sizeAuxiliary) *
                  20 /
                  DshTypography.sizeAuxiliary;
              final minimumHeight = math.max(36.0, lineHeight + 14);
              return FocusTraversalGroup(
                policy: WidgetOrderTraversalPolicy(),
                child: Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    for (final level in levels)
                      _ReasoningOption(
                        id: '${level['id']}',
                        label: reasoningLevelLabel(level),
                        description: level['description'] as String?,
                        selected: '${level['id']}' == widget.value,
                        pending: '${level['id']}' == pendingId,
                        enabled: widget.enabled,
                        maxWidth: constraints.maxWidth,
                        minimumHeight: minimumHeight,
                        lineHeight: lineHeight,
                        onPressed: () => commit('${level['id']}'),
                      ),
                  ],
                ),
              );
            },
          ),
        if (pendingId != null)
          Semantics(
            liveRegion: true,
            child: Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                '正在切换：${labelFor(pendingId)}',
                key: const ValueKey('reasoning-pending-value'),
                style: DshTypography.caption.copyWith(color: colors.muted),
              ),
            ),
          ),
        if (failure != null)
          Semantics(
            liveRegion: true,
            child: Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                failure!,
                key: const ValueKey('reasoning-update-error'),
                style: DshTypography.caption.copyWith(color: colors.error),
              ),
            ),
          ),
      ],
    );
  }
}

class _ReasoningOption extends StatelessWidget {
  const _ReasoningOption({
    required this.id,
    required this.label,
    required this.description,
    required this.selected,
    required this.pending,
    required this.enabled,
    required this.maxWidth,
    required this.minimumHeight,
    required this.lineHeight,
    required this.onPressed,
  });

  final String id;
  final String label;
  final String? description;
  final bool selected;
  final bool pending;
  final bool enabled;
  final double maxWidth;
  final double minimumHeight;
  final double lineHeight;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final radius = BorderRadius.circular(8);
    final hint = description?.trim();
    return Semantics(
      button: true,
      selected: selected,
      enabled: enabled,
      label: '$label${pending ? '，待确认' : ''}',
      onTap: enabled ? onPressed : null,
      child: DshTooltip(
        message: hint == null || hint.isEmpty ? label : '$label\n$hint',
        excludeFromSemantics: true,
        child: Material(
          key: ValueKey('reasoning-level-$id'),
          color: selected ? colors.selected : colors.base,
          shape: RoundedRectangleBorder(
            borderRadius: radius,
            side: BorderSide(
              color: selected || pending ? colors.blue : colors.border,
            ),
          ),
          child: InkWell(
            borderRadius: radius,
            onTap: enabled ? onPressed : null,
            excludeFromSemantics: true,
            hoverColor: colors.hover,
            child: ConstrainedBox(
              constraints: BoxConstraints(
                maxWidth: maxWidth,
                minHeight: minimumHeight,
              ),
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: 10,
                  vertical: 7,
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    SizedBox(
                      width: 14,
                      height: lineHeight,
                      child: pending
                          ? DshGlyph(
                              DshIcons.loaderCircle.data,
                              key: ValueKey('reasoning-pending-$id'),
                              size: 14,
                              color: colors.blue,
                            )
                          : selected
                          ? DshGlyph(
                              DshIcons.check.data,
                              key: ValueKey('reasoning-selected-$id'),
                              size: 14,
                              color: enabled ? colors.blue : colors.muted,
                            )
                          : null,
                    ),
                    const SizedBox(width: 6),
                    Flexible(
                      child: Text(
                        label,
                        softWrap: true,
                        style: DshTypography.auxiliary.copyWith(
                          fontWeight: selected ? FontWeight.w600 : null,
                          color: enabled ? colors.text : colors.muted,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
