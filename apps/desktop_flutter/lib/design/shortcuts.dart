import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../src/controller.dart';
import '../l10n/zh.dart';
import 'error.dart';
import 'primitives.dart';

import 'package:dsh_desktop/design/typography.dart';

final shortcutNames = {
  'sidebar': DshShortcutZh.toggleSidebar,
  'new': DshZh.newSession,
  'search': DshZh.searchCommands,
  'settings': DshShellZh.settings,
  'composer': DshShortcutZh.focusComposer,
  'workbench': DshShortcutZh.toggleWorkbench,
  'stop': DshZh.stopExecution,
  'focus-next': DshShortcutZh.focusNext,
  'focus-previous': DshShortcutZh.focusPrevious,
  'cycle-next': DshShortcutZh.cycleNext,
  'cycle-previous': DshShortcutZh.cyclePrevious,
  for (var index = 1; index <= 9; index++)
    'session-$index': DshZh.visibleSession(index),
};
bool get commandShortcuts => defaultTargetPlatform == TargetPlatform.macOS;
String get primaryShortcutLabel => commandShortcuts ? 'Cmd' : 'Ctrl';
bool get primaryShortcutPressed => commandShortcuts
    ? HardwareKeyboard.instance.isMetaPressed
    : HardwareKeyboard.instance.isControlPressed;

Map<String, SingleActivator> get defaultShortcuts {
  SingleActivator primary(LogicalKeyboardKey key, {bool shift = false}) =>
      SingleActivator(
        key,
        control: !commandShortcuts,
        meta: commandShortcuts,
        shift: shift,
      );
  return {
    'sidebar': primary(LogicalKeyboardKey.keyB),
    'new': primary(LogicalKeyboardKey.keyN),
    'search': primary(LogicalKeyboardKey.keyK),
    'settings': primary(LogicalKeyboardKey.comma),
    'composer': primary(LogicalKeyboardKey.keyL),
    'workbench': primary(LogicalKeyboardKey.keyJ, shift: true),
    'stop': primary(LogicalKeyboardKey.period),
    'focus-next': const SingleActivator(LogicalKeyboardKey.f6),
    'focus-previous': const SingleActivator(LogicalKeyboardKey.f6, shift: true),
    'cycle-next': const SingleActivator(LogicalKeyboardKey.tab, control: true),
    'cycle-previous': const SingleActivator(
      LogicalKeyboardKey.tab,
      control: true,
      shift: true,
    ),
    for (final (index, key) in [
      LogicalKeyboardKey.digit1,
      LogicalKeyboardKey.digit2,
      LogicalKeyboardKey.digit3,
      LogicalKeyboardKey.digit4,
      LogicalKeyboardKey.digit5,
      LogicalKeyboardKey.digit6,
      LogicalKeyboardKey.digit7,
      LogicalKeyboardKey.digit8,
      LogicalKeyboardKey.digit9,
    ].indexed)
      'session-${index + 1}': primary(key),
  };
}

Map<String, dynamic> encodeShortcut(SingleActivator binding) => {
  'key': binding.trigger.keyId,
  'control': binding.control,
  'alt': binding.alt,
  'shift': binding.shift,
  'meta': binding.meta,
};
SingleActivator decodeShortcut(dynamic value, SingleActivator fallback) =>
    value is Map && value['key'] is int
    ? SingleActivator(
        LogicalKeyboardKey(value['key'] as int),
        control: value['control'] == true,
        alt: value['alt'] == true,
        shift: value['shift'] == true,
        meta: value['meta'] == true,
      )
    : fallback;
Map<String, SingleActivator> configuredShortcuts(DesktopController c) {
  final stored = c.preferences.layout['shortcuts'];
  return {
    // Explicit bindings precede new defaults so upgrades never steal a saved key.
    if (stored is Map)
      for (final entry in defaultShortcuts.entries)
        if (stored.containsKey(entry.key))
          entry.key: decodeShortcut(stored[entry.key], entry.value),
    for (final entry in defaultShortcuts.entries)
      if (stored is! Map || !stored.containsKey(entry.key))
        entry.key: decodeShortcut(
          stored is Map ? stored[entry.key] : null,
          entry.value,
        ),
  };
}

bool sameShortcut(SingleActivator a, SingleActivator b) =>
    a.trigger == b.trigger &&
    a.control == b.control &&
    a.alt == b.alt &&
    a.shift == b.shift &&
    a.meta == b.meta;

String? shortcutConflict(Map<String, SingleActivator> bindings) {
  final entries = bindings.entries.toList();
  for (var i = 0; i < entries.length; i++) {
    for (var j = i + 1; j < entries.length; j++) {
      if (sameShortcut(entries[i].value, entries[j].value)) {
        return DshShortcutZh.conflict(
          first: shortcutNames[entries[i].key]!,
          second: shortcutNames[entries[j].key]!,
        );
      }
    }
  }
  return null;
}

String shortcutLabel(SingleActivator value) => [
  if (value.control) 'Ctrl',
  if (value.alt) commandShortcuts ? 'Option' : 'Alt',
  if (value.shift) 'Shift',
  if (value.meta) commandShortcuts ? 'Cmd' : 'Win',
  value.trigger.keyLabel,
].join('+');

class ShortcutEditor extends StatefulWidget {
  const ShortcutEditor({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<ShortcutEditor> createState() => _ShortcutEditorState();
}

class _ShortcutEditorState extends State<ShortcutEditor> {
  late final bindings = configuredShortcuts(widget.controller);
  final bindingsScroll = ScrollController();
  String? capturing, error;
  bool saving = false;
  String query = '';

  @override
  void dispose() {
    bindingsScroll.dispose();
    super.dispose();
  }

  @override
  void didUpdateWidget(ShortcutEditor oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      bindings
        ..clear()
        ..addAll(configuredShortcuts(widget.controller));
      capturing = null;
      error = null;
      saving = false;
    }
  }

  Iterable<MapEntry<String, String>> get visibleShortcuts {
    final filter = query.toLowerCase().replaceAll(RegExp(r'\s+'), '');
    return shortcutNames.entries.where((entry) {
      final searchable =
          '${entry.value} ${entry.key} ${shortcutLabel(bindings[entry.key]!)}'
              .toLowerCase()
              .replaceAll(RegExp(r'\s+'), '');
      return searchable.contains(filter);
    });
  }

  KeyEventResult capture(FocusNode node, KeyEvent event) {
    if (saving || capturing == null || event is! KeyDownEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      setState(() => capturing = null);
      return KeyEventResult.handled;
    }
    if ([
      LogicalKeyboardKey.controlLeft,
      LogicalKeyboardKey.controlRight,
      LogicalKeyboardKey.shiftLeft,
      LogicalKeyboardKey.shiftRight,
      LogicalKeyboardKey.altLeft,
      LogicalKeyboardKey.altRight,
      LogicalKeyboardKey.metaLeft,
      LogicalKeyboardKey.metaRight,
    ].contains(key)) {
      return KeyEventResult.handled;
    }
    final keyboard = HardwareKeyboard.instance;
    if (!keyboard.isControlPressed &&
        !keyboard.isAltPressed &&
        !(commandShortcuts && keyboard.isMetaPressed) &&
        key != LogicalKeyboardKey.f6) {
      setState(
        () => error = commandShortcuts
            ? DshShortcutZh.macModifierRequired
            : DshShortcutZh.modifierRequired,
      );
      return KeyEventResult.handled;
    }
    if ((!commandShortcuts && keyboard.isMetaPressed) ||
        (keyboard.isAltPressed &&
            [
              LogicalKeyboardKey.tab,
              LogicalKeyboardKey.f4,
              LogicalKeyboardKey.escape,
            ].contains(key)) ||
        (commandShortcuts &&
            keyboard.isMetaPressed &&
            [
              LogicalKeyboardKey.tab,
              LogicalKeyboardKey.space,
              LogicalKeyboardKey.keyQ,
              LogicalKeyboardKey.keyH,
              LogicalKeyboardKey.keyM,
            ].contains(key)) ||
        ((keyboard.isControlPressed || keyboard.isMetaPressed) &&
            !keyboard.isAltPressed &&
            [
              LogicalKeyboardKey.keyC,
              LogicalKeyboardKey.keyV,
              LogicalKeyboardKey.keyX,
              LogicalKeyboardKey.keyZ,
              LogicalKeyboardKey.keyY,
              LogicalKeyboardKey.keyA,
            ].contains(key))) {
      setState(() => error = DshShortcutZh.reservedKey);
      return KeyEventResult.handled;
    }
    if ((key == LogicalKeyboardKey.enter ||
            key == LogicalKeyboardKey.numpadEnter) &&
        (keyboard.isControlPressed || keyboard.isMetaPressed) &&
        !keyboard.isAltPressed) {
      setState(() => error = DshShortcutZh.sendKeyReserved);
      return KeyEventResult.handled;
    }
    final binding = SingleActivator(
      key,
      control: keyboard.isControlPressed,
      alt: keyboard.isAltPressed,
      shift: keyboard.isShiftPressed,
      meta: keyboard.isMetaPressed,
    );
    if (bindings.entries.any(
      (e) =>
          e.key != capturing &&
          e.value.trigger == binding.trigger &&
          e.value.control == binding.control &&
          e.value.alt == binding.alt &&
          e.value.shift == binding.shift &&
          e.value.meta == binding.meta,
    )) {
      setState(() => error = DshShortcutZh.duplicateKey);
      return KeyEventResult.handled;
    }
    setState(() {
      bindings[capturing!] = binding;
      capturing = null;
      error = null;
    });
    return KeyEventResult.handled;
  }

  Future<void> save() async {
    if (saving || capturing != null) return;
    final conflict = shortcutConflict(bindings);
    if (conflict != null) {
      setState(() => error = conflict);
      return;
    }
    final controller = widget.controller;
    final next = {
      for (final entry in bindings.entries)
        entry.key: encodeShortcut(entry.value),
    };
    setState(() {
      saving = true;
      error = null;
    });
    try {
      await controller.preferences.saveLayoutValue('shortcuts', next);
      if (mounted && widget.controller == controller) {
        controller.emit();
        Navigator.pop(context);
      }
    } catch (exception) {
      if (mounted && widget.controller == controller) {
        setState(
          () => error = DshShortcutZh.saveFailure(
            detail: DshError.describe(exception).message,
          ),
        );
      }
    } finally {
      if (mounted && widget.controller == controller) {
        setState(() => saving = false);
      }
    }
  }

  @override
  Widget build(BuildContext context) => Focus(
    onKeyEvent: capture,
    autofocus: true,
    child: AlertDialog(
      scrollable: true,
      title: const Text(
        DshShortcutZh.title,
        style: TextStyle(fontSize: DshTypography.sizeSectionTitle),
      ),
      content: SizedBox(
        width: 470,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            DshField(
              key: const Key('shortcut-search'),
              hint: DshShortcutZh.search,
              onChanged: (value) => setState(() {
                query = value;
                capturing = null;
                error = null;
                if (bindingsScroll.hasClients) bindingsScroll.jumpTo(0);
              }),
            ),
            const SizedBox(height: 12),
            const Text(
              DshShortcutZh.captureHint,
              style: TextStyle(
                fontSize: DshTypography.sizeCaption,
                height: 1.6,
              ),
            ),
            const SizedBox(height: 12),
            if (error != null) DshErrorView(error: error!),
            SizedBox(
              height: (MediaQuery.sizeOf(context).height * .4).clamp(
                160.0,
                340.0,
              ),
              child: Scrollbar(
                controller: bindingsScroll,
                thumbVisibility: true,
                child: SingleChildScrollView(
                  controller: bindingsScroll,
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (visibleShortcuts.isEmpty)
                        const Padding(
                          padding: EdgeInsets.symmetric(vertical: 16),
                          child: Text(DshShortcutZh.noResults),
                        ),
                      for (final entry in visibleShortcuts)
                        Padding(
                          key: ValueKey('shortcut-row-${entry.key}'),
                          padding: const EdgeInsets.symmetric(vertical: 5),
                          child: LayoutBuilder(
                            builder: (context, constraints) {
                              final stacked =
                                  constraints.maxWidth < 360 ||
                                  MediaQuery.textScalerOf(context).scale(1) >
                                      1.5;
                              final label = Text(
                                entry.value,
                                style: const TextStyle(
                                  fontSize: DshTypography.sizeBody,
                                ),
                              );
                              final bindingText = capturing == entry.key
                                  ? DshShortcutZh.capturing
                                  : shortcutLabel(bindings[entry.key]!);
                              final bindingWidth =
                                  // Horizontal padding plus the outline border.
                                  (stacked ? constraints.maxWidth : 150.0) - 26;
                              var bindingHeight = 36.0;
                              if (stacked) {
                                final painter = TextPainter(
                                  text: TextSpan(
                                    text: bindingText,
                                    style: DshTypography.body,
                                  ),
                                  textDirection: Directionality.of(context),
                                  textScaler: MediaQuery.textScalerOf(context),
                                )..layout(maxWidth: bindingWidth);
                                bindingHeight = painter.height + 14;
                                painter.dispose();
                              }
                              final button = Tooltip(
                                message: shortcutLabel(bindings[entry.key]!),
                                child: DshButton(
                                  outline: true,
                                  width: stacked ? constraints.maxWidth : 150,
                                  height: bindingHeight,
                                  onPressed: saving
                                      ? null
                                      : () => setState(() {
                                          capturing = entry.key;
                                          error = null;
                                        }),
                                  child: SizedBox(
                                    width: bindingWidth,
                                    child: Text(
                                      bindingText,
                                      maxLines: stacked ? null : 1,
                                      overflow: stacked
                                          ? TextOverflow.visible
                                          : TextOverflow.ellipsis,
                                    ),
                                  ),
                                ),
                              );
                              return stacked
                                  ? Column(
                                      crossAxisAlignment:
                                          CrossAxisAlignment.start,
                                      children: [
                                        label,
                                        const SizedBox(height: 8),
                                        button,
                                      ],
                                    )
                                  : Row(
                                      children: [
                                        Expanded(child: label),
                                        button,
                                      ],
                                    );
                            },
                          ),
                        ),
                    ],
                  ),
                ),
              ),
            ),
            const SizedBox(height: 10),
            Text(
              DshShortcutZh.sendHint(modifier: primaryShortcutLabel),
              style: const TextStyle(
                fontSize: DshTypography.sizeCaption,
                height: 1.6,
              ),
            ),
          ],
        ),
      ),
      actions: [
        DshButton(
          onPressed: saving
              ? null
              : () => setState(() {
                  bindings
                    ..clear()
                    ..addAll(defaultShortcuts);
                  capturing = null;
                  error = null;
                }),
          child: const Text(DshShortcutZh.restoreDefaults),
        ),
        DshButton(
          onPressed: saving ? null : () => Navigator.pop(context),
          child: const Text(DshZh.cancel),
        ),
        DshButton(
          primary: true,
          onPressed: capturing != null || saving ? null : save,
          child: Text(saving ? DshZh.saving : DshZh.save),
        ),
      ],
    ),
  );
}
