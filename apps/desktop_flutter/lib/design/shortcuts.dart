import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../src/controller.dart';
import 'primitives.dart';

const shortcutNames = {
  'sidebar': '展开／收起侧边栏',
  'new': '新建会话',
  'search': '搜索会话',
  'settings': '设置',
  'composer': '聚焦消息输入框',
  'workbench': '展开／收起工作台',
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
    for (final entry in defaultShortcuts.entries)
      entry.key: decodeShortcut(
        stored is Map ? stored[entry.key] : null,
        entry.value,
      ),
  };
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
  String? capturing, error;
  bool saving = false;
  String query = '';

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
        !(commandShortcuts && keyboard.isMetaPressed)) {
      setState(
        () => error = commandShortcuts
            ? '请使用 Cmd、Ctrl 或 Option 组合键，避免影响文字输入。'
            : '请使用 Ctrl 或 Alt 组合键，避免影响文字输入。',
      );
      return KeyEventResult.handled;
    }
    if ((!commandShortcuts && keyboard.isMetaPressed) ||
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
      setState(() => error = '此组合键用于系统或文字编辑，请选择其他组合。');
      return KeyEventResult.handled;
    }
    if ((key == LogicalKeyboardKey.enter ||
            key == LogicalKeyboardKey.numpadEnter) &&
        (keyboard.isControlPressed || keyboard.isMetaPressed) &&
        !keyboard.isAltPressed) {
      setState(() => error = '此组合键用于消息发送或换行，请选择其他组合。');
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
      setState(() => error = '此快捷键已绑定其他操作。');
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
        setState(() => error = '保存失败，原快捷键仍然有效：$exception');
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
      title: const Text('快捷键', style: TextStyle(fontSize: 17)),
      content: SizedBox(
        width: 470,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            DshField(
              key: const Key('shortcut-search'),
              hint: '搜索操作或组合键',
              onChanged: (value) => setState(() {
                query = value;
                capturing = null;
                error = null;
              }),
            ),
            const SizedBox(height: 12),
            const Text(
              '点击组合键后按下新组合；Esc 取消绑定。终端内优先使用终端快捷键。',
              style: TextStyle(fontSize: 12, height: 1.6),
            ),
            const SizedBox(height: 12),
            if (visibleShortcuts.isEmpty)
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 16),
                child: Text('未找到匹配的快捷键'),
              ),
            for (final entry in visibleShortcuts)
              Padding(
                key: ValueKey('shortcut-row-${entry.key}'),
                padding: const EdgeInsets.symmetric(vertical: 5),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        entry.value,
                        style: const TextStyle(fontSize: 14),
                      ),
                    ),
                    DshButton(
                      outline: true,
                      width: 150,
                      onPressed: saving
                          ? null
                          : () => setState(() {
                              capturing = entry.key;
                              error = null;
                            }),
                      child: SizedBox(
                        width: 126,
                        child: Text(
                          capturing == entry.key
                              ? '按下组合键…'
                              : shortcutLabel(bindings[entry.key]!),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            const SizedBox(height: 10),
            Text(
              'Enter 发送 · Shift+Enter 换行\n忙碌时 $primaryShortcutLabel+Enter 使用另一发送行为；空闲时可续写编号列表。',
              style: const TextStyle(fontSize: 12, height: 1.6),
            ),
            if (error != null)
              Text(
                error!,
                style: const TextStyle(color: Colors.red, fontSize: 12),
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
          child: const Text('恢复默认'),
        ),
        DshButton(
          onPressed: saving ? null : () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        DshButton(
          primary: true,
          onPressed: capturing != null || saving ? null : save,
          child: Text(saving ? '保存中…' : '保存'),
        ),
      ],
    ),
  );
}
