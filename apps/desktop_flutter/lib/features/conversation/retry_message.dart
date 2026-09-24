import 'dart:async';
import 'dart:convert';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';

class RetryMessage extends StatefulWidget {
  const RetryMessage({super.key, required this.item});
  final TranscriptItem item;
  @override
  State<RetryMessage> createState() => _RetryMessageState();
}

class _RetryMessageState extends State<RetryMessage> {
  bool expanded = false;
  Timer? timer;
  Json data = {};
  int remaining = 1;
  void update() {
    timer?.cancel();
    data = object(jsonDecode(widget.item.output));
    remaining = (((data['delayMs'] as num?) ?? 0) / 1000).ceil().clamp(
      1,
      86400,
    );
    if (widget.item.status == 'scheduled' && remaining > 1) {
      timer = Timer.periodic(const Duration(seconds: 1), (timer) {
        setState(() => remaining--);
        if (remaining <= 1) timer.cancel();
      });
    }
  }

  @override
  void initState() {
    super.initState();
    update();
  }

  @override
  void didUpdateWidget(RetryMessage old) {
    super.didUpdateWidget(old);
    if (old.item.output != widget.item.output ||
        old.item.status != widget.item.status) {
      update();
    }
  }

  @override
  void dispose() {
    timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final label = switch (widget.item.status) {
      'started' => '已重试模型请求',
      'cancelled' => '已取消模型重试',
      _ => '正在重试模型请求',
    };
    final failure = object(data['failure']);
    return SizedBox(
      width: double.infinity,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          InkWell(
            onTap: () => setState(() => expanded = !expanded),
            child: SizedBox(
              height: 24,
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Flexible(
                    child: Text(
                      '$label（${data['retry']}/${data['maximum']}） · ${remaining}s',
                      style: TextStyle(
                        fontSize: 13,
                        height: 20 / 13,
                        color: colors.muted,
                      ),
                    ),
                  ),
                  const SizedBox(width: 7),
                  DshGlyph(
                    expanded
                        ? LucideIcons.chevronDown
                        : LucideIcons.chevronRight,
                    size: 12,
                    color: colors.muted,
                  ),
                ],
              ),
            ),
          ),
          if (expanded)
            Padding(
              padding: const EdgeInsets.only(left: 14, top: 3),
              child: SelectableText(
                '重试延迟：${data['delayMs']}ms\n${failure['code'] ?? ''}\n${failure['message'] ?? ''}',
                style: TextStyle(
                  fontSize: 12,
                  height: 18 / 12,
                  color: colors.muted,
                ),
              ),
            ),
        ],
      ),
    );
  }
}
