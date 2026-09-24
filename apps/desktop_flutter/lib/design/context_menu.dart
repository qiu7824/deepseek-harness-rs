import 'package:flutter/material.dart';

Future<String?> nativeContextMenu(
  BuildContext context,
  Offset position,
  Map<String, String> actions,
) {
  final overlay = Overlay.of(context).context.findRenderObject()! as RenderBox;
  final point = overlay.globalToLocal(position);
  return showMenu<String>(
    context: context,
    position: RelativeRect.fromRect(
      Rect.fromLTWH(point.dx, point.dy, 0, 0),
      Offset.zero & overlay.size,
    ),
    items: [
      for (final action in actions.entries)
        PopupMenuItem(
          value: action.key,
          height: 34,
          child: Text(action.value, style: const TextStyle(fontSize: 14)),
        ),
    ],
  );
}
