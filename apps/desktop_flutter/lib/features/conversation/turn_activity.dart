import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../../src/controller.dart';
import '../../design/primitives.dart';

import 'package:dsh_desktop/design/typography.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

String turnActivityLabel(DesktopController c) {
  if (!c.connected) return DshConversationZh.restoringConnection;
  if (c.interactions.any((f) => f.type == 'approval/requested')) {
    return DshConversationZh.awaitingApproval;
  }
  if (c.interactions.any((f) => f.type == 'question/requested')) {
    return DshConversationZh.awaitingUserAnswer;
  }
  if (c.sending) return DshConversationZh.sending;
  if (c.compacting) return DshConversationZh.compactingContext;
  if (c.commandRunning) return DshConversationZh.executingCommand;
  if (!c.running) return '';
  final tool = c.transcript
      .where((m) => m.kind == 'tool' && m.status == 'pending')
      .lastOrNull;
  if (tool != null) return DshConversationZh.executingTool(title: tool.title);
  final tail = c.transcript.where((m) => m.streaming).lastOrNull;
  if (tail?.kind == 'reasoning') return DshConversationZh.thinkingProgress;
  if (tail?.kind == 'assistant') return DshConversationZh.generatingAnswer;
  final chunk = c.window.events
      .where((event) => event.type == 'assistant/chunk')
      .lastOrNull;
  if ([
    'tool-call-start',
    'tool-call-delta',
  ].contains(object(chunk?.data['chunk'])['type'])) {
    return DshConversationZh.generatingToolArguments;
  }
  final phase = object(
    object(c.projections['sessionStats'])['requestPhase'],
  )['phase'];
  return const {
        'credentials': DshConversationZh.preparingModelAuthentication,
        'request_sent': DshConversationZh.awaitingModelResponse,
        'response_headers': DshConversationZh.receivingModelResponse,
        'attachment_prepare': DshConversationZh.preparingAttachments,
        'attachment_upload': DshConversationZh.uploadingAttachments,
      }[phase] ??
      DshConversationZh.continuingWork;
}

class TurnActivity extends StatefulWidget {
  const TurnActivity({
    super.key,
    required this.controller,
    this.readingHistory = false,
  });
  final DesktopController controller;
  final bool readingHistory;
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
    if (!foreground || widget.readingHistory) return;
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
  void didUpdateWidget(TurnActivity oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.readingHistory != widget.readingHistory) syncTimer();
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
      final label = widget.readingHistory
              ? DshConversationZh.readingHistory
              : turnActivityLabel(widget.controller),
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
                asset: label == DshConversationZh.thinkingProgress
                    ? 'assets/icons/web-IconThinkOutline14.svg'
                    : 'assets/icons/web-IconApiOutline14.svg',
                size: 14,
                color: color,
              ),
              const SizedBox(width: 8),
              Flexible(
                child: Text(
                  label,
                  style: TextStyle(
                    fontSize: DshTypography.sizeBody,
                    color: color,
                  ),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              ExcludeSemantics(
                child: Text(
                  widget.readingHistory
                      ? ''
                      : MediaQuery.disableAnimationsOf(context)
                      ? '…'
                      : '.' * (ticks % 3 + 1),
                  style: TextStyle(color: color),
                ),
              ),
              if (seconds >= 5 && !widget.readingHistory)
                Text(
                  '  ${seconds}s',
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: color,
                  ),
                ),
            ],
          ),
        ),
      );
    },
  );
}
