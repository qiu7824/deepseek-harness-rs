import 'package:flutter/material.dart';
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
const defaultShortcuts = {
  'sidebar': SingleActivator(LogicalKeyboardKey.keyB, control: true),
  'new': SingleActivator(LogicalKeyboardKey.keyN, control: true),
  'search': SingleActivator(LogicalKeyboardKey.keyK, control: true),
  'settings': SingleActivator(LogicalKeyboardKey.comma, control: true),
  'composer': SingleActivator(LogicalKeyboardKey.keyL, control: true),
  'workbench': SingleActivator(
    LogicalKeyboardKey.keyJ,
    control: true,
    shift: true,
  ),
};

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
  if (value.alt) 'Alt',
  if (value.shift) 'Shift',
  if (value.meta) 'Win',
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
  KeyEventResult capture(FocusNode node, KeyEvent event) {
    if (capturing == null || event is! KeyDownEvent) {
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
    if (!keyboard.isControlPressed && !keyboard.isAltPressed) {
      setState(() => error = '请使用 Ctrl 或 Alt 组合键，避免影响文字输入。');
      return KeyEventResult.handled;
    }
    if (keyboard.isMetaPressed ||
        (keyboard.isControlPressed &&
            !keyboard.isShiftPressed &&
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
    final binding = SingleActivator(
      key,
      control: keyboard.isControlPressed,
      alt: keyboard.isAltPressed,
      shift: keyboard.isShiftPressed,
    );
    if (bindings.entries.any(
      (e) =>
          e.key != capturing &&
          shortcutLabel(e.value) == shortcutLabel(binding),
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

  @override
  Widget build(BuildContext context) => Focus(
    onKeyEvent: capture,
    autofocus: true,
    child: AlertDialog(
      title: const Text('快捷键', style: TextStyle(fontSize: 17)),
      content: SizedBox(
        width: 470,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              '点击组合键后按下新组合；Esc 取消绑定。终端内优先使用终端快捷键。',
              style: TextStyle(fontSize: 12, height: 1.6),
            ),
            const SizedBox(height: 12),
            for (final entry in shortcutNames.entries)
              Padding(
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
                      onPressed: () => setState(() {
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
            const Text(
              'Enter 发送 · Shift+Enter 换行\n忙碌时 Ctrl+Enter 使用另一发送行为；空闲时可续写编号列表。',
              style: TextStyle(fontSize: 12, height: 1.6),
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
          onPressed: () => setState(() {
            bindings
              ..clear()
              ..addAll(defaultShortcuts);
            capturing = null;
            error = null;
          }),
          child: const Text('恢复默认'),
        ),
        DshButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('取消'),
        ),
        DshButton(
          primary: true,
          onPressed: capturing != null
              ? null
              : () async {
                  widget.controller.preferences.layout['shortcuts'] = {
                    for (final entry in bindings.entries)
                      entry.key: encodeShortcut(entry.value),
                  };
                  await widget.controller.run(
                    widget.controller.preferences.save,
                  );
                  widget.controller.emit();
                  if (context.mounted) Navigator.pop(context);
                },
          child: const Text('保存'),
        ),
      ],
    ),
  );
}
