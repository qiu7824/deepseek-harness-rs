import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:file_selector/file_selector.dart';
import 'package:dsh_client/dsh_client.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../design/typography.dart';
import '../../design/bounded_image.dart';
import '../../design/context_menu.dart';
import '../../src/composer_attachments.dart';

import 'package:dsh_desktop/l10n/conversation_zh.dart';

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
        label: DshConversationZh.openNamedFile(name: file.name),
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
                  if (onPressed != null) 'open': DshConversationZh.openFile,
                  if (!file.path.startsWith(
                    UploadedFileReceipt.referencePrefix,
                  ))
                    'copy': DshConversationZh.copyFilePath,
                  'name': DshConversationZh.copyFilename,
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
                        fontSize: DshTypography.sizeAuxiliary,
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
  RequestScope scope = RequestScope();
  int generation = 0;
  Uint8List? data;
  Object? error;
  @override
  void initState() {
    super.initState();
    load();
  }

  @override
  void didUpdateWidget(AttachmentView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.client != widget.client ||
        oldWidget.sessionId != widget.sessionId ||
        oldWidget.attachment['attachmentId'] !=
            widget.attachment['attachmentId']) {
      scope.cancel();
      scope = RequestScope();
      data = null;
      error = null;
      load();
    }
  }

  @override
  void dispose() {
    scope.cancel();
    data = null;
    super.dispose();
  }

  Future<void> load() async {
    scope.cancel();
    final requestScope = scope = RequestScope();
    final revision = ++generation;
    final client = widget.client, sessionId = widget.sessionId;
    final attachmentId = widget.attachment['attachmentId'];
    bool current() =>
        mounted &&
        !requestScope.cancelled &&
        revision == generation &&
        client == widget.client &&
        sessionId == widget.sessionId;
    setState(() => error = null);
    try {
      final result = await client.rpc(
        'session.attachment',
        payload: {'sessionId': sessionId, 'attachmentId': attachmentId},
        scope: requestScope,
      );
      if (!current()) return;
      final encoded = result['data'] as String;
      if (encoded.length > 24 * 1024 * 1024) {
        throw StateError(DshConversationZh.imageDisplayLimit);
      }
      final decoded = base64Decode(encoded);
      if (current()) setState(() => data = decoded);
    } catch (e) {
      if (current()) setState(() => error = e);
    }
  }

  Future<void> save() async {
    final revision = generation, bytes = data;
    try {
      final target = await getSaveLocation(
        suggestedName: widget.attachment['name'] as String? ?? 'image.png',
      );
      if (mounted &&
          revision == generation &&
          target != null &&
          bytes != null) {
        await XFile.fromData(bytes).saveTo(target.path);
      }
    } catch (failure) {
      if (mounted && revision == generation) {
        showDshError(context, failure, operation: DshConversationZh.saveImage);
      }
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
                      widget.attachment['name'] as String? ??
                          DshConversationZh.image,
                      style: const TextStyle(fontSize: DshTypography.sizeBody),
                    ),
                  ),
                  DshIcon(
                    DshIcons.download.data,
                    label: DshConversationZh.saveImage,
                    onPressed: save,
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
                  image: MemoryImage(data!),
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
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: 10),
    child: SizedBox(
      width: 220,
      height: 160,
      child: error != null
          ? SingleChildScrollView(
              child: DshErrorView(error: error!, onRetry: load),
            )
          : data == null
          ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
          : InkWell(
              onTap: preview,
              onSecondaryTapDown: (event) async {
                final action = await nativeContextMenu(
                  context,
                  event.globalPosition,
                  {
                    'preview': DshConversationZh.viewOriginalImage,
                    'save': DshConversationZh.saveImage,
                    'name': DshConversationZh.copyImageName,
                  },
                );
                if (!mounted) return;
                if (action == 'preview') await preview();
                if (action == 'save') await save();
                if (action == 'name') {
                  await Clipboard.setData(
                    ClipboardData(
                      text:
                          '${widget.attachment['name'] ?? DshConversationZh.image}',
                    ),
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

/// One composer attachment before sending: images show a thumbnail that opens
/// a preview, other files show their name and size. Both can be removed.
class PendingAttachmentTile extends StatelessWidget {
  const PendingAttachmentTile({
    super.key,
    required this.file,
    required this.onRemove,
    this.onPreview,
  });
  final PendingAttachment file;
  final VoidCallback onRemove;
  final VoidCallback? onPreview;

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final size = UploadedFileReceipt(
      file.name,
      '',
      file.data.length,
    ).displaySize;
    final remove = Semantics(
      button: true,
      label: DshConversationZh.removeNamedAttachment(name: file.name),
      child: Material(
        color: colors.base,
        shape: CircleBorder(side: BorderSide(color: colors.border)),
        child: InkWell(
          customBorder: const CircleBorder(),
          onTap: onRemove,
          child: Padding(
            padding: const EdgeInsets.all(3),
            child: DshGlyph(DshIcons.close.data, size: 12, color: colors.muted),
          ),
        ),
      ),
    );
    if (imageMediaTypes.contains(file.type)) {
      return Tooltip(
        message: '${file.name} · $size',
        child: SizedBox(
          width: 64,
          height: 64,
          child: Stack(
            clipBehavior: Clip.none,
            children: [
              Positioned.fill(
                child: Semantics(
                  button: onPreview != null,
                  label: DshConversationZh.previewNamedImage(name: file.name),
                  child: InkWell(
                    onTap: onPreview,
                    borderRadius: BorderRadius.circular(10),
                    child: ClipRRect(
                      borderRadius: BorderRadius.circular(10),
                      child: DecoratedBox(
                        position: DecorationPosition.foreground,
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(10),
                          border: Border.all(color: colors.border),
                        ),
                        child: Image.memory(
                          file.data,
                          cacheWidth: 128,
                          fit: BoxFit.cover,
                          gaplessPlayback: true,
                          errorBuilder: (_, _, _) => ColoredBox(
                            color: colors.layer,
                            child: Center(
                              child: DshGlyph(
                                DshIcons.image.data,
                                size: 18,
                                color: colors.muted,
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
              Positioned(top: -6, right: -6, child: remove),
            ],
          ),
        ),
      );
    }
    return Container(
      constraints: const BoxConstraints(maxWidth: 240),
      padding: const EdgeInsets.fromLTRB(10, 6, 6, 6),
      decoration: BoxDecoration(
        color: colors.layer,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: colors.border),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          DshGlyph(DshIcons.attach.data, size: 14, color: colors.muted),
          const SizedBox(width: 6),
          Flexible(
            child: Text(
              file.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: DshTypography.caption.copyWith(color: colors.text),
            ),
          ),
          const SizedBox(width: 6),
          Text(
            size,
            style: DshTypography.caption.copyWith(color: colors.muted),
          ),
          const SizedBox(width: 6),
          remove,
        ],
      ),
    );
  }
}
