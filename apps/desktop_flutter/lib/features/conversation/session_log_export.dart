import 'dart:async';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../src/controller.dart';

import 'package:dsh_desktop/l10n/conversation_zh.dart';

String sessionLogFilename(String sessionId) =>
    'dsh-session-${sessionId.replaceAll(RegExp(r'[^A-Za-z0-9_-]'), '_')}.zip';

String sessionLogExportPath(String sessionId) => Uri(
  path: '/api/session.export',
  queryParameters: {'sessionId': sessionId, 'includeDescendants': 'true'},
).toString();

Future<int> downloadSessionLog(
  DshClient api,
  String sessionId,
  File destination, {
  RequestScope? scope,
  void Function(int bytes)? onProgress,
}) => api.downloadTo(
  sessionLogExportPath(sessionId),
  destination,
  scope: scope,
  maxBytes: 1 << 40,
  totalTimeout: const Duration(hours: 2),
  onProgress: onProgress,
);

typedef SessionLogLocationPicker = Future<String?> Function(String filename);

Future<String?> _pickSessionLogLocation(String filename) async {
  final location = await getSaveLocation(
    suggestedName: filename,
    acceptedTypeGroups: [
      const XTypeGroup(label: 'ZIP', extensions: ['zip']),
    ],
  );
  return location?.path;
}

class SessionLogExportAction extends StatelessWidget {
  const SessionLogExportAction({
    super.key,
    required this.controller,
    required this.sessionId,
    this.pickLocation,
  });

  final DesktopController controller;
  final String sessionId;
  final SessionLogLocationPicker? pickLocation;

  @override
  Widget build(BuildContext context) => DshIcon(
    DshIcons.download.data,
    label: DshConversationZh.downloadSessionLog,
    size: 28,
    onPressed: controller.client == null
        ? null
        : () {
            final api = controller.client!;
            showDialog<void>(
              context: context,
              builder: (_) => SessionLogExportDialog(
                controller: controller,
                api: api,
                sessionId: sessionId,
                pickLocation: pickLocation ?? _pickSessionLogLocation,
              ),
            );
          },
  );
}

class SessionLogExportDialog extends StatefulWidget {
  const SessionLogExportDialog({
    super.key,
    required this.controller,
    required this.api,
    required this.sessionId,
    required this.pickLocation,
  });

  final DesktopController controller;
  final DshClient api;
  final String sessionId;
  final SessionLogLocationPicker pickLocation;

  @override
  State<SessionLogExportDialog> createState() => _SessionLogExportDialogState();
}

class _SessionLogExportDialogState extends State<SessionLogExportDialog> {
  RequestScope scope = RequestScope();
  bool downloading = false, complete = false, choosing = false;
  Object? error;
  int bytes = 0;
  int lastPaintedAt = 0;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(checkSession);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) unawaited(start());
    });
  }

  void checkSession() {
    if (widget.controller.selectedId == widget.sessionId &&
        identical(widget.controller.client, widget.api)) {
      return;
    }
    scope.cancel();
    if (!mounted) return;
    final route = ModalRoute.of(context);
    if (route != null) Navigator.of(context).removeRoute(route);
  }

  Future<void> start() async {
    if (choosing || downloading) return;
    scope.cancel();
    final requestScope = scope = RequestScope();
    setState(() {
      choosing = true;
      error = null;
      complete = false;
      bytes = 0;
    });
    try {
      final path = await widget.pickLocation(
        sessionLogFilename(widget.sessionId),
      );
      if (!mounted || requestScope.cancelled) return;
      if (path == null) {
        Navigator.of(context).pop();
        return;
      }
      setState(() => downloading = true);
      await downloadSessionLog(
        widget.api,
        widget.sessionId,
        File(path),
        scope: requestScope,
        onProgress: (count) {
          final now = DateTime.now().millisecondsSinceEpoch;
          if (!mounted || requestScope.cancelled || now - lastPaintedAt < 150) {
            return;
          }
          lastPaintedAt = now;
          setState(() => bytes = count);
        },
      );
      if (mounted && !requestScope.cancelled) {
        setState(() => complete = true);
      }
    } catch (failure) {
      if (mounted && !requestScope.cancelled) {
        setState(() => error = failure);
      }
    } finally {
      if (mounted && !requestScope.cancelled) {
        setState(() {
          downloading = false;
          choosing = false;
        });
      }
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(checkSession);
    scope.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(
      error != null
          ? DshConversationZh.sessionExportFailed
          : complete
          ? DshConversationZh.sessionExportComplete
          : DshConversationZh.exportingSession,
    ),
    content: error != null
        ? SizedBox(
            width: 480,
            child: SingleChildScrollView(
              child: DshErrorView(
                error: error!,
                onRetry: choosing || downloading ? null : start,
              ),
            ),
          )
        : Text(
            complete
                ? DshConversationZh.sessionExportedHint
                : downloading
                ? DshConversationZh.exportProgress(bytes: bytes)
                : DshConversationZh.chooseExportLocation,
          ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text(DshConversationZh.close),
      ),
    ],
  );
}
