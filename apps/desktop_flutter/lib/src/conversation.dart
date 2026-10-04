import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter/rendering.dart' show ScrollCacheExtent;
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:desktop_drop/desktop_drop.dart';
import 'package:file_selector/file_selector.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:url_launcher/url_launcher.dart';

import '../design/primitives.dart';
import '../design/shortcuts.dart';
import '../design/typography.dart';
import '../design/bounded_image.dart';
import '../design/error.dart';
import '../design/motion.dart';
import '../l10n/zh.dart';
import 'reading_position.dart';
import '../design/rich_content.dart';
import '../design/text_document.dart';
import '../design/context_menu.dart';
import '../features/conversation/attachment_view.dart';
import '../features/conversation/session_status.dart';
import '../features/conversation/session_views.dart';
import '../features/conversation/artifacts_view.dart';
import '../features/workbench/workbench_panel.dart' show previewUrl;
import '../features/conversation/code_graph_view.dart';
import 'composer_attachments.dart';
import 'controller.dart';
import 'composer_clipboard.dart';
import 'interactions.dart';
import 'voice_input.dart';
import '../features/conversation/feedback_controller.dart';
import '../features/conversation/feedback_actions.dart';
import '../features/conversation/message_rail.dart';
import '../features/conversation/tool_message.dart';
import '../features/conversation/plan_mode_control.dart';
import '../features/conversation/permission_control.dart';
import '../features/conversation/streaming_presentation.dart';
import '../features/conversation/turn_activity.dart';
import '../features/conversation/read_aloud.dart';
import '../features/conversation/model_selector.dart';
export '../features/conversation/model_selector.dart'
    show ModelPicker, ModelSelectionControls, composerModelLabel;
import '../features/conversation/turn_stats.dart';
import 'resource_diagnostics.dart';
import '../features/conversation/retry_message.dart';

import 'package:dsh_desktop/l10n/conversation_zh.dart';

TextEditingValue? continueNumberedDraft(TextEditingValue value) {
  if (!value.selection.isValid) return null;
  final start = value.selection.start, end = value.selection.end;
  final lineStart = start == 0
      ? 0
      : value.text.lastIndexOf('\n', start - 1) + 1;
  final match = RegExp(r'^([ \t]*)(\d{1,9})([.)、．])([ \t]*)(.*)$')
      .firstMatch(value.text.substring(lineStart, start));
  if (match == null) return null;
  final insertion =
      '\n${match[1]}${int.parse(match[2]!) + 1}${match[3]}${match[4]!.isEmpty ? ' ' : match[4]}';
  return TextEditingValue(
    text: value.text.replaceRange(start, end, insertion),
    selection: TextSelection.collapsed(offset: start + insertion.length),
  );
}

const _messageRenderLimit = 96 * 1024;

String boundedMessageText(String value) {
  if (value.length <= _messageRenderLimit) return value;
  final end = TextDocument.boundary(value, _messageRenderLimit);
  return DshConversationZh.truncatedMessage(prefix: value.substring(0, end));
}

class Conversation extends StatefulWidget {
  const Conversation({
    super.key,
    required this.controller,
    this.maxContentWidth = 0,
    this.onOpenSettings,
    this.onOpenWorkbench,
    this.onOpenPath,
    this.onOpenPlan,
    this.onSelectWorkspace,
    this.viewRequest,
    this.headerInset = 28,
    this.workspaceAnchor,
  });
  final DesktopController controller;
  final double maxContentWidth;
  final double headerInset;
  final GlobalKey? workspaceAnchor;
  final VoidCallback? onOpenSettings, onSelectWorkspace;
  final void Function([String tab])? onOpenWorkbench;
  final ValueChanged<String>? onOpenPath;
  final ValueChanged<TranscriptItem>? onOpenPlan;
  final ValueNotifier<String>? viewRequest;
  @override
  State<Conversation> createState() => _ConversationState();
}

class _ConversationState extends State<Conversation>
    with WidgetsBindingObserver
    implements ResourceDiagnostics {
  DesktopController get c => widget.controller;
  final input = TextEditingController(), scroll = ScrollController();
  final focus = FocusNode();
  final railFocus = FocusNode();
  final railCurrent = ValueNotifier<int?>(null);
  final messageViewport = GlobalKey();
  final railViewport = GlobalKey();
  final userAnchors = <int, GlobalKey>{};
  final readingAnchors = <String, GlobalKey>{};
  bool _readingFramePending = false;
  int navigation = 0;
  bool navigating = false, railFramePending = false;
  String? navigationError;
  int railIndexRevision = -1;
  String railUserSignature = '';
  List<MessageRailEntry> railEntriesCache = [];
  final voice = VoiceInputController();
  final readAloud = ReadAloudController();
  MessageFeedbackController? feedback;
  bool feedbackConnected = false;
  final attachments = <PendingAttachment>[];
  final clipboard = ComposerClipboard();
  int attachmentEpoch = 0, importingAttachments = 0;
  @override
  Map<String, int> get resourceDiagnostics => {
    'conversationViews': 1,
    'speechActive': readAloud.activeId == null ? 0 : 1,
    'speechTextUnits': readAloud.retainedTextUnits,
    'pendingAttachmentBytes': attachments.fold<int>(
      0,
      (total, file) => total + file.data.length,
    ),
  };
  String? _session;
  DshClient? _composerClient;
  String? _composerDraftScope;
  int _composerSelection = -1;
  bool follow = true, dropping = false;
  int revision = 0, voiceRevision = 0;
  String view = 'conversation';
  Widget? _cachedLayout;
  ConversationReadingPosition? _pendingReadingPosition;
  bool _restoringScroll = false;

  void _selectWorkspace() => widget.onSelectWorkspace?.call();
  void _openPath(String path) => widget.onOpenPath?.call(path);
  void _openPlan(TranscriptItem item) => widget.onOpenPlan?.call(item);

  @override
  void setState(VoidCallback fn) {
    _cachedLayout = null;
    super.setState(fn);
  }

  void conversationChanged() {
    changed();
    if (mounted) setState(() {});
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _cachedLayout = null;
  }

  @override
  void didUpdateWidget(Conversation oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != c) {
      oldWidget.controller.conversationChanges.removeListener(
        conversationChanged,
      );
      oldWidget.controller.messageChanges.removeListener(changed);
      oldWidget.controller.composerFocus.removeListener(requestFocus);
      c.conversationChanges.addListener(conversationChanged);
      c.messageChanges.addListener(changed);
      c.composerFocus.addListener(requestFocus);
      _session = null;
      _composerClient = null;
      _composerDraftScope = null;
      _composerSelection = -1;
      changed();
      _cachedLayout = null;
    }
    if (oldWidget.viewRequest != widget.viewRequest) {
      oldWidget.viewRequest?.removeListener(applyViewRequest);
      widget.viewRequest?.addListener(applyViewRequest);
      _cachedLayout = null;
    }
    if (oldWidget.maxContentWidth != widget.maxContentWidth ||
        oldWidget.headerInset != widget.headerInset ||
        oldWidget.workspaceAnchor != widget.workspaceAnchor ||
        (oldWidget.onSelectWorkspace == null) !=
            (widget.onSelectWorkspace == null) ||
        (oldWidget.onOpenPath == null) != (widget.onOpenPath == null) ||
        (oldWidget.onOpenPlan == null) != (widget.onOpenPlan == null) ||
        (oldWidget.onOpenWorkbench == null) !=
            (widget.onOpenWorkbench == null)) {
      _cachedLayout = null;
    }
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    c.conversationChanges.addListener(conversationChanged);
    c.messageChanges.addListener(changed);
    c.composerFocus.addListener(requestFocus);
    widget.viewRequest?.addListener(applyViewRequest);
    scroll.addListener(onScroll);
    voice.addListener(voiceChanged);
    changed();
  }

  void requestFocus() {
    if (!mounted) return;
    if (view != 'conversation') {
      setState(() => view = 'conversation');
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) focus.requestFocus();
      });
    } else {
      focus.requestFocus();
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    c.conversationChanges.removeListener(conversationChanged);
    c.messageChanges.removeListener(changed);
    c.composerFocus.removeListener(requestFocus);
    widget.viewRequest?.removeListener(applyViewRequest);
    input.dispose();
    scroll.dispose();
    focus.dispose();
    railFocus.dispose();
    railCurrent.dispose();
    voice.removeListener(voiceChanged);
    voice.dispose();
    readAloud.dispose();
    feedback?.dispose();
    attachments.clear();
    super.dispose();
  }

  void applyViewRequest() {
    final request = widget.viewRequest?.value;
    if (!mounted || request == null || request.isEmpty) return;
    widget.viewRequest!.value = '';
    if (request == 'user-message-rail') {
      setState(() => view = 'conversation');
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) railFocus.requestFocus();
      });
      return;
    }
    setState(() => view = request);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state != AppLifecycleState.resumed) {
      voice.cancel();
      unawaited(readAloud.stop());
    }
  }

  void voiceChanged() {
    if (!mounted) return;
    final text = voice.latestText;
    if (voice.textRevision != voiceRevision) {
      voiceRevision = voice.textRevision;
      input.value = input.value.copyWith(
        text: text,
        selection: TextSelection.collapsed(offset: text.length),
        composing: TextRange.empty,
      );
      c.setDraft(text);
    }
  }

  void changed() {
    if (!c.pluginEnabled('dsh-voice-input') ||
        !c.connected ||
        c.sending ||
        c.interactions.any((frame) => frame.type == 'question/requested')) {
      voice.cancel(notify: false);
    }
    final api = c.client, id = c.selectedId;
    if (feedback?.api != api || feedback?.sessionId != id) {
      voice.cancel(notify: false);
      unawaited(readAloud.stop());
      feedback?.dispose();
      feedback = api != null && id != null
          ? MessageFeedbackController(api, id)
          : null;
      feedbackConnected = false;
    }
    feedback?.retainTargets(
      c.transcript
          .where((i) => i.kind == 'turn-tail')
          .map((i) => i.messageId)
          .whereType<String>(),
    );
    if (c.connected && !feedbackConnected && feedback != null) {
      unawaited(feedback!.ensure(refresh: true));
    }
    feedbackConnected = c.connected;
    if (view == 'project-tasks' || c.menuSettings[view] == false) {
      view = 'conversation';
    }
    if (_session != c.selectedId ||
        _composerClient != c.client ||
        (_session == null && _composerDraftScope != c.draftScopeKey) ||
        (_session == null && _composerSelection != c.selectionRevision)) {
      final adoptDraft =
          _session == null &&
          _composerClient == c.client &&
          c.selectionAdoptsDraft;
      if (!adoptDraft) {
        attachmentEpoch++;
        importingAttachments = 0;
      }
      navigation++;
      navigating = false;
      navigationError = null;
      userAnchors.clear();
      readingAnchors.clear();
      railEntriesCache = [];
      railIndexRevision = -1;
      railUserSignature = '';
      railCurrent.value = null;
      voice.cancel(notify: false);
      unawaited(readAloud.stop());
      view = 'conversation';
      _session = c.selectedId;
      _composerClient = c.client;
      _composerDraftScope = c.draftScopeKey;
      if (!adoptDraft || input.text.isEmpty) input.text = c.draft;
      if (!adoptDraft) attachments.clear();
      _pendingReadingPosition = c.readingPositions[_session];
      _restoringScroll = _pendingReadingPosition != null;
      follow = _pendingReadingPosition == null;
    }
    if (_composerSelection != c.selectionRevision) {
      navigation++;
      _pendingReadingPosition = c.readingPositions[_session];
      _restoringScroll = _pendingReadingPosition != null;
      follow = _pendingReadingPosition == null;
    }
    _composerSelection = c.selectionRevision;
    if (_restoringScroll && !c.readingPositions.containsKey(_session)) {
      _pendingReadingPosition = null;
      _restoringScroll = false;
      follow = true;
      navigation++;
    }
    final readingPosition = _pendingReadingPosition;
    if (readingPosition != null && !c.loading && c.transcript.isNotEmpty) {
      _pendingReadingPosition = null;
      unawaited(restoreReadingPosition(readingPosition));
    }
    final next = c.window.lastSeq ?? -1;
    if (follow && revision != next) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && scroll.hasClients) scroll.jumpTo(0);
      });
    }
    revision = next;
    final userSeqs = c.transcript
        .where((m) => m.kind == 'user')
        .map((m) => m.seq)
        .toSet();
    userAnchors.removeWhere((seq, _) => !userSeqs.contains(seq));
    final ids = c.transcript.map((item) => item.id).toSet();
    readingAnchors.removeWhere((id, _) => !ids.contains(id));
    updateRailHighlight();
  }

  Future<void> restoreReadingPosition(
    ConversationReadingPosition position,
  ) async {
    final owner = c, api = c.client, session = c.selectedId;
    final selection = c.selectionRevision, token = ++navigation;
    _restoringScroll = true;
    bool valid() =>
        mounted &&
        identical(c, owner) &&
        c.client == api &&
        c.selectedId == session &&
        c.selectionRevision == selection &&
        token == navigation;
    try {
      // The requested page starts at the anchor. Its reverse-list offset is
      // deliberately not reused: page length and following messages may change.
      for (var frame = 0; frame < 16; frame++) {
        await WidgetsBinding.instance.endOfFrame;
        if (!valid() || c.loading || !scroll.hasClients) return;
        final matching =
            c.transcript
                .where((item) => item.id == position.itemId)
                .firstOrNull ??
            c.transcript.where((item) => item.seq == position.seq).firstOrNull;
        final row = readingAnchors[matching?.id]?.currentContext
            ?.findRenderObject();
        final port = messageViewport.currentContext?.findRenderObject();
        if (row is RenderBox &&
            row.hasSize &&
            port is RenderBox &&
            port.hasSize) {
          final top =
              row.localToGlobal(Offset.zero).dy -
              port.localToGlobal(Offset.zero).dy;
          final desiredTop = position.viewportOffset.clamp(
            row.size.height > 24 ? -row.size.height + 24 : 0,
            port.size.height > 24 ? port.size.height - 24 : 0,
          );
          final offset = (scroll.offset + desiredTop - top).clamp(
            scroll.position.minScrollExtent,
            scroll.position.maxScrollExtent,
          );
          if ((offset - scroll.offset).abs() < .5) return;
          scroll.jumpTo(offset);
        } else {
          scroll.jumpTo(scroll.position.maxScrollExtent);
        }
      }
    } finally {
      if (valid()) _restoringScroll = false;
    }
  }

  void rememberVisibleReadingPosition() {
    if (_readingFramePending || !mounted) return;
    _readingFramePending = true;
    final owner = c, api = c.client, session = c.selectedId;
    final selection = c.selectionRevision;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _readingFramePending = false;
      if (!mounted ||
          !identical(owner, c) ||
          api != c.client ||
          session != c.selectedId ||
          selection != c.selectionRevision ||
          session == null ||
          follow ||
          navigating ||
          _restoringScroll ||
          c.loading) {
        return;
      }
      final port = messageViewport.currentContext?.findRenderObject();
      if (port is! RenderBox || !port.hasSize) return;
      final origin = port.localToGlobal(Offset.zero).dy;
      TranscriptItem? chosen;
      int? chosenSeq, precedingSeq = c.window.firstSeq;
      double? chosenTop;
      for (final item in c.transcript) {
        precedingSeq = item.seq ?? precedingSeq;
        if (precedingSeq == null) continue;
        final row = readingAnchors[item.id]?.currentContext?.findRenderObject();
        if (row is! RenderBox || !row.hasSize) continue;
        final top = row.localToGlobal(Offset.zero).dy - origin;
        if (top >= port.size.height || top + row.size.height < 24) continue;
        if (chosenTop == null || top < chosenTop) {
          chosen = item;
          chosenSeq = precedingSeq;
          chosenTop = top;
        }
      }
      if (chosen == null) return;
      c.rememberReadingPosition(
        session,
        seq: chosenSeq,
        itemId: chosen.id,
        viewportOffset: chosenTop!,
        follow: false,
      );
    });
  }

  List<MessageRailEntry> get railEntries {
    final revision = c.projectionWindow.revisionOf('userMessageRail');
    final signature = c.transcript
        .where((m) => m.kind == 'user' && m.seq != null)
        .map((m) => m.seq)
        .join(',');
    if (revision != railIndexRevision || signature != railUserSignature) {
      railEntriesCache = messageRailEntries(
        c.projections['userMessageRail'],
        c.transcript,
      );
      railIndexRevision = revision;
      railUserSignature = signature;
    }
    return railEntriesCache;
  }

  void updateRailHighlight() {
    if (railFramePending || !mounted) return;
    railFramePending = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      railFramePending = false;
      if (!mounted) return;
      final port =
          railViewport.currentContext?.findRenderObject() ??
          messageViewport.currentContext?.findRenderObject();
      if (port is! RenderBox || !port.hasSize) return;
      int? closest;
      var distance = double.infinity;
      for (final entry in userAnchors.entries) {
        final row = entry.value.currentContext?.findRenderObject();
        if (row is! RenderBox || !row.hasSize) continue;
        final center =
            row.localToGlobal(Offset(0, row.size.height / 2)).dy -
            port.localToGlobal(Offset.zero).dy;
        final delta = (center - port.size.height / 2).abs();
        if (delta < distance) {
          distance = delta;
          closest = entry.key;
        }
      }
      railCurrent.value = closest;
    });
  }

  Future<void> jumpMessage(MessageRailEntry entry) async {
    _pendingReadingPosition = null;
    _restoringScroll = false;
    final token = ++navigation, session = c.selectedId, api = c.client;
    bool valid() =>
        mounted &&
        token == navigation &&
        session == c.selectedId &&
        api == c.client;
    setState(() {
      follow = false;
      navigating = true;
      navigationError = null;
    });
    try {
      final existing = userAnchors[entry.seq]?.currentContext;
      if (existing != null && !c.loading) {
        c.holdHistory(entry.seq);
      } else {
        await c.loadHistory(
          after: entry.seq,
          targetSeq: entry.seq,
          force: true,
        );
      }
      if (!valid()) return;
      for (var frame = 0; frame < 12; frame++) {
        await WidgetsBinding.instance.endOfFrame;
        if (!mounted || !valid()) return;
        final target = userAnchors[entry.seq]?.currentContext;
        if (target != null && target.mounted) {
          final row = target.findRenderObject();
          final port =
              railViewport.currentContext?.findRenderObject() ??
              messageViewport.currentContext?.findRenderObject();
          if (row is! RenderBox ||
              port is! RenderBox ||
              !row.hasSize ||
              !port.hasSize ||
              !scroll.hasClients) {
            continue;
          }
          final desired = port
              .localToGlobal(Offset(0, port.size.height / 2))
              .dy;
          final center = row.localToGlobal(Offset(0, row.size.height / 2)).dy;
          final offset = (scroll.offset + desired - center).clamp(
            scroll.position.minScrollExtent,
            scroll.position.maxScrollExtent,
          );
          if (MediaQuery.disableAnimationsOf(context)) {
            scroll.jumpTo(offset);
          } else {
            await scroll.animateTo(
              offset,
              duration: DshMotion.duration(context, DshMotion.panel),
              curve: DshMotion.curve,
            );
          }
          if (valid()) railCurrent.value = entry.seq;
          return;
        }
        if (scroll.hasClients) scroll.jumpTo(scroll.position.maxScrollExtent);
      }
      throw StateError(DshConversationZh.messageNotFound);
    } catch (e) {
      if (valid()) setState(() => navigationError = '$e');
    } finally {
      if (valid()) {
        setState(() => navigating = false);
        rememberVisibleReadingPosition();
      }
    }
  }

  Future<void> returnToLatest() async {
    _pendingReadingPosition = null;
    _restoringScroll = false;
    railFocus.unfocus();
    final token = ++navigation, session = c.selectedId;
    setState(() {
      navigating = true;
      navigationError = null;
      follow = false;
    });
    try {
      await c.returnLatest();
      if (!mounted || token != navigation || session != c.selectedId) return;
      await WidgetsBinding.instance.endOfFrame;
      if (!mounted || token != navigation || session != c.selectedId) return;
      if (scroll.hasClients) scroll.jumpTo(0);
      follow = true;
      if (session != null) {
        c.rememberReadingPosition(
          session,
          seq: null,
          itemId: null,
          viewportOffset: 0,
          follow: true,
        );
      }
    } catch (e) {
      if (mounted && token == navigation) {
        navigationError = DshConversationZh.latestHistoryFailed(error: e);
      }
    } finally {
      if (mounted && token == navigation) setState(() => navigating = false);
    }
  }

  void onScroll() {
    if (!scroll.hasClients) return;
    if (_restoringScroll || _session != c.selectedId || c.loading) return;
    updateRailHighlight();
    if (navigating) return;
    if (scroll.offset < 60 &&
        c.holdingLiveHistory &&
        !c.window.hasAfter &&
        !c.loading) {
      unawaited(returnToLatest());
      return;
    }
    final nextFollow = scroll.offset < 60 && !c.readingHistory;
    if (nextFollow && c.selectedId != null) {
      c.rememberReadingPosition(
        c.selectedId!,
        seq: null,
        itemId: null,
        viewportOffset: 0,
        follow: true,
      );
    }
    if (follow != nextFollow) setState(() => follow = nextFollow);
    if (!nextFollow) rememberVisibleReadingPosition();
    if (!nextFollow && !c.readingHistory && !c.window.hasAfter) {
      c.holdLiveHistory();
    }
    if (scroll.position.extentAfter < 200 && c.window.hasBefore && !c.loading) {
      unawaited(
        c.run(() => c.loadHistory(before: c.window.firstSeq, merge: true)),
      );
    }
    if (scroll.offset < 80 && c.window.hasAfter && !c.loading) {
      unawaited(
        c.run(
          () => c.loadHistory(after: (c.window.lastSeq ?? -1) + 1, merge: true),
        ),
      );
    }
  }

  Future<void> send({bool steer = false}) async {
    if (c.sending || c.changingPlanMode || importingAttachments > 0) return;
    if (input.value.composing.isValid && !input.value.composing.isCollapsed) {
      return;
    }
    if (input.text.trim().isEmpty && attachments.isEmpty) return;
    final owner = c.client, selection = c.selectionRevision;
    if (c.readingHistory || c.window.hasAfter) await returnToLatest();
    if (!mounted ||
        c.client != owner ||
        c.selectionRevision != selection ||
        c.readingHistory ||
        c.window.hasAfter) {
      return;
    }
    voice.cancel(notify: false);
    final text = input.text;
    final submittedAttachments = attachments.toList();
    final parts = [
      for (final file in submittedAttachments)
        {
          'type': imageMediaTypes.contains(file.type) ? 'image' : 'file',
          'name': file.name,
          'mediaType': file.type,
          'data': base64Encode(file.data),
        },
    ];
    c.setDraft(text);
    follow = true;
    String? acceptedSession;
    await c.run(() async {
      acceptedSession = await c.sendParts(
        text,
        parts,
        mode: steer ? 'steer' : 'queue',
      );
    });
    if (!mounted) return;
    if (acceptedSession != null &&
        c.client == owner &&
        c.selectedId == acceptedSession) {
      if (input.text == text) input.clear();
      setState(() => attachments.removeWhere(submittedAttachments.contains));
    }
  }

  Future<void> addFiles(List<XFile> files) async {
    final epoch = attachmentEpoch, owner = c.client;
    bool valid() => mounted && epoch == attachmentEpoch && owner == c.client;
    if (files.isEmpty) return;
    setState(() => importingAttachments++);
    try {
      final pending = <({String name, Uint8List data, String type})>[];
      var pendingBytes = 0;
      for (final file in files) {
        if (!valid()) return;
        if (FileSystemEntity.typeSync(file.path) ==
            FileSystemEntityType.directory) {
          throw StateError(
            DshConversationZh.folderAttachmentRejected(name: file.name),
          );
        }
        if (attachments.length + pending.length >= 8) {
          throw StateError(DshConversationZh.attachmentCountLimit);
        }
        final length = await file.length();
        if (!valid()) return;
        final total = attachments.fold<int>(0, (n, f) => n + f.data.length);
        if (length + pendingBytes + total > 16 * 1024 * 1024) {
          throw StateError(DshConversationZh.attachmentBytesLimit);
        }
        final data = await file.readAsBytes();
        if (!valid()) return;
        if (data.length +
                pendingBytes +
                attachments.fold<int>(0, (n, f) => n + f.data.length) >
            16 * 1024 * 1024) {
          throw StateError(DshConversationZh.attachmentBytesLimit);
        }
        final type = attachmentMediaType(file.name, data);
        pending.add((name: file.name, data: data, type: type));
        pendingBytes += data.length;
      }
      if (!valid()) return;
      if (attachments.length + pending.length > 8) {
        throw StateError(DshConversationZh.attachmentCountLimit);
      }
      // Recheck after every asynchronous read: a concurrent drop/paste may
      // have filled the same composer while this batch was loading.
      if (pendingBytes + attachments.fold<int>(0, (n, f) => n + f.data.length) >
          16 * 1024 * 1024) {
        throw StateError(DshConversationZh.attachmentBytesLimit);
      }
      setState(() => attachments.addAll(pending));
    } catch (e) {
      if (valid()) {
        c.error = '$e';
        c.emit();
      }
    } finally {
      if (mounted && epoch == attachmentEpoch) {
        setState(() => importingAttachments--);
      }
    }
  }

  Future<void> pickFiles() async {
    final epoch = attachmentEpoch, owner = c.client;
    try {
      final files = await openFiles();
      if (mounted && epoch == attachmentEpoch && owner == c.client) {
        await addFiles(files);
      }
    } catch (e) {
      if (mounted && epoch == attachmentEpoch && owner == c.client) {
        c.error = '$e';
        c.emit();
      }
    }
  }

  Future<void> paste() async {
    final epoch = attachmentEpoch, owner = c.client;
    bool valid() => mounted && epoch == attachmentEpoch && owner == c.client;
    setState(() => importingAttachments++);
    try {
      final files = await clipboard.readFiles();
      if (!valid()) return;
      final text = await Clipboard.getData(Clipboard.kTextPlain);
      if (!valid()) return;
      final imageOnly =
          files?.length == 1 && files!.first.mimeType == 'image/png';
      if (files != null &&
          files.isNotEmpty &&
          (!imageOnly || text?.text?.isNotEmpty != true)) {
        await addFiles(files);
        return;
      }
      final data = text;
      if (!valid() || data?.text == null) return;
      final value = input.value;
      final selection = value.selection.isValid
          ? value.selection
          : TextSelection.collapsed(offset: value.text.length);
      voice.cancel();
      input.value = value.replaced(selection, data!.text!);
      c.setDraft(input.text);
    } catch (e) {
      if (valid()) {
        c.error = '$e';
        c.emit();
      }
    } finally {
      if (mounted && epoch == attachmentEpoch) {
        setState(() => importingAttachments--);
      }
    }
  }

  Future<void> previewAttachment(PendingAttachment file) => showDialog<void>(
    context: context,
    builder: (context) => Dialog(
      child: SizedBox(
        width: 1000,
        height: 720,
        child: Column(
          children: [
            Padding(
              padding: const EdgeInsets.all(10),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      file.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: DshTypography.sizeBody),
                    ),
                  ),
                  DshIcon(
                    DshIcons.close.data,
                    label: DshConversationZh.close,
                    onPressed: () => Navigator.pop(context),
                  ),
                ],
              ),
            ),
            Expanded(
              child: InteractiveViewer(
                minScale: .1,
                maxScale: 5,
                child: DshBoundedImage(
                  image: MemoryImage(file.data),
                  evictOnDispose: true,
                  fit: BoxFit.contain,
                ),
              ),
            ),
          ],
        ),
      ),
    ),
  );

  @override
  Widget build(BuildContext context) => _cachedLayout ??= LayoutBuilder(
    builder: (context, box) => buildConversation(
      context,
      widget.maxContentWidth > 0
          ? widget.maxContentWidth
          : (box.maxWidth * .64).clamp(0.0, 920.0),
      box.maxWidth,
    ),
  );

  Widget buildConversation(
    BuildContext context,
    double contentWidth,
    double viewportWidth,
  ) {
    final colors = DshColors(context);
    return DropTarget(
      onDragEntered: (_) => setState(() => dropping = true),
      onDragExited: (_) => setState(() => dropping = false),
      onDragDone: (d) {
        setState(() => dropping = false);
        unawaited(addFiles(d.files));
      },
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: colors.base,
          border: dropping ? Border.all(color: colors.blue, width: 2) : null,
        ),
        child: ListenableBuilder(
          listenable: c.messageChanges,
          builder: (context, _) {
            final hero =
                c.transcript.isEmpty &&
                c.window.oversizedEventSeq == null &&
                !c.interruptible &&
                !c.loading &&
                !c.sending;
            final activityCount =
                c.holdingHistoryActivity ||
                    (!c.readingHistory && (c.interruptible || c.compacting))
                ? 1
                : 0;
            // Changed files sit below the latest reply once a turn settles,
            // where the activity row was while it ran.
            final summaryCount =
                activityCount == 0 &&
                    !c.readingHistory &&
                    !c.window.hasAfter &&
                    c.transcript.isNotEmpty &&
                    c.client != null &&
                    c.selectedId != null &&
                    c.pluginEnabled('dsh-artifacts')
                ? 1
                : 0;
            final lead = activityCount + summaryCount;
            final messageIndices = {
              for (var i = 0; i < c.transcript.length; i++)
                c.transcript[i].id: c.transcript.length - 1 - i + lead,
              if (activityCount == 1) 'turn-activity': 0,
              if (summaryCount == 1) 'artifact-summary': 0,
            };
            return Stack(
              children: [
                Column(
                  children: [
                    if (c.selectedId != null && !c.blankConversation)
                      Container(
                        height:
                            (MediaQuery.textScalerOf(context).scale(24) + 12)
                                .clamp(44.0, double.infinity),
                        padding: EdgeInsets.fromLTRB(
                          widget.headerInset - 8,
                          12,
                          180,
                          0,
                        ),
                        alignment: Alignment.centerLeft,
                        child: Row(
                          children: [
                            Flexible(
                              child: ConstrainedBox(
                                constraints: const BoxConstraints(
                                  maxWidth: 220,
                                ),
                                child: Padding(
                                  padding: const EdgeInsets.symmetric(
                                    horizontal: 8,
                                  ),
                                  child: Text(
                                    c.selected?.displayTitle ??
                                        DshConversationZh.newSession,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: const TextStyle(
                                      fontSize: DshTypography.sizeBody,
                                      fontWeight: FontWeight.w500,
                                    ),
                                  ),
                                ),
                              ),
                            ),
                            const SizedBox(width: 10),
                            DshGlyph(
                              DshIcons.workflow.data,
                              size: 14,
                              color: colors.muted,
                            ),
                            const SizedBox(width: 5),
                            Flexible(
                              child: Text(
                                c.presetName,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  fontSize: DshTypography.sizeCaption,
                                  color: colors.muted,
                                ),
                              ),
                            ),
                          ],
                        ),
                      ),
                    if (c.selectedId != null && !c.blankConversation)
                      Container(
                        height: (MediaQuery.textScalerOf(context).scale(16) + 8)
                            .clamp(32.0, double.infinity),
                        width: double.infinity,
                        decoration: BoxDecoration(
                          border: Border(
                            bottom: BorderSide(color: colors.border),
                          ),
                        ),
                        child: SingleChildScrollView(
                          scrollDirection: Axis.horizontal,
                          padding: EdgeInsets.only(
                            left: widget.headerInset,
                            top: 4,
                          ),
                          child: Row(
                            children: [
                              for (final tab in {
                                'conversation': DshConversationZh.conversation,
                                if (c.menuSettings['trajectory'] != false)
                                  'trajectory': DshConversationZh.trajectory,
                                if (c.menuSettings['artifacts'] != false)
                                  'artifacts': DshConversationZh.artifacts,
                                if (c.menuSettings['code-graph'] != false)
                                  'code-graph': DshConversationZh.codeGraph,
                                if (c.menuSettings['context'] != false)
                                  'context': DshConversationZh.context,
                              }.entries)
                                Padding(
                                  padding: const EdgeInsets.only(right: 36),
                                  child: Container(
                                    decoration: BoxDecoration(
                                      border: Border(
                                        bottom: BorderSide(
                                          color: view == tab.key
                                              ? colors.blue
                                              : Colors.transparent,
                                          width: 2,
                                        ),
                                      ),
                                    ),
                                    child: Semantics(
                                      key: ValueKey(
                                        'conversation-view-${tab.key}',
                                      ),
                                      button: true,
                                      selected: view == tab.key,
                                      child: InkWell(
                                        onTap: () =>
                                            setState(() => view = tab.key),
                                        splashFactory: NoSplash.splashFactory,
                                        child: Align(
                                          alignment: Alignment.topCenter,
                                          child: Text(
                                            tab.value,
                                            style: TextStyle(
                                              fontSize:
                                                  DshTypography.sizeAuxiliary,
                                              height: 16 / 13,
                                              fontWeight: FontWeight.w500,
                                              color: view == tab.key
                                                  ? colors.blue
                                                  : colors.muted,
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
                    if (c.window.oversizedEventSeq != null)
                      Padding(
                        padding: const EdgeInsets.symmetric(
                          horizontal: 32,
                          vertical: 8,
                        ),
                        child: Text(
                          DshConversationZh.historyWindowLimit,
                          style: TextStyle(
                            fontSize: DshTypography.sizeCaption,
                            color: colors.muted,
                          ),
                        ),
                      ),
                    Expanded(
                      child: view == 'context'
                          ? ContextView(controller: c)
                          : view == 'code-graph' &&
                                c.client != null &&
                                c.selectedId != null
                          ? CodeGraphView(
                              key: ValueKey(c.selectedId),
                              api: c.client!,
                              session: c.selectedId!,
                            )
                          : view == 'artifacts' &&
                                c.client != null &&
                                c.selectedId != null
                          ? ArtifactsView(
                              key: ValueKey(c.selectedId),
                              api: c.client!,
                              session: c.selectedId!,
                            )
                          : view == 'trajectory'
                          ? TraceView(controller: c)
                          : hero
                          ? Center(
                              child: Padding(
                                padding: const EdgeInsets.fromLTRB(
                                  32,
                                  0,
                                  40,
                                  0,
                                ),
                                child: ConstrainedBox(
                                  constraints: BoxConstraints(
                                    maxWidth: contentWidth + 32,
                                  ),
                                  child: Column(
                                    mainAxisSize: MainAxisSize.min,
                                    children: [
                                      Wrap(
                                        alignment: WrapAlignment.center,
                                        crossAxisAlignment:
                                            WrapCrossAlignment.center,
                                        spacing: 10,
                                        runSpacing: 8,
                                        children: [
                                          Text(
                                            DshConversationZh.welcomeTitle,
                                            style: DshTypography.headline,
                                            textAlign: TextAlign.center,
                                          ),
                                          Container(
                                            padding: const EdgeInsets.symmetric(
                                              horizontal: 7,
                                              vertical: 3,
                                            ),
                                            decoration: BoxDecoration(
                                              color: colors.blue.withValues(
                                                alpha: .09,
                                              ),
                                              borderRadius:
                                                  BorderRadius.circular(6),
                                            ),
                                            child: Text(
                                              DshConversationZh.rustEdition,
                                              style: TextStyle(
                                                fontSize:
                                                    DshTypography.sizeCaption,
                                                color: DshTokens.of(context)
                                                    .info
                                                    .foreground,
                                              ),
                                            ),
                                          ),
                                        ],
                                      ),
                                      const SizedBox(height: 25),
                                      composer(true),
                                      if (widget.onOpenWorkbench != null) ...[
                                        const SizedBox(height: 12),
                                        DshButton(
                                          key: const Key('hero-open-workbench'),
                                          icon: DshIcons.panelRight.data,
                                          onPressed: () => widget
                                              .onOpenWorkbench
                                              ?.call('start'),
                                          child: const Text(
                                            DshShellZh.showWorkbench,
                                          ),
                                        ),
                                      ],
                                      const SizedBox(height: 12),
                                    ],
                                  ),
                                ),
                              ),
                            )
                          : Stack(
                              key: messageViewport,
                              children: [
                                if (c.transcript.isEmpty && c.loading)
                                  const Center(
                                    child: CircularProgressIndicator(
                                      strokeWidth: 2,
                                    ),
                                  ),
                                Align(
                                  alignment: Alignment.topCenter,
                                  // One selection spans every visible message.
                                  child: SelectionArea(
                                    child: ListView.builder(
                                      // Keep the same viewport while replies
                                      // grow; switching at a message/byte
                                      // threshold moves short conversations
                                      // to the bottom and remounts their text.
                                      // The finite viewport still bounds lazy
                                      // row construction for long histories.
                                      shrinkWrap: true,
                                      key: PageStorageKey(
                                        'messages-${c.selectedId}',
                                      ),
                                      scrollCacheExtent:
                                          ScrollCacheExtent.pixels(240),
                                      controller: scroll,
                                      reverse: true,
                                      findChildIndexCallback: (key) =>
                                          key is ValueKey<String>
                                          ? messageIndices[key.value]
                                          : null,
                                      padding: const EdgeInsets.fromLTRB(
                                        32,
                                        16,
                                        56,
                                        16,
                                      ),
                                      itemCount:
                                          lead +
                                          c.transcript.length +
                                          (c.window.hasBefore ? 1 : 0),
                                      itemBuilder: (context, rawIndex) {
                                        if (summaryCount == 1 &&
                                            rawIndex == 0) {
                                          return Align(
                                            key: const ValueKey(
                                              'artifact-summary',
                                            ),
                                            alignment: Alignment.topCenter,
                                            child: ConstrainedBox(
                                              constraints: BoxConstraints(
                                                maxWidth: contentWidth,
                                              ),
                                              child: ArtifactSummary(
                                                key: ValueKey((
                                                  c.client,
                                                  c.selectedId,
                                                  c.transcript.length,
                                                )),
                                                api: c.client!,
                                                session: c.selectedId!,
                                                onOpenAll: () => setState(
                                                  () => view = 'artifacts',
                                                ),
                                              ),
                                            ),
                                          );
                                        }
                                        if (activityCount == 1 &&
                                            rawIndex == 0) {
                                          return Align(
                                            key: const ValueKey(
                                              'turn-activity',
                                            ),
                                            alignment: Alignment.topCenter,
                                            child: ConstrainedBox(
                                              constraints: BoxConstraints(
                                                maxWidth: contentWidth,
                                              ),
                                              child: TurnActivity(
                                                controller: c,
                                                readingHistory:
                                                    c.holdingLiveHistory,
                                              ),
                                            ),
                                          );
                                        }
                                        final index = rawIndex - lead;
                                        if (index == c.transcript.length) {
                                          return Center(
                                            child: DshButton(
                                              onPressed: c.loading
                                                  ? null
                                                  : () => c.run(
                                                      () => c.loadHistory(
                                                        before:
                                                            c.window.firstSeq,
                                                        merge: true,
                                                      ),
                                                    ),
                                              child: Text(
                                                c.loading
                                                    ? DshConversationZh
                                                          .loadingHistory
                                                    : DshConversationZh
                                                          .loadEarlier,
                                              ),
                                            ),
                                          );
                                        }
                                        final item =
                                            c.transcript[c.transcript.length -
                                                1 -
                                                index];
                                        final readingAnchor = readingAnchors
                                            .putIfAbsent(
                                              item.id,
                                              GlobalKey.new,
                                            );
                                        if (item.kind == 'user' &&
                                            item.seq != null) {
                                          userAnchors[item.seq!] =
                                              readingAnchor;
                                        }
                                        return Align(
                                          key: ValueKey(item.id),
                                          alignment: Alignment.topCenter,
                                          child: ConstrainedBox(
                                            key: readingAnchor,
                                            constraints: BoxConstraints(
                                              maxWidth: contentWidth,
                                            ),
                                            child: MessageCard(
                                              fontSize: c.bodyFontSize,
                                              key: ValueKey(item.id),
                                              item: item,
                                              bottomSpacing: rawIndex == 0
                                                  ? 0
                                                  : 16,
                                              animateUpdates:
                                                  follow && !c.readingHistory,
                                              cwd: c.selected?.cwd,
                                              onOpenPath:
                                                  widget.onOpenPath == null
                                                  ? null
                                                  : _openPath,
                                              onOpenPlan:
                                                  widget.onOpenPlan == null
                                                  ? null
                                                  : _openPlan,
                                              feedback: feedback,
                                              readAloud:
                                                  item.kind == 'turn-tail'
                                                  ? readAloud
                                                  : null,
                                              hintDisplay:
                                                  '${c.conversationSettings['hintDisplay'] ?? 'both'}',
                                              client: c.client,
                                              sessionId: c.selectedId,
                                              onDetails: () => details(item),
                                              onBranch:
                                                  item.kind == 'turn-tail' &&
                                                      !item.streaming &&
                                                      item.seq != null &&
                                                      c.connected
                                                  ? () => branch(item.seq!)
                                                  : null,
                                              onOpenFile:
                                                  widget.onOpenWorkbench == null
                                                  ? null
                                                  : () =>
                                                        widget.onOpenWorkbench!(
                                                          'files',
                                                        ),
                                            ),
                                          ),
                                        );
                                      },
                                    ),
                                  ),
                                ),
                                if (c.readingHistory ||
                                    c.window.hasAfter ||
                                    c.window.needsRefresh ||
                                    !follow)
                                  Positioned(
                                    right:
                                        ((viewportWidth - contentWidth) / 2 +
                                                12)
                                            .clamp(12.0, double.infinity),
                                    bottom: 16,
                                    child: Tooltip(
                                      message: c.unreadHistoryEvents > 0
                                          ? DshConversationZh.unreadActivity(
                                              count: c.unreadHistoryEvents,
                                            )
                                          : DshConversationZh.jumpToBottom,
                                      child: Semantics(
                                        label: DshConversationZh.jumpToBottom,
                                        button: true,
                                        child: DecoratedBox(
                                          decoration: BoxDecoration(
                                            shape: BoxShape.circle,
                                            boxShadow: [
                                              BoxShadow(
                                                color: colors.dark
                                                    ? const Color(0x66000000)
                                                    : const Color(0x18000000),
                                                blurRadius: 8,
                                                offset: const Offset(0, 2),
                                              ),
                                            ],
                                          ),
                                          child: Material(
                                            color: colors.base,
                                            shape: CircleBorder(
                                              side: BorderSide(
                                                color: colors.border,
                                              ),
                                            ),
                                            clipBehavior: Clip.antiAlias,
                                            child: InkWell(
                                              customBorder:
                                                  const CircleBorder(),
                                              onTap: returnToLatest,
                                              child: SizedBox.square(
                                                dimension: 34,
                                                child: DshGlyph(
                                                  DshIcons.chevronDown.data,
                                                  size: 14,
                                                  color: colors.text,
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
                    if (c.interactions.isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.fromLTRB(16, 6, 24, 10),
                        child: ConstrainedBox(
                          constraints: BoxConstraints(
                            maxWidth: contentWidth,
                            maxHeight: (MediaQuery.sizeOf(context).height * .6)
                                .clamp(0, 520),
                          ),
                          child: SingleChildScrollView(
                            child: Column(
                              children: [
                                for (final f in c.interactions)
                                  InteractionCard(
                                    key: ValueKey((c.client, f.rpcId)),
                                    controller: c,
                                    frame: f,
                                  ),
                              ],
                            ),
                          ),
                        ),
                      ),
                    if (c.queued.isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 32),
                        child: Column(
                          children: [
                            for (final q in c.queued.take(4))
                              Container(
                                padding: const EdgeInsets.all(8),
                                child: Row(
                                  children: [
                                    DshGlyph(
                                      DshIcons.clock3.data,
                                      size: 14,
                                      color: colors.muted,
                                    ),
                                    const SizedBox(width: 8),
                                    Expanded(
                                      child: Text(
                                        contentText(
                                          object(q['message'])['content'],
                                        ),
                                        maxLines: 1,
                                        overflow: TextOverflow.ellipsis,
                                        style: const TextStyle(
                                          fontSize: DshTypography.sizeCaption,
                                        ),
                                      ),
                                    ),
                                    DshIcon(
                                      DshIcons.close.data,
                                      label:
                                          DshConversationZh.removeQueuedMessage,
                                      onPressed: () => c.run(() async {
                                        await c.client!.call(
                                          'session.updateQueue',
                                          {
                                            'sessionId': c.selectedId,
                                            'itemId': q['id'],
                                            'action': {'kind': 'remove'},
                                          },
                                          true,
                                        );
                                      }),
                                    ),
                                  ],
                                ),
                              ),
                          ],
                        ),
                      ),
                    if ((!hero || view != 'conversation') &&
                        !c.interactions.any(
                          (f) =>
                              f.type == 'question/requested' ||
                              f.type == 'approval/requested',
                        ))
                      Padding(
                        key: ValueKey('conversation-composer-${c.selectedId}'),
                        padding: const EdgeInsets.fromLTRB(16, 0, 24, 0),
                        child: ConstrainedBox(
                          constraints: BoxConstraints(
                            maxWidth: contentWidth + 32,
                          ),
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Padding(
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 16,
                                ),
                                child: ProgressDock(
                                  key: ValueKey(c.selectedId),
                                  controller: c,
                                ),
                              ),
                              composer(false),
                              SessionStatsLine(controller: c),
                            ],
                          ),
                        ),
                      ),
                  ],
                ),
                if (!hero &&
                    view == 'conversation' &&
                    c.pluginEnabled('dsh-context-jump'))
                  Positioned(
                    top: c.selectedId == null
                        ? 0
                        : (MediaQuery.textScalerOf(context).scale(24) + 12)
                                  .clamp(44.0, double.infinity) +
                              (MediaQuery.textScalerOf(context).scale(16) + 8)
                                  .clamp(32.0, double.infinity),
                    left: 0,
                    right: 0,
                    bottom: 0,
                    child: SizedBox.expand(
                      key: railViewport,
                      child: ListenableBuilder(
                        listenable: c.projectionChanges,
                        builder: (_, _) => UserMessageRail(
                          key: ValueKey('rail-${c.selectedId}'),
                          entries: railEntries,
                          onActivate: jumpMessage,
                          focusNode: railFocus,
                          current: railCurrent,
                          error: navigationError,
                        ),
                      ),
                    ),
                  ),
              ],
            );
          },
        ),
      ),
    );
  }

  Widget composer(bool hero) {
    return ListenableBuilder(
      listenable: Listenable.merge([c.projectionChanges, voice]),
      builder: (_, _) => buildComposer(hero),
    );
  }

  Widget buildComposer(bool hero) {
    final colors = DshColors(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (hero)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Wrap(
              spacing: 2,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                DshButton(
                  key: widget.workspaceAnchor,
                  height: 26,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  icon: DshIcons.folder.data,
                  trailing: DshGlyph(DshIcons.chevronDown.data, size: 12),
                  onPressed: widget.onSelectWorkspace == null
                      ? null
                      : _selectWorkspace,
                  // A selected session keeps its own folder even after the
                  // sidebar retargets 新会话 to another Workspace.
                  child: Text(
                    '${(c.selected == null ? c.currentWorkspace : c.workspaceOf(c.selected!))?['title'] ?? (c.selected?.cwd.isNotEmpty == true ? displayPath(c.selected!.cwd).split(RegExp(r'[/\\]')).last : DshConversationZh.chooseWorkspace)}',
                    style: const TextStyle(fontSize: DshTypography.sizeCaption),
                  ),
                ),
                PopupMenuButton<String>(
                  tooltip: DshConversationZh.agentPreset,
                  onSelected: (v) {
                    c.preset = v;
                    if (c.selectedId != null) {
                      unawaited(
                        c.run(() async {
                          await c.client!.call('agentPreset.select', {
                            'sessionId': c.selectedId,
                            'agentPreset': v,
                          }, true);
                          c.emit();
                        }),
                      );
                    } else {
                      c.emit();
                    }
                  },
                  itemBuilder: (_) => [
                    for (final p in c.presets)
                      PopupMenuItem(
                        value: p['id'] as String,
                        enabled: p['broken'] == null,
                        child: Text('${p['name'] ?? p['id']}'),
                      ),
                  ],
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 6,
                      vertical: 4,
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        DshGlyph(
                          DshIcons.workflow.data,
                          size: 14,
                          color: colors.text,
                        ),
                        const SizedBox(width: 5),
                        Text(
                          c.presetName,
                          style: const TextStyle(
                            fontSize: DshTypography.sizeCaption,
                          ),
                        ),
                        const SizedBox(width: 4),
                        DshGlyph(DshIcons.chevronDown.data, size: 12),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        ListenableBuilder(
          listenable: focus,
          builder: (context, child) => AnimatedContainer(
            key: const ValueKey('composer-card'),
            duration: DshMotion.duration(context, DshMotion.quick),
            curve: DshMotion.curve,
            decoration: BoxDecoration(
              color: colors.dark ? const Color(0xff2c2c2e) : colors.base,
              border: Border.all(
                color: focus.hasFocus
                    ? colors.blue.withValues(alpha: colors.dark ? .7 : .45)
                    : colors.border,
              ),
              borderRadius: BorderRadius.circular(20),
              boxShadow: [
                BoxShadow(
                  color: Colors.black.withValues(
                    alpha: colors.dark ? 0.16 : (focus.hasFocus ? .07 : .04),
                  ),
                  blurRadius: focus.hasFocus ? 18 : 12,
                  offset: const Offset(0, 4),
                ),
              ],
            ),
            child: child,
          ),
          child: Padding(
            padding: const EdgeInsets.only(top: 10),
            child: Column(
              children: [
                if (attachments.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
                    child: Align(
                      alignment: Alignment.centerLeft,
                      child: Wrap(
                        spacing: 8,
                        runSpacing: 8,
                        children: [
                          for (final (index, file) in attachments.indexed)
                            PendingAttachmentTile(
                              key: ValueKey('pending-attachment-$index'),
                              file: file,
                              onPreview: imageMediaTypes.contains(file.type)
                                  ? () => previewAttachment(file)
                                  : null,
                              onRemove: () =>
                                  setState(() => attachments.remove(file)),
                            ),
                        ],
                      ),
                    ),
                  ),
                Focus(
                  onKeyEvent: (_, event) {
                    final keyboard = HardwareKeyboard.instance;
                    if (event is! KeyDownEvent ||
                        (event.logicalKey != LogicalKeyboardKey.enter &&
                            event.logicalKey !=
                                LogicalKeyboardKey.numpadEnter) ||
                        keyboard.isShiftPressed ||
                        keyboard.isAltPressed ||
                        (commandShortcuts
                            ? keyboard.isControlPressed
                            : keyboard.isMetaPressed) ||
                        input.value.composing.isValid &&
                            !input.value.composing.isCollapsed) {
                      return KeyEventResult.ignored;
                    }
                    if (!c.running && primaryShortcutPressed) {
                      final continued = continueNumberedDraft(input.value);
                      if (continued != null) {
                        input.value = continued;
                        c.setDraft(input.text);
                        return KeyEventResult.handled;
                      }
                    }
                    unawaited(
                      send(
                        steer:
                            c.running &&
                            ((c.conversationSettings['busyEnter'] == 'steer') !=
                                primaryShortcutPressed),
                      ),
                    );
                    return KeyEventResult.handled;
                  },
                  child: Actions(
                    actions: {
                      PasteTextIntent: CallbackAction<PasteTextIntent>(
                        onInvoke: (_) {
                          unawaited(paste());
                          return null;
                        },
                      ),
                    },
                    child: TextField(
                      key: const Key('prompt-input'),
                      controller: input,
                      focusNode: focus,
                      contextMenuBuilder: (context, editable) {
                        final items = editable.contextMenuButtonItems
                            .where(
                              (item) =>
                                  item.type != ContextMenuButtonType.paste,
                            )
                            .toList();
                        items.insert(
                          items.length.clamp(0, 2),
                          ContextMenuButtonItem(
                            type: ContextMenuButtonType.paste,
                            onPressed: () {
                              editable.hideToolbar();
                              unawaited(paste());
                            },
                          ),
                        );
                        return AdaptiveTextSelectionToolbar.buttonItems(
                          anchors: editable.contextMenuAnchors,
                          buttonItems: items,
                        );
                      },
                      onChanged: (text) {
                        voice.cancel();
                        c.setDraft(text);
                      },
                      style: DshTypography.composer.copyWith(
                        color: colors.text,
                      ),
                      minLines: hero ? 2 : 1,
                      maxLines: 8,
                      decoration: InputDecoration(
                        hintText:
                            c.currentWorkspace == null && c.selectedId == null
                            ? DshConversationZh.chooseWorkspaceHint
                            : c.planMode?.requestedActive == true
                            ? DshConversationZh.planPromptHint
                            : hero
                            ? DshConversationZh.buildPromptHint
                            : DshConversationZh.messagePromptHint,
                        hintStyle: DshTypography.composer.copyWith(
                          color: colors.muted,
                        ),
                        border: InputBorder.none,
                        enabledBorder: InputBorder.none,
                        focusedBorder: InputBorder.none,
                        isDense: true,
                        contentPadding: const EdgeInsets.fromLTRB(16, 4, 16, 0),
                      ),
                    ),
                  ),
                ),
                const SizedBox(height: 12),
                Padding(
                  padding: const EdgeInsets.fromLTRB(8, 2, 8, 6),
                  child: LayoutBuilder(
                    builder: (context, box) => Flex(
                      direction: box.maxWidth < 400
                          ? Axis.vertical
                          : Axis.horizontal,
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: box.maxWidth < 400
                          ? CrossAxisAlignment.stretch
                          : CrossAxisAlignment.center,
                      children: [
                        Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            ComposerAction(
                              DshIcons.plus.data,
                              asset: 'assets/icons/composer-command.svg',
                              label: DshConversationZh.commands,
                              onPressed: c.selectedId == null
                                  ? null
                                  : commandMenu,
                            ),
                            const SizedBox(width: 2),
                            ComposerAction(
                              DshIcons.link.data,
                              asset: 'assets/icons/composer-reference.svg',
                              label: DshConversationZh.insertSession,
                              onPressed: referenceMenu,
                            ),
                            const SizedBox(width: 2),
                            ComposerAction(
                              DshIcons.attach.data,
                              asset: 'assets/icons/composer-attachment.svg',
                              label: DshConversationZh.uploadFiles,
                              onPressed: pickFiles,
                            ),
                            const SizedBox(width: 6),
                            PermissionControl(
                              key: ValueKey((c.client, c.selectedId)),
                              controller: c,
                            ),
                            PlanModeControl(controller: c),
                          ],
                        ),
                        Flexible(
                          fit: FlexFit.loose,
                          child: Padding(
                            padding: box.maxWidth < 400
                                ? const EdgeInsets.only(top: 6)
                                : EdgeInsets.zero,
                            child: Row(
                              mainAxisAlignment: MainAxisAlignment.end,
                              children: [
                                Flexible(
                                  child: ConstrainedBox(
                                    constraints: BoxConstraints(
                                      maxWidth:
                                          320 *
                                          MediaQuery.textScalerOf(context)
                                              .scale(1),
                                    ),
                                    child: ModelSelectionControls(
                                      controller: c,
                                      onManage: widget.onOpenSettings,
                                      onReturnFocus: () {
                                        if (mounted &&
                                            focus.context != null &&
                                            ModalRoute.of(context)?.isCurrent !=
                                                false) {
                                          focus.requestFocus();
                                        }
                                      },
                                    ),
                                  ),
                                ),
                                if (c.pluginEnabled('dsh-voice-input')) ...[
                                  const SizedBox(width: 8),
                                  VoiceInputButton(
                                    listening: voice.listening,
                                    starting: voice.phase == 'starting',
                                    stopping: voice.phase == 'stopping',
                                    supported: voice.supported,
                                    error: voice.error,
                                    onStart: c.connected && !c.sending
                                        ? () => voice.start(input.text)
                                        : null,
                                    onStop: voice.stop,
                                  ),
                                ],
                                if (c.interruptible || c.compacting)
                                  DshIcon(
                                    DshIcons.stop.data,
                                    label: DshZh.stopExecution,
                                    key: const Key('stop-task'),
                                    onPressed: c.connected
                                        ? () => c.run(c.stop)
                                        : null,
                                  ),
                                const SizedBox(width: 6),
                                Tooltip(
                                  message: c.running
                                      ? '${DshZh.queueMessage}；$primaryShortcutLabel+Enter ${c.conversationSettings['busyEnter'] == 'steer' ? DshZh.queueMessage : DshZh.steerExecution}'
                                      : DshZh.send,
                                  child: Semantics(
                                    label: c.running
                                        ? DshZh.queueMessage
                                        : DshZh.send,
                                    button: true,
                                    child: ValueListenableBuilder(
                                      valueListenable: input,
                                      builder: (_, value, _) {
                                        final canSend =
                                            c.connected &&
                                            !c.sending &&
                                            importingAttachments == 0 &&
                                            !c.changingPlanMode &&
                                            !c.changingModel &&
                                            !c.modelSelectionUnconfirmed &&
                                            (value.text.trim().isNotEmpty ||
                                                attachments.isNotEmpty);
                                        return ShadButton(
                                          key: const Key('send-message'),
                                          onPressed: canSend
                                              ? () => send()
                                              : null,
                                          enabled: canSend,
                                          width: 40,
                                          height: 40,
                                          padding: EdgeInsets.zero,
                                          backgroundColor: canSend
                                              ? colors.blue
                                              : colors.layer,
                                          decoration: ShadDecoration(
                                            border: ShadBorder.all(
                                              radius: BorderRadius.circular(20),
                                            ),
                                          ),
                                          child: DshGlyph(
                                            c.sending
                                                ? DshIcons.loaderCircle.data
                                                : DshIcons.send.data,
                                            size: 16,
                                            color: canSend
                                                ? colors.onAccent
                                                : DshTokens.of(context)
                                                      .disabled,
                                          ),
                                        );
                                      },
                                    ),
                                  ),
                                ),
                              ],
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
        if (c.pluginEnabled('dsh-voice-input') &&
            (voice.error != null || voice.active))
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 6, 12, 4),
            child: Semantics(
              liveRegion: true,
              child: Text(
                voice.error ??
                    (voice.phase == 'starting'
                        ? DshConversationZh.startingSpeech
                        : voice.phase == 'stopping'
                        ? DshConversationZh.finishingSpeech
                        : DshConversationZh.listening),
                key: const ValueKey('voice-status'),
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: voice.error != null
                      ? const Color(0xffd92d20)
                      : colors.muted,
                ),
              ),
            ),
          ),
      ],
    );
  }

  Future<void> commandMenu() async {
    final api = c.client, session = c.selectedId;
    if (api == null || session == null) return;
    await c.run(() async {
      final commands = await api.availableCommands(session);
      if (c.client == api && c.selectedId == session) c.commands = commands;
    });
    if (!mounted || c.client != api || c.selectedId != session) return;
    final selected = await showDialog<String>(
      context: context,
      builder: (context) => SimpleDialog(
        title: const Text(DshConversationZh.commands),
        children: [
          for (final cmd in c.commands)
            SimpleDialogOption(
              onPressed: () => Navigator.pop(context, '${cmd['name']}'),
              child: Text('/${cmd['name']}  ${cmd['description'] ?? ''}'),
            ),
        ],
      ),
    );
    if (selected != null) {
      input.text = '/$selected ';
      focus.requestFocus();
    }
  }

  Future<void> referenceMenu() async {
    final id = await showDialog<String>(
      context: context,
      builder: (context) => SimpleDialog(
        title: const Text(DshConversationZh.insertSession),
        children: [
          for (final s in c.sessions.take(100))
            SimpleDialogOption(
              onPressed: () => Navigator.pop(context, s.id),
              child: Text(s.title),
            ),
        ],
      ),
    );
    if (id != null) {
      final encoded = base64Url
          .encode(utf8.encode(jsonEncode(id)))
          .replaceAll('=', '');
      final label =
          (c.sessions.where((s) => s.id == id).firstOrNull?.title ?? id)
              .replaceAll('\\', '\\\\')
              .replaceAll(']', '\\]');
      input.text += '\n@[$label](dsh-session:$encoded)\n';
      c.setDraft(input.text);
      focus.requestFocus();
    }
  }

  Future<void> branch(int seq) async {
    final api = c.client, session = c.selectedId;
    if (api == null || session == null) return;
    await c.run(() async {
      final result = await api.call('session.fork', {
        'sessionId': session,
        'atSeq': seq,
      }, true);
      if (!mounted || c.client != api || c.selectedId != session) return;
      await c.refreshSessions();
      if (mounted && c.client == api && c.selectedId == session) {
        await c.select(result['sessionId'] as String);
      }
    });
  }

  Future<void> details(TranscriptItem item) => showDialog<void>(
    context: context,
    builder: (context) => Dialog(
      child: SizedBox(
        width: 820,
        height: 620,
        child: Column(
          children: [
            Padding(
              padding: const EdgeInsets.all(16),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      item.title.isEmpty
                          ? DshConversationZh.executionDetails
                          : item.title,
                    ),
                  ),
                  if (item.output.isNotEmpty)
                    DshIcon(
                      DshIcons.copy.data,
                      label: DshConversationZh.copyResult,
                      onPressed: () =>
                          Clipboard.setData(ClipboardData(text: item.output)),
                    ),
                  DshIcon(
                    DshIcons.copy.data,
                    label: item.output.isNotEmpty
                        ? DshConversationZh.copyInput
                        : DshConversationZh.copy,
                    onPressed: () => Clipboard.setData(
                      ClipboardData(text: item.clipboardText),
                    ),
                  ),
                  DshIcon(
                    DshIcons.close.data,
                    label: DshConversationZh.close,
                    onPressed: () => Navigator.pop(context),
                  ),
                ],
              ),
            ),
            const Divider(height: 1),
            Expanded(
              child: TextDocument(
                sections: [
                  (
                    title: item.output.isNotEmpty
                        ? DshConversationZh.input
                        : '',
                    text: item.clipboardText,
                  ),
                  if (item.output.isNotEmpty)
                    (title: DshConversationZh.result, text: item.output),
                ],
              ),
            ),
          ],
        ),
      ),
    ),
  );
}

class ComposerAction extends StatelessWidget {
  const ComposerAction(
    this.icon, {
    super.key,
    required this.label,
    this.asset,
    this.glyphSize = 16,
    this.color,
    this.backgroundColor,
    this.onPressed,
  });
  final IconData icon;
  final String label;
  final String? asset;
  final double glyphSize;
  final Color? color, backgroundColor;
  final VoidCallback? onPressed;
  @override
  Widget build(BuildContext context) => Tooltip(
    message: label,
    child: Semantics(
      label: label,
      button: true,
      child: ShadButton.ghost(
        width: DshTokens.of(context).controlMinimum,
        height: DshTokens.of(context).controlMinimum,
        padding: EdgeInsets.zero,
        onPressed: onPressed,
        enabled: onPressed != null,
        backgroundColor: backgroundColor,
        hoverBackgroundColor: DshColors(context).hover,
        decoration: ShadDecoration(
          border: ShadBorder.all(radius: BorderRadius.circular(10), width: 0),
        ),
        child: DshGlyph(
          icon,
          asset: asset,
          size: glyphSize,
          color:
              color ??
              (onPressed == null
                  ? DshTokens.of(context).disabled
                  : DshColors(context).muted),
        ),
      ),
    ),
  );
}

class MessageCard extends StatelessWidget {
  const MessageCard({
    super.key,
    required this.item,
    this.fontSize = 15,
    this.onDetails,
    this.onOpenFile,
    this.onBranch,
    this.onOpenPath,
    this.onOpenPlan,
    this.feedback,
    this.readAloud,
    this.client,
    this.sessionId,
    this.cwd,
    this.hintDisplay = 'both',
    this.animateUpdates = true,
    this.bottomSpacing = 16,
  });
  final TranscriptItem item;
  final double fontSize;
  final VoidCallback? onDetails, onOpenFile, onBranch;
  final ValueChanged<String>? onOpenPath;
  final ValueChanged<TranscriptItem>? onOpenPlan;
  final DshClient? client;
  final String? sessionId;
  final String? cwd;
  final String hintDisplay;
  final bool animateUpdates;
  final double bottomSpacing;
  final MessageFeedbackController? feedback;
  final ReadAloudController? readAloud;

  String? localLinkPath(String url) {
    if (RegExp(r'^[a-zA-Z]:[\\/]').hasMatch(url) || url.startsWith(r'\\')) {
      return url;
    }
    final uri = Uri.tryParse(url);
    if (uri == null) return null;
    if (uri.scheme == 'file') {
      try {
        return uri.toFilePath(windows: true);
      } on UnsupportedError {
        return null;
      }
    }
    if (uri.scheme.isEmpty && !url.startsWith('#')) {
      return Uri.decodeComponent(uri.path);
    }
    return null;
  }

  Future<void> openLink(String url) async {
    final uri = Uri.tryParse(url);
    if (uri != null && ['http', 'https'].contains(uri.scheme)) {
      await launchUrl(uri, mode: LaunchMode.externalApplication);
    } else {
      final path = localLinkPath(url);
      if (path != null && onOpenPath != null) {
        onOpenPath!(path);
      } else {
        onOpenFile?.call();
      }
    }
  }

  Future<void> linkMenu(
    BuildContext context,
    String label,
    String? href,
    Offset point,
  ) async {
    if (href == null) return;
    final path = localLinkPath(href);
    final action = await nativeContextMenu(context, point, {
      'open': path == null
          ? DshConversationZh.openLinkInBrowser
          : DshConversationZh.openFile,
      'copy': path == null
          ? DshConversationZh.copyLinkAddress
          : DshConversationZh.copyFilePath,
      'text': DshConversationZh.copyLinkText,
      if (path != null && client != null && sessionId != null) ...{
        'reveal': DshConversationZh.revealInFileManager,
        'external': DshConversationZh.openWithLocalTool,
        'save': DshConversationZh.saveOriginalCopy,
      },
    });
    if (!context.mounted) return;
    if (action == 'open') await openLink(href);
    if (action == 'copy' || action == 'text') {
      await Clipboard.setData(
        ClipboardData(
          text: action == 'text' ? label : displayPath(path ?? href),
        ),
      );
    }
    if (path != null && client != null && sessionId != null) {
      try {
        if (action == 'reveal' || action == 'external') {
          final office = RegExp(
            r'\.(docx?|xlsx?|pptx?|wps|et|dps)$',
            caseSensitive: false,
          ).hasMatch(path);
          await client!.request(
            '/__dsh-preview/file-action',
            body: {
              'sessionId': sessionId,
              'path': path,
              'intent': action == 'reveal'
                  ? 'reveal'
                  : office
                  ? 'office'
                  : 'open',
            },
            mutation: true,
          );
        } else if (action == 'save') {
          final target = await getSaveLocation(
            suggestedName: artifactName(path),
          );
          if (target == null || !context.mounted) return;
          await client!.downloadTo(
            previewUrl('file', sessionId!, {'path': path}),
            File(target.path),
          );
        }
      } catch (failure) {
        if (context.mounted) {
          showDshError(context, failure, operation: DshConversationZh.openFile);
        }
      }
    }
  }

  @override
  Widget build(BuildContext context) => Padding(
    padding: EdgeInsets.only(bottom: bottomSpacing),
    child: buildContent(context),
  );

  Widget buildContent(BuildContext context) {
    final colors = DshColors(context);
    if (item.kind == 'retry') return RetryMessage(item: item);
    if (item.kind == 'compaction') {
      return ExpansionTile(
        tilePadding: EdgeInsets.zero,
        leading: DshGlyph(DshIcons.archive.data, size: 16, color: colors.muted),
        title: Text(
          item.title,
          style: TextStyle(
            fontSize: DshTypography.sizeAuxiliary,
            color: item.status == 'failed' ? Colors.red : colors.muted,
          ),
        ),
        children: [if (item.text.isNotEmpty) DshMarkdown(data: item.text)],
      );
    }
    if (item.kind == 'reasoning') {
      return ReasoningMessage(item: item, hintDisplay: hintDisplay);
    }
    if (item.kind == 'tool' || item.kind == 'result') {
      return ToolMessage(
        item: item,
        cwd: cwd,
        onDetails: onDetails,
        onOpenPath: onOpenPath,
        onOpenPlan: onOpenPlan,
        hintDisplay: hintDisplay,
      );
    }
    if (['tool', 'result', 'context'].contains(item.kind)) {
      final failed = item.status == 'failed' || item.status == 'interrupted';
      final icon = item.iconKind == 'todo'
          ? DshIcons.listChecks.data
          : ['read', 'edit', 'system', 'context'].contains(item.iconKind)
          ? DshIcons.fileText.data
          : item.iconKind == 'command'
          ? DshIcons.squareTerminal.data
          : DshIcons.terminal.data;
      return Padding(
        padding: EdgeInsets.zero,
        child: SizedBox(
          height: 24,
          child: InkWell(
            onTap: onDetails,
            borderRadius: BorderRadius.circular(7),
            child: Row(
              children: [
                if (hintDisplay != 'text') ...[
                  DshGlyph(
                    failed ? DshIcons.circleAlert.data : icon,
                    asset: item.iconKind == 'todo' && !failed
                        ? 'assets/icons/task-list.svg'
                        : null,
                    size: 14,
                    color: failed ? Colors.red : colors.muted,
                  ),
                  const SizedBox(width: 8),
                ],
                if (hintDisplay != 'icons')
                  Flexible(
                    flex: 0,
                    child: Text(
                      item.title.isEmpty
                          ? DshConversationZh.toolResult
                          : item.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: DshTypography.sizeBody,
                        height: 24 / 14,
                        color: failed ? Colors.red : colors.muted,
                      ),
                    ),
                  ),
                if (item.summary.isNotEmpty) ...[
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 8),
                    child: Text('·', style: TextStyle(color: colors.muted)),
                  ),
                  Expanded(
                    child: Text(
                      item.summary,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: DshTypography.sizeBody,
                        height: 24 / 14,
                        color: colors.muted,
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      );
    }
    final displayText = boundedMessageText(item.text);
    final body = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (item.kind != 'user' &&
            item.images.isNotEmpty &&
            client != null &&
            sessionId != null)
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final image in item.images)
                AttachmentView(
                  client: client!,
                  sessionId: sessionId!,
                  attachment: image,
                ),
            ],
          ),
        if (item.text.isNotEmpty && item.kind == 'user')
          Text(
            displayText,
            style: DshTypography.composer.copyWith(color: colors.text),
          )
        else if (item.text.isNotEmpty)
          ProgressiveText(
            text: displayText,
            revealInitial: false,
            streaming: item.streaming && animateUpdates,
            builder: (visible) => ConstrainedBox(
              constraints: const BoxConstraints(minHeight: 24),
              child: DshMarkdown(
                data: visible,
                fontSize: fontSize,
                conversationStyle: item.kind == 'assistant',
                imageBaseDirectory: cwd,
                onTapLink: (_, url, _) {
                  if (url != null) unawaited(openLink(url));
                },
                onSecondaryTapLink: (label, href, point) =>
                    linkMenu(context, label, href, point),
              ),
            ),
          ),
      ],
    );
    String clock() {
      final t = item.time;
      if (t == null || t < 0 || t >= 8640000000000000) return '';
      final d = DateTime.fromMillisecondsSinceEpoch(t).toLocal();
      final now = DateTime.now(),
          clock =
              '${d.hour.toString().padLeft(2, '0')}:${d.minute.toString().padLeft(2, '0')}';
      return d.year == now.year && d.month == now.month && d.day == now.day
          ? clock
          : DshConversationZh.monthDayTime(
              month: d.month,
              day: d.day,
              time: clock,
            );
    }

    String clockWithMetrics() => [
      clock(),
      if (item.runMs != null)
        DshConversationZh.elapsedTime(duration: turnDuration(item.runMs!)),
      if (item.ttftMs != null)
        DshConversationZh.firstTokenTime(
          seconds: turnRate(item.ttftMs! / 1000),
        ),
      if (item.tokensPerSecond != null)
        '${turnRate(item.tokensPerSecond!)} tok/s',
    ].where((text) => text.isNotEmpty).join(' · ');

    Widget actions({bool user = false}) => MessageActionRow(
      time: clockWithMetrics(),
      user: user,
      children: [
        DshIcon(
          DshIcons.copy.data,
          label: DshConversationZh.copy,
          size: 28,
          onPressed: () =>
              Clipboard.setData(ClipboardData(text: item.clipboardText)),
        ),
        if (!user &&
            item.kind == 'turn-tail' &&
            readAloud != null &&
            item.text.trim().isNotEmpty) ...[
          const SizedBox(width: 10),
          ListenableBuilder(
            listenable: readAloud!,
            builder: (context, _) {
              final speaking = readAloud!.isSpeaking(item.id);
              return DshIcon(
                speaking ? DshIcons.stop.data : DshIcons.volume2.data,
                asset: speaking
                    ? 'assets/icons/web-stop-read-aloud.svg'
                    : 'assets/icons/web-read-aloud.svg',
                label: speaking
                    ? DshConversationZh.stopReadAloud
                    : DshConversationZh.readAloud,
                size: 28,
                onPressed: () async {
                  try {
                    await readAloud!.toggle(item.id, item.clipboardText);
                  } catch (e) {
                    if (context.mounted) {
                      showDshError(
                        context,
                        e,
                        operation: DshConversationZh.readAloud,
                      );
                    }
                  }
                },
              );
            },
          ),
        ],
        if (user && item.text.length > _messageRenderLimit)
          DshIcon(
            DshIcons.fileText.data,
            label: DshConversationZh.viewFullMessage,
            size: 28,
            onPressed: onDetails,
          ),
        if (!user &&
            item.kind == 'turn-tail' &&
            !item.streaming &&
            feedback != null &&
            item.messageId != null) ...[
          const SizedBox(width: 10),
          MessageFeedbackActions(
            controller: feedback!,
            messageId: item.messageId!,
          ),
        ],
        if (!user && item.kind == 'turn-tail' && item.turnUsage != null)
          TurnUsageButton(usage: item.turnUsage!),
        if (!user && item.kind == 'turn-tail' && item.runMs != null)
          TurnTimeButton(
            runMs: item.runMs!,
            ttftMs: item.ttftMs,
            tokensPerSecond: item.tokensPerSecond,
          ),
        if (!user && item.kind == 'turn-tail') ...[
          const SizedBox(width: 10),
          DshIcon(
            DshIcons.gitBranch.data,
            label: DshConversationZh.branchConversation,
            size: 28,
            onPressed: item.status == 'branch-unavailable' ? null : onBranch,
          ),
        ],
      ],
    );
    if (item.kind == 'turn-tail') {
      return Padding(
        key: ValueKey('actions-${item.id}'),
        padding: EdgeInsets.zero,
        child: Transform.translate(
          offset: const Offset(-6, 0),
          child: actions(),
        ),
      );
    }
    return Padding(
      padding: EdgeInsets.zero,
      child: LayoutBuilder(
        builder: (context, box) => SizedBox(
          width: double.infinity,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (item.kind == 'user') ...[
                Align(
                  alignment: Alignment.centerRight,
                  child: ConstrainedBox(
                    constraints: BoxConstraints(
                      maxWidth: (box.maxWidth * .82).clamp(0.0, 525.0),
                    ),
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.end,
                      children: [
                        for (final (index, file) in item.files.indexed)
                          Padding(
                            padding: EdgeInsets.only(
                              bottom:
                                  index + 1 < item.files.length ||
                                      item.text.isNotEmpty ||
                                      item.images.isNotEmpty
                                  ? 8
                                  : 0,
                            ),
                            child: UploadedFileCard(
                              file: file,
                              onPressed: onOpenPath == null
                                  ? null
                                  : () => onOpenPath!(file.path),
                            ),
                          ),
                        if (item.images.isNotEmpty &&
                            client != null &&
                            sessionId != null)
                          Wrap(
                            alignment: WrapAlignment.end,
                            spacing: 8,
                            runSpacing: 8,
                            children: [
                              for (final image in item.images)
                                AttachmentView(
                                  client: client!,
                                  sessionId: sessionId!,
                                  attachment: image,
                                ),
                            ],
                          ),
                        if (item.text.isNotEmpty)
                          Container(
                            key: ValueKey('bubble-${item.id}'),
                            padding: const EdgeInsets.symmetric(
                              horizontal: 16,
                              vertical: 10,
                            ),
                            decoration: BoxDecoration(
                              color: colors.bubble,
                              borderRadius: BorderRadius.circular(22),
                            ),
                            child: body,
                          ),
                      ],
                    ),
                  ),
                ),
                const SizedBox(height: 6),
                Align(
                  alignment: Alignment.centerRight,
                  child: actions(user: true),
                ),
              ] else if (item.kind == 'notice')
                Text(
                  displayText,
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
                )
              else if (item.kind == 'error')
                DshErrorView(error: item.text)
              else
                body,
            ],
          ),
        ),
      ),
    );
  }
}

class MessageActionRow extends StatefulWidget {
  const MessageActionRow({
    super.key,
    required this.children,
    required this.time,
    this.user = false,
  });
  final List<Widget> children;
  final String time;
  final bool user;
  @override
  State<MessageActionRow> createState() => _MessageActionRowState();
}

class _MessageActionRowState extends State<MessageActionRow> {
  bool hover = false, focused = false;
  @override
  Widget build(BuildContext context) {
    final time = Padding(
      padding: EdgeInsets.only(
        left: widget.user ? 0 : 22,
        right: widget.user ? 22 : 0,
      ),
      child: Opacity(
        key: const ValueKey('message-time-opacity'),
        opacity: hover || focused ? 1 : 0,
        child: Text(
          widget.time,
          style: TextStyle(
            fontSize: DshTypography.sizeBody,
            height: 24 / 14,
            color: DshColors(context).muted,
          ),
        ),
      ),
    );
    return MouseRegion(
      onEnter: (_) => setState(() => hover = true),
      onExit: (_) => setState(() => hover = false),
      child: Focus(
        canRequestFocus: false,
        onFocusChange: (value) => setState(() => focused = value),
        child: SizedBox(
          height: 28,
          child: Row(
            mainAxisSize: widget.user ? MainAxisSize.min : MainAxisSize.max,
            children: [
              if (widget.user && widget.time.isNotEmpty) time,
              ...widget.children,
              if (!widget.user && widget.time.isNotEmpty) Flexible(child: time),
            ],
          ),
        ),
      ),
    );
  }
}

class ReasoningMessage extends StatefulWidget {
  const ReasoningMessage({
    super.key,
    required this.item,
    required this.hintDisplay,
  });
  final TranscriptItem item;
  final String hintDisplay;
  @override
  State<ReasoningMessage> createState() => _ReasoningMessageState();
}

class _ReasoningMessageState extends State<ReasoningMessage> {
  bool expanded = false;
  int page = 0;
  final summaryScroll = ScrollController();
  @override
  void dispose() {
    summaryScroll.dispose();
    super.dispose();
  }

  static const pageSize = 16000;
  @override
  void didUpdateWidget(covariant ReasoningMessage old) {
    super.didUpdateWidget(old);
    if (widget.item.streaming && !old.item.streaming) {
      expanded = false;
      page = 0;
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context), text = widget.item.text;
    final visible = text.trimRight();
    final lineStart = widget.item.streaming ? visible.lastIndexOf('\n') + 1 : 0;
    final firstEnd = visible.indexOf('\n');
    final start = widget.item.streaming
        ? TextDocument.boundary(
            visible,
            (visible.length - 2048).clamp(lineStart, visible.length),
          )
        : 0;
    final end = widget.item.streaming
        ? visible.length
        : (firstEnd < 0 ? visible.length : firstEnd).clamp(0, 512);
    final summary = visible.substring(start, end);
    if (widget.item.streaming) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && summaryScroll.hasClients) {
          summaryScroll.jumpTo(summaryScroll.position.maxScrollExtent);
        }
      });
    }
    final pages = ((text.length + pageSize - 1) ~/ pageSize).clamp(1, 100000);
    page = page.clamp(0, pages - 1);
    var a = page * pageSize, b = ((page + 1) * pageSize).clamp(0, text.length);
    bool low(int i) =>
        i < text.length &&
        text.codeUnitAt(i) >= 0xdc00 &&
        text.codeUnitAt(i) <= 0xdfff;
    if (a > 0 && low(a)) a--;
    if (b < text.length && low(b)) b--;
    return Padding(
      padding: EdgeInsets.zero,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ThinkingSweep(
            running: widget.item.streaming,
            child: InkWell(
              onTap: () => setState(() => expanded = !expanded),
              child: SizedBox(
                height: 24,
                child: Row(
                  children: [
                    if (widget.hintDisplay != 'text') ...[
                      DshGlyph(
                        expanded
                            ? DshIcons.chevronDown.data
                            : DshIcons.brain.data,
                        asset: expanded
                            ? null
                            : 'assets/icons/web-IconThinkOutline14.svg',
                        size: 14,
                        color: colors.muted,
                      ),
                      const SizedBox(width: 8),
                    ],
                    if (widget.hintDisplay != 'icons')
                      Text(
                        DshConversationZh.thinking,
                        style: TextStyle(
                          fontSize: DshTypography.sizeBody,
                          color: colors.muted,
                        ),
                      ),
                    if (!expanded) ...[
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        child: Text('·', style: TextStyle(color: colors.muted)),
                      ),
                      Expanded(
                        child: widget.item.streaming
                            ? SingleChildScrollView(
                                controller: summaryScroll,
                                scrollDirection: Axis.horizontal,
                                child: Text(
                                  summary,
                                  style: TextStyle(
                                    fontSize: DshTypography.sizeBody,
                                    color: colors.muted,
                                  ),
                                ),
                              )
                            : Text(
                                summary,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  fontSize: DshTypography.sizeBody,
                                  color: colors.muted,
                                ),
                              ),
                      ),
                    ],
                  ],
                ),
              ),
            ),
          ),
          if (expanded)
            Padding(
              padding: const EdgeInsets.fromLTRB(22, 4, 0, 4),
              child: SelectableText(
                text.substring(a, b),
                style: TextStyle(
                  fontSize: DshTypography.sizeBody,
                  height: 24 / 14,
                  color: colors.muted,
                ),
              ),
            ),
          if (expanded && pages > 1)
            Row(
              children: [
                DshButton(
                  height: 26,
                  onPressed: page == 0 ? null : () => setState(() => page--),
                  child: const Text(DshConversationZh.previousSection),
                ),
                Text(
                  '${page + 1} / $pages',
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
                ),
                DshButton(
                  height: 26,
                  onPressed: page + 1 >= pages
                      ? null
                      : () => setState(() => page++),
                  child: const Text(DshConversationZh.nextSection),
                ),
              ],
            ),
        ],
      ),
    );
  }
}
