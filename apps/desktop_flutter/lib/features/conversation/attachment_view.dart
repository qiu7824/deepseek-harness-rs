import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:file_selector/file_selector.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:dsh_client/dsh_client.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';
import '../../design/context_menu.dart';

class UploadedFileCard extends StatelessWidget {
  const UploadedFileCard({super.key, required this.file, this.onPressed});
  final UploadedFileReceipt file;
  final VoidCallback? onPressed;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Tooltip(
      message: file.path.startsWith(UploadedFileReceipt.referencePrefix)
          ? file.name
          : displayPath(file.path),
      child: Semantics(
        button: true,
        enabled: onPressed != null,
        label: '打开文件：${file.name}',
        child: Material(
          color: colors.layer,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(10),
            side: BorderSide(color: colors.border),
          ),
          child: InkWell(
            onTap: onPressed,
            onSecondaryTapDown: (event) async {
              final action = await nativeContextMenu(
                context,
                event.globalPosition,
                {
                  if (onPressed != null) 'open': '打开文件',
                  if (!file.path.startsWith(
                    UploadedFileReceipt.referencePrefix,
                  ))
                    'copy': '复制文件路径',
                  'name': '复制文件名',
                },
              );
              if (!context.mounted) return;
              if (action == 'open') onPressed?.call();
              if (action == 'copy' || action == 'name') {
                await Clipboard.setData(
                  ClipboardData(
                    text: action == 'copy' ? displayPath(file.path) : file.name,
                  ),
                );
              }
            },
            borderRadius: BorderRadius.circular(10),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Flexible(
                    child: Text(
                      file.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: DshTypography.body.copyWith(
                        fontSize: 13,
                        height: 18 / 13,
                        color: colors.text,
                      ),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Text(
                    file.displaySize,
                    style: DshTypography.caption.copyWith(color: colors.text),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class AttachmentView extends StatefulWidget {
  const AttachmentView({
    super.key,
    required this.client,
    required this.sessionId,
    required this.attachment,
  });
  final DshClient client;
  final String sessionId;
  final Json attachment;
  @override
  State<AttachmentView> createState() => _AttachmentViewState();
}

class _AttachmentViewState extends State<AttachmentView> {
  final scope = RequestScope();
  Uint8List? data;
  String? error;
  @override
  void initState() {
    super.initState();
    load();
  }

  @override
  void dispose() {
    scope.cancel();
    data = null;
    super.dispose();
  }

  Future<void> load() async {
    try {
      final result = await widget.client.rpc(
        'session.attachment',
        payload: {
          'sessionId': widget.sessionId,
          'attachmentId': widget.attachment['attachmentId'],
        },
        scope: scope,
      );
      if (!mounted) return;
      final encoded = result['data'] as String;
      if (encoded.length > 24 * 1024 * 1024) throw StateError('图片超过显示上限');
      final decoded = base64Decode(encoded);
      if (mounted) setState(() => data = decoded);
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    }
  }

  Future<void> save() async {
    final target = await getSaveLocation(
      suggestedName: widget.attachment['name'] as String? ?? 'image.png',
    );
    if (target != null && data != null) {
      await XFile.fromData(data!).saveTo(target.path);
    }
  }

  Future<void> preview() => showDialog<void>(
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
                      widget.attachment['name'] as String? ?? '图片',
                      style: const TextStyle(fontSize: 14),
                    ),
                  ),
                  DshIcon(LucideIcons.download, label: '保存图片', onPressed: save),
                  DshIcon(
                    LucideIcons.x,
                    label: '关闭',
                    onPressed: () => Navigator.pop(context),
                  ),
                ],
              ),
            ),
            Expanded(
              child: InteractiveViewer(
                minScale: .1,
                maxScale: 5,
                child: Image.memory(
                  data!,
                  cacheWidth: 2000,
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
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 10),
    child: SizedBox(
      width: 220,
      height: 160,
      child: error != null
          ? DshEmpty(error!, icon: LucideIcons.circleAlert)
          : data == null
          ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
          : InkWell(
              onTap: preview,
              onSecondaryTapDown: (event) async {
                final action = await nativeContextMenu(
                  context,
                  event.globalPosition,
                  {'preview': '查看原图', 'save': '保存图片', 'name': '复制图片名称'},
                );
                if (!mounted) return;
                if (action == 'preview') await preview();
                if (action == 'save') await save();
                if (action == 'name') {
                  await Clipboard.setData(
                    ClipboardData(text: '${widget.attachment['name'] ?? '图片'}'),
                  );
                }
              },
              child: ClipRRect(
                borderRadius: BorderRadius.circular(12),
                child: Image.memory(
                  data!,
                  cacheWidth: 600,
                  fit: BoxFit.cover,
                  errorBuilder: (_, error, _) => DshEmpty('$error'),
                ),
              ),
            ),
    ),
  );
}
