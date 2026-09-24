import 'dart:async';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import '../../src/controller.dart';

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
    LucideIcons.download,
    label: '下载会话日志',
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
  final scope = RequestScope();
  bool downloading = false, complete = false;
  String? error;
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
    try {
      final path = await widget.pickLocation(
        sessionLogFilename(widget.sessionId),
      );
      if (!mounted || scope.cancelled) return;
      if (path == null) {
        Navigator.of(context).pop();
        return;
      }
      setState(() => downloading = true);
      await downloadSessionLog(
        widget.api,
        widget.sessionId,
        File(path),
        scope: scope,
        onProgress: (count) {
          final now = DateTime.now().millisecondsSinceEpoch;
          if (!mounted || now - lastPaintedAt < 150) return;
          lastPaintedAt = now;
          setState(() => bytes = count);
        },
      );
      if (mounted && !scope.cancelled) {
        setState(() => complete = true);
      }
    } catch (failure) {
      if (mounted && !scope.cancelled) {
        setState(() => error = '$failure');
      }
    } finally {
      if (mounted && !scope.cancelled) {
        setState(() => downloading = false);
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
          ? 'Session 导出失败'
          : complete
          ? 'Session 导出完成'
          : '正在导出 Session',
    ),
    content: Text(
      error ??
          (complete
              ? '会话、子会话和附件已保存到所选 ZIP 文件。'
              : downloading
              ? '正在保存会话、子会话和附件…${bytes > 0 ? ' 已写入 ${(bytes / 1048576).toStringAsFixed(1)} MiB' : ''}'
              : '请选择 ZIP 文件的保存位置。'),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.of(context).pop(),
        child: const Text('关闭'),
      ),
    ],
  );
}
