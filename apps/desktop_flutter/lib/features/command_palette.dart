import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../design/primitives.dart';
import '../l10n/zh.dart';

import 'package:dsh_desktop/design/typography.dart';

class DshCommand {
  const DshCommand({
    required this.id,
    required this.group,
    required this.title,
    required this.icon,
    required this.onInvoke,
    this.subtitle = '',
    this.searchTerms = '',
    this.shortcut,
    this.enabled = true,
  });
  final String id, group, title, subtitle, searchTerms;
  final String? shortcut;
  final IconData icon;
  final VoidCallback onInvoke;
  final bool enabled;

  bool matches(String query) => '$title $subtitle $searchTerms'
      .toLowerCase()
      .contains(query.trim().toLowerCase());
}

/// Virtualized, grouped search. Returning the command before invoking it keeps
/// new dialogs outside the palette route and restores the calling focus.
class DshCommandPalette extends StatefulWidget {
  const DshCommandPalette({super.key, required this.commands});
  final List<DshCommand> commands;
  @override
  State<DshCommandPalette> createState() => _DshCommandPaletteState();
}

class _DshCommandPaletteState extends State<DshCommandPalette> {
  final query = TextEditingController();
  final focus = FocusNode();
  final scroll = ScrollController();
  int selected = 0;
  List<DshCommand> get results =>
      widget.commands.where((c) => c.matches(query.text)).toList();
  double get rowHeight =>
      72 * MediaQuery.textScalerOf(context).scale(1).clamp(1, 2);

  @override
  void dispose() {
    query.dispose();
    focus.dispose();
    scroll.dispose();
    super.dispose();
  }

  void move(int direction) {
    final rows = results;
    if (rows.isEmpty) return;
    var next = selected;
    for (var i = 0; i < rows.length; i++) {
      next = (next + direction) % rows.length;
      if (rows[next].enabled) break;
    }
    setState(() => selected = next);
    if (scroll.hasClients) {
      final top = selected * rowHeight;
      final bottom = top + rowHeight;
      final viewport = scroll.position.viewportDimension;
      if (top < scroll.offset || bottom > scroll.offset + viewport) {
        scroll.jumpTo(
          (top < scroll.offset ? top : bottom - viewport).clamp(
            0,
            scroll.position.maxScrollExtent,
          ),
        );
      }
    }
  }

  KeyEventResult onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent ||
        (!query.value.composing.isCollapsed && query.value.composing.isValid)) {
      return KeyEventResult.ignored;
    }
    switch (event.logicalKey) {
      case LogicalKeyboardKey.arrowDown:
        move(1);
      case LogicalKeyboardKey.arrowUp:
        move(-1);
      case LogicalKeyboardKey.escape:
        Navigator.pop(context);
      case LogicalKeyboardKey.enter || LogicalKeyboardKey.numpadEnter:
        final rows = results;
        if (rows.isNotEmpty && rows[selected].enabled) {
          Navigator.pop(context, rows[selected]);
        }
      default:
        return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final rows = results;
    final colors = DshColors(context);
    return Dialog(
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxWidth: 680,
          maxHeight: MediaQuery.sizeOf(context).height * .76,
        ),
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Focus(
            onKeyEvent: onKey,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                DshField(
                  key: const Key('command-palette-query'),
                  controller: query,
                  focusNode: focus,
                  autofocus: true,
                  hint: DshZh.searchCommands,
                  onChanged: (_) => setState(() {
                    selected = results
                        .indexWhere((c) => c.enabled)
                        .clamp(0, results.length);
                    if (scroll.hasClients) scroll.jumpTo(0);
                  }),
                ),
                const SizedBox(height: 8),
                Flexible(
                  child: rows.isEmpty
                      ? Padding(
                          padding: const EdgeInsets.all(24),
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              const Text(DshZh.noMatchingCommands),
                              DshButton(
                                onPressed: () => setState(() {
                                  query.clear();
                                  selected = 0;
                                }),
                                child: const Text(DshZh.clearFilter),
                              ),
                            ],
                          ),
                        )
                      : ListView.builder(
                          controller: scroll,
                          shrinkWrap: true,
                          itemExtent: rowHeight,
                          itemCount: rows.length,
                          itemBuilder: (context, index) {
                            final row = rows[index];
                            return Semantics(
                              selected: index == selected,
                              enabled: row.enabled,
                              button: true,
                              child: Material(
                                color: index == selected
                                    ? colors.hover
                                    : Colors.transparent,
                                borderRadius: BorderRadius.circular(8),
                                child: InkWell(
                                  key: ValueKey('command-${row.id}'),
                                  borderRadius: BorderRadius.circular(8),
                                  onTap: row.enabled
                                      ? () => Navigator.pop(context, row)
                                      : null,
                                  child: Padding(
                                    padding: const EdgeInsets.symmetric(
                                      horizontal: 12,
                                      vertical: 6,
                                    ),
                                    child: Row(
                                      children: [
                                        DshGlyph(
                                          row.icon,
                                          size: 16,
                                          color: row.enabled
                                              ? null
                                              : colors.muted,
                                        ),
                                        const SizedBox(width: 12),
                                        Expanded(
                                          child: Column(
                                            crossAxisAlignment:
                                                CrossAxisAlignment.start,
                                            mainAxisAlignment:
                                                MainAxisAlignment.center,
                                            children: [
                                              Text(
                                                row.title,
                                                maxLines: 1,
                                                overflow: TextOverflow.ellipsis,
                                                style: TextStyle(
                                                  fontSize:
                                                      DshTypography.sizeBody,
                                                  color: row.enabled
                                                      ? null
                                                      : colors.muted,
                                                ),
                                              ),
                                              Text(
                                                '${row.group}${row.subtitle.isEmpty ? '' : ' · ${row.subtitle}'}',
                                                maxLines: 1,
                                                overflow: TextOverflow.ellipsis,
                                                style: TextStyle(
                                                  fontSize:
                                                      DshTypography.sizeCaption,
                                                  color: colors.muted,
                                                ),
                                              ),
                                            ],
                                          ),
                                        ),
                                        if (row.shortcut != null) ...[
                                          const SizedBox(width: 8),
                                          Text(
                                            row.shortcut!,
                                            style: TextStyle(
                                              fontSize:
                                                  DshTypography.sizeCaption,
                                              color: colors.muted,
                                            ),
                                          ),
                                        ],
                                      ],
                                    ),
                                  ),
                                ),
                              ),
                            );
                          },
                        ),
                ),
                const SizedBox(height: 8),
                Text(
                  DshShortcutZh.paletteHint(count: rows.length),
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
