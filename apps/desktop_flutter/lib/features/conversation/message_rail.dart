import 'dart:math' as math;

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';

class MessageRailEntry {
  const MessageRailEntry(this.seq, this.text, this.images);
  final int seq, images;
  final String text;
  String get label => text.isEmpty ? '（仅图片）' : text;
}

String railSnippet(String text, [int length = 200]) {
  var end = math.min(text.length, 1024);
  if (end < text.length &&
      end > 0 &&
      text.codeUnitAt(end - 1) >= 0xd800 &&
      text.codeUnitAt(end - 1) <= 0xdbff) {
    end--;
  }
  final prefix = text.substring(0, end);
  final runes = prefix.replaceAll(RegExp(r'\s+'), ' ').trim().runes;
  final chars = runes.take(length + 1).toList();
  return '${String.fromCharCodes(chars.take(length))}${chars.length > length || text.length > 1024 ? '…' : ''}';
}

List<MessageRailEntry> messageRailEntries(
  Object? projection,
  List<TranscriptItem> transcript,
) {
  final entries = <int, MessageRailEntry>{};
  if (projection is List) {
    for (final value in projection) {
      if (value is! Map) continue;
      final seq = value['seq'];
      if (seq is! int || seq < 0) continue;
      entries[seq] = MessageRailEntry(
        seq,
        railSnippet('${value['text'] ?? ''}'),
        value['images'] is num
            ? (value['images'] as num).toInt().clamp(0, 100000)
            : 0,
      );
    }
  }
  // A live message can arrive before its index projection.
  for (final item in transcript) {
    if (item.kind == 'user' && item.seq != null) {
      entries.putIfAbsent(
        item.seq!,
        () => MessageRailEntry(
          item.seq!,
          railSnippet(item.clipboardText),
          item.images.length,
        ),
      );
    }
  }
  return entries.values.toList()..sort((a, b) => a.seq.compareTo(b.seq));
}

class UserMessageRail extends StatefulWidget {
  const UserMessageRail({
    super.key,
    required this.entries,
    required this.onActivate,
    required this.focusNode,
    required this.current,
    this.error,
  });
  final List<MessageRailEntry> entries;
  final Future<void> Function(MessageRailEntry) onActivate;
  final FocusNode focusNode;
  final ValueNotifier<int?> current;
  final String? error;
  static int capacity(double height) =>
      math.max(1, ((height - 26) / 10).floor() + 1);
  @override
  State<UserMessageRail> createState() => _UserMessageRailState();
}

class _UserMessageRailState extends State<UserMessageRail> {
  int start = -1, previousCapacity = 0;
  int? active;
  bool revealCurrent = true;
  @override
  void initState() {
    super.initState();
    widget.current.addListener(currentChanged);
  }

  void currentChanged() {
    if (active == null && !widget.focusNode.hasFocus) {
      setState(() => revealCurrent = true);
    }
  }

  @override
  void dispose() {
    widget.current.removeListener(currentChanged);
    super.dispose();
  }

  @override
  void didUpdateWidget(UserMessageRail old) {
    super.didUpdateWidget(old);
    if (old.current != widget.current) {
      old.current.removeListener(currentChanged);
      widget.current.addListener(currentChanged);
      revealCurrent = true;
    }
    if (old.entries.length != widget.entries.length) {
      start = -1;
      active = null;
      revealCurrent = true;
    }
  }

  void page(int direction, int count) => setState(() {
    start = (start + direction * math.max(1, count - 1)).toInt().clamp(
      0,
      math.max(0, widget.entries.length - count),
    );
    active = null;
  });

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, box) {
      if (widget.entries.isEmpty || box.maxHeight <= 0) {
        return const SizedBox.shrink();
      }
      final colors = DshColors(context),
          count = UserMessageRail.capacity(box.maxHeight);
      if (start < 0 || previousCapacity != count) {
        start = math.max(0, widget.entries.length - count);
      }
      previousCapacity = count;
      start = start.clamp(0, math.max(0, widget.entries.length - count));
      if (revealCurrent) {
        revealCurrent = false;
        final index = widget.entries.indexWhere(
          (entry) => entry.seq == widget.current.value,
        );
        if (index >= 0 && (index < start || index >= start + count)) {
          start = (index - count ~/ 2).clamp(
            0,
            math.max(0, widget.entries.length - count),
          );
        }
      }
      final entries = widget.entries.skip(start).take(count).toList();
      final top = (box.maxHeight - entries.length * 10) / 2;
      final narrow = MediaQuery.sizeOf(context).width <= 767;
      return ValueListenableBuilder<int?>(
        valueListenable: widget.current,
        builder: (context, current, _) {
          final currentIndex = entries.indexWhere((e) => e.seq == current);
          final selected = active ?? (currentIndex < 0 ? null : currentIndex);
          final preview =
              widget.error ??
              (active != null && active! < entries.length
                  ? '${entries[active!].label}${entries[active!].images > 0 ? '\n含图片 ${entries[active!].images} 张' : ''}'
                  : '');
          final previewStyle = DshTypography.body.copyWith(
            color: Colors.white,
            fontSize: 13,
            height: 20 / 13,
          );
          var previewWidth = 0.0, previewHeight = 0.0;
          if (preview.isNotEmpty) {
            final painter =
                TextPainter(
                  text: TextSpan(text: preview, style: previewStyle),
                  textDirection: Directionality.of(context),
                  textScaler: MediaQuery.textScalerOf(context),
                  maxLines: 7,
                )..layout(
                  maxWidth: math.min(
                    320,
                    math.max(80, box.maxWidth - (narrow ? 36 : 54) - 28),
                  ),
                );
            previewWidth = painter.width + 20;
            previewHeight = painter.height + 16;
            painter.dispose();
          }
          return Stack(
            clipBehavior: Clip.none,
            children: [
              Positioned(
                left: narrow ? 6 : 20,
                top: top,
                width: 26,
                height: entries.length * 10,
                child: Semantics(
                  label:
                      '你说过的话：${widget.entries.length} 条，当前 ${start + 1}-${start + entries.length}',
                  child: Focus(
                    focusNode: widget.focusNode,
                    onFocusChange: (focused) {
                      if (!focused) {
                        setState(() => active = null);
                      } else if (active == null) {
                        setState(
                          () => active = currentIndex >= 0 ? currentIndex : 0,
                        );
                      }
                    },
                    onKeyEvent: (_, event) {
                      if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
                        return KeyEventResult.ignored;
                      }
                      final key = event.logicalKey;
                      if (key == LogicalKeyboardKey.pageUp ||
                          key == LogicalKeyboardKey.pageDown) {
                        page(key == LogicalKeyboardKey.pageUp ? -1 : 1, count);
                      } else if (key == LogicalKeyboardKey.arrowDown ||
                          key == LogicalKeyboardKey.arrowUp) {
                        setState(
                          () => active =
                              ((active ?? 0) +
                                      (key == LogicalKeyboardKey.arrowUp
                                          ? -1
                                          : 1))
                                  .clamp(0, entries.length - 1),
                        );
                      } else if (key == LogicalKeyboardKey.enter ||
                          key == LogicalKeyboardKey.space) {
                        widget.onActivate(entries[active ?? 0]);
                      } else if (key == LogicalKeyboardKey.home ||
                          key == LogicalKeyboardKey.end) {
                        setState(() {
                          start = key == LogicalKeyboardKey.home
                              ? 0
                              : math.max(0, widget.entries.length - count);
                          active = null;
                        });
                      } else {
                        return KeyEventResult.ignored;
                      }
                      return KeyEventResult.handled;
                    },
                    child: Listener(
                      onPointerSignal: (event) {
                        if (event is PointerScrollEvent) {
                          GestureBinding.instance.pointerSignalResolver
                              .register(
                                event,
                                (_) => page(
                                  event.scrollDelta.dy < 0 ? -1 : 1,
                                  count,
                                ),
                              );
                        }
                      },
                      child: Column(
                        children: [
                          for (var i = 0; i < entries.length; i++)
                            Semantics(
                              button: true,
                              label:
                                  '跳到你说的话：${railSnippet(entries[i].label, 64)}',
                              selected: entries[i].seq == current,
                              child: MouseRegion(
                                cursor: SystemMouseCursors.click,
                                onEnter: (_) => setState(() => active = i),
                                onExit: (_) => setState(() => active = null),
                                child: GestureDetector(
                                  behavior: HitTestBehavior.opaque,
                                  key: ValueKey(
                                    'message-rail-${entries[i].seq}',
                                  ),
                                  onTap: () {
                                    widget.focusNode.requestFocus();
                                    setState(() => active = i);
                                    widget.onActivate(entries[i]);
                                  },
                                  child: SizedBox(
                                    height: 10,
                                    width: 26,
                                    child: Align(
                                      alignment: Alignment.centerLeft,
                                      child: AnimatedContainer(
                                        duration: const Duration(
                                          milliseconds: 120,
                                        ),
                                        height: 2,
                                        width: active == null
                                            ? 12
                                            : math
                                                  .max(
                                                    12,
                                                    26 -
                                                        (active! - i).abs() * 6,
                                                  )
                                                  .toDouble(),
                                        decoration: BoxDecoration(
                                          borderRadius: BorderRadius.circular(
                                            1,
                                          ),
                                          color:
                                              (selected == i
                                                      ? colors.text
                                                      : colors.muted)
                                                  .withValues(
                                                    alpha: active == null
                                                        ? (selected == i
                                                              ? .8
                                                              : .35)
                                                        : (selected == i
                                                              ? 1
                                                              : .5),
                                                  ),
                                        ),
                                      ),
                                    ),
                                  ),
                                ),
                              ),
                            ),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
              if (active != null && active! < entries.length ||
                  widget.error != null)
                Positioned(
                  left: narrow ? 36 : 54,
                  top: (top + (active ?? 0) * 10 + 4).clamp(
                    8,
                    math.max(8, box.maxHeight - previewHeight - 8),
                  ),
                  width: previewWidth,
                  child: IgnorePointer(
                    child: Material(
                      color: const Color(0xff2c2c2e),
                      elevation: 6,
                      borderRadius: BorderRadius.circular(8),
                      child: Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 10,
                          vertical: 8,
                        ),
                        child: Text(
                          preview,
                          maxLines: 7,
                          overflow: TextOverflow.clip,
                          style: previewStyle,
                        ),
                      ),
                    ),
                  ),
                ),
            ],
          );
        },
      );
    },
  );
}
