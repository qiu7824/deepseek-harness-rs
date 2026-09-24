import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../src/controller.dart';
import '../../design/primitives.dart';

String turnActivityLabel(DesktopController c) {
  if (!c.connected) return '连接中断，正在恢复状态';
  if (c.interactions.any((f) => f.type == 'approval/requested')) return '等待审批';
  if (c.interactions.any((f) => f.type == 'question/requested')) {
    return '等待你的回答';
  }
  if (c.sending) return '正在发送';
  if (c.compacting) return '正在压缩上下文';
  if (c.commandRunning) return '正在执行命令';
  if (!c.running) return '';
  final tool = c.transcript
      .where((m) => m.kind == 'tool' && m.status == 'pending')
      .lastOrNull;
  if (tool != null) return '正在执行工具 · ${tool.title}';
  final tail = c.transcript.where((m) => m.streaming).lastOrNull;
  if (tail?.kind == 'reasoning') return '正在思考';
  if (tail?.kind == 'assistant') return '正在生成回复';
  final chunk = c.window.events
      .where((event) => event.type == 'assistant/chunk')
      .lastOrNull;
  if ([
    'tool-call-start',
    'tool-call-delta',
  ].contains(object(chunk?.data['chunk'])['type'])) {
    return '正在生成工具参数';
  }
  final phase = object(
    object(c.projections['sessionStats'])['requestPhase'],
  )['phase'];
  return const {
        'credentials': '正在准备模型认证',
        'request_sent': '正在等待模型响应',
        'response_headers': '正在接收模型响应',
        'attachment_prepare': '正在准备附件',
        'attachment_upload': '正在上传附件',
      }[phase] ??
      '正在继续处理';
}

class TurnActivity extends StatefulWidget {
  const TurnActivity({super.key, required this.controller});
  final DesktopController controller;
  @override
  State<TurnActivity> createState() => _TurnActivityState();
}

class _TurnActivityState extends State<TurnActivity>
    with WidgetsBindingObserver {
  Timer? timer;
  int ticks = 0;
  final started = DateTime.now();
  bool foreground = true;
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  void syncTimer() {
    timer?.cancel();
    if (!foreground) return;
    timer = Timer.periodic(
      Duration(
        milliseconds: MediaQuery.disableAnimationsOf(context) ? 1000 : 350,
      ),
      (_) {
        if (mounted) setState(() => ticks++);
      },
    );
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    foreground = state == AppLifecycleState.resumed;
    syncTimer();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    syncTimer();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([
      widget.controller,
      widget.controller.projectionChanges,
      widget.controller.messageChanges,
    ]),
    builder: (_, _) {
      final label = turnActivityLabel(widget.controller),
          color = DshColors(context).muted;
      if (label.isEmpty) return const SizedBox.shrink();
      final stamp = widget.controller.window.events
          .where((event) => event.type == 'turn/start')
          .lastOrNull
          ?.raw['time'];
      final anchor =
          stamp is num &&
              stamp > 0 &&
              stamp <= DateTime.now().millisecondsSinceEpoch
          ? DateTime.fromMillisecondsSinceEpoch(stamp.toInt())
          : started;
      final seconds = DateTime.now().difference(anchor).inSeconds;
      return Semantics(
        liveRegion: true,
        label: label,
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 8),
          child: Row(
            children: [
              DshGlyph(
                null,
                asset: label == '正在思考'
                    ? 'assets/icons/web-IconThinkOutline14.svg'
                    : 'assets/icons/web-IconApiOutline14.svg',
                size: 14,
                color: color,
              ),
              const SizedBox(width: 8),
              Flexible(
                child: Text(
                  label,
                  style: TextStyle(fontSize: 14, color: color),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              ExcludeSemantics(
                child: Text(
                  MediaQuery.disableAnimationsOf(context)
                      ? '…'
                      : '.' * (ticks % 3 + 1),
                  style: TextStyle(color: color),
                ),
              ),
              if (seconds >= 5)
                Text(
                  '  ${seconds}s',
                  style: TextStyle(fontSize: 12, color: color),
                ),
            ],
          ),
        ),
      );
    },
  );
}
