import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/services.dart';

/// One file waiting in the composer until the message is accepted.
typedef PendingAttachment = ({String name, Uint8List data, String type});

/// Raster formats the Host accepts as image parts (Web `isRasterFile`).
const imageMediaTypes = {'image/png', 'image/jpeg', 'image/webp', 'image/gif'};

/// Largest decoded image the composer admits (Web `maxImagePixels`).
const maxClipboardImagePixels = 32000000;
const maxClipboardAttachmentBytes = 16 * 1024 * 1024;

/// Detects the raster format from its signature so an image part always names
/// the type the Host codec will detect; a mislabeled extension cannot reject it.
String? sniffImageType(Uint8List data) {
  bool starts(List<int> signature, [int offset = 0]) {
    if (data.length < offset + signature.length) return false;
    for (var i = 0; i < signature.length; i++) {
      if (data[offset + i] != signature[i]) return false;
    }
    return true;
  }

  if (starts(const [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])) {
    return 'image/png';
  }
  if (starts(const [0xFF, 0xD8, 0xFF])) return 'image/jpeg';
  if (starts(const [0x47, 0x49, 0x46, 0x38])) return 'image/gif';
  if (starts(const [0x52, 0x49, 0x46, 0x46]) &&
      starts(const [0x57, 0x45, 0x42, 0x50], 8)) {
    return 'image/webp';
  }
  return null;
}

const _fileMediaTypes = {
  'pdf': 'application/pdf',
  'txt': 'text/plain',
  'log': 'text/plain',
  'md': 'text/markdown',
  'markdown': 'text/markdown',
  'csv': 'text/csv',
  'tsv': 'text/tab-separated-values',
  'json': 'application/json',
  'xml': 'application/xml',
  'yaml': 'application/yaml',
  'yml': 'application/yaml',
  'html': 'text/html',
  'htm': 'text/html',
  'svg': 'image/svg+xml',
  'bmp': 'image/bmp',
  'zip': 'application/zip',
  'doc': 'application/msword',
  'docx':
      'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  'xls': 'application/vnd.ms-excel',
  'xlsx': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  'ppt': 'application/vnd.ms-powerpoint',
  'pptx': 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
};

/// Media type for a composer attachment: sniffed raster types first, then the
/// extension for generic files, then an opaque byte stream.
String attachmentMediaType(String name, Uint8List data) {
  final sniffed = sniffImageType(data);
  if (sniffed != null) return sniffed;
  final dot = name.lastIndexOf('.');
  final ext = dot < 0 ? '' : name.substring(dot + 1).toLowerCase();
  return _fileMediaTypes[ext] ?? 'application/octet-stream';
}

/// Stable ASCII name for a pasted image, e.g. `pasted-image-20260926-101530.png`.
String pastedImageName(DateTime time) {
  String two(int value) => value.toString().padLeft(2, '0');
  return 'pasted-image-${time.year}${two(time.month)}${two(time.day)}-'
      '${two(time.hour)}${two(time.minute)}${two(time.second)}.png';
}

/// Cuts a PNG stream after its IEND chunk. Clipboard memory can be larger than
/// the image it holds; a stream without a complete chunk chain is rejected.
Uint8List? trimPngStream(Uint8List data) {
  if (sniffImageType(data) != 'image/png') return null;
  final view = ByteData.sublistView(data);
  var offset = 8;
  while (offset + 12 <= data.length) {
    final length = view.getUint32(offset);
    final end = offset + 12 + length;
    if (length > data.length || end > data.length) return null;
    final type = String.fromCharCodes(data, offset + 4, offset + 8);
    if (type == 'IEND') {
      return end == data.length ? data : Uint8List.sublistView(data, 0, end);
    }
    offset = end;
  }
  return null;
}

/// Encodes opaque BGRA pixels, top row first, as PNG.
Future<Uint8List> pngFromBgra(Uint8List pixels, int width, int height) async {
  if (width <= 0 ||
      height <= 0 ||
      width * height > maxClipboardImagePixels ||
      pixels.length != width * height * 4) {
    throw StateError('剪贴板图片尺寸无效');
  }
  final decoded = Completer<ui.Image>();
  ui.decodeImageFromPixels(
    pixels,
    width,
    height,
    ui.PixelFormat.bgra8888,
    decoded.complete,
  );
  final image = await decoded.future;
  try {
    final png = await image.toByteData(format: ui.ImageByteFormat.png);
    if (png == null) throw StateError('无法编码剪贴板图片');
    if (png.lengthInBytes > maxClipboardAttachmentBytes) {
      throw StateError('附件总大小不能超过 16 MiB');
    }
    return png.buffer.asUint8List(png.offsetInBytes, png.lengthInBytes);
  } finally {
    image.dispose();
  }
}

/// Clipboard content beyond plain text.
class ClipboardAttachments {
  const ClipboardAttachments({this.files = const [], this.png});
  final List<String> files;
  final Uint8List? png;
  bool get isEmpty => files.isEmpty && png == null;
}

/// Reads copied files and images through the runner's `dsh/clipboard`
/// channel. A client without the channel reads as empty so text paste keeps
/// working.
class NativeClipboard {
  static const channel = MethodChannel('dsh/clipboard');

  static Future<ClipboardAttachments> read() async {
    Object? raw;
    for (var attempt = 0; ; attempt++) {
      try {
        raw = await channel.invokeMethod<Object>('read');
        break;
      } on MissingPluginException {
        return const ClipboardAttachments();
      } on PlatformException catch (e) {
        if (e.code == 'clipboard-busy' && attempt < 2) {
          await Future<void>.delayed(const Duration(milliseconds: 25));
          continue;
        }
        throw StateError(switch (e.code) {
          'clipboard-busy' => '剪贴板正被其他程序使用，请重试。',
          'clipboard-too-large' => '附件总大小不能超过 16 MiB，图片像素不能超过 3200 万。',
          'clipboard-too-many-files' => '单条消息最多添加 8 个附件',
          _ => '无法读取剪贴板图片或文件，请重新复制后重试。',
        });
      }
    }
    if (raw is! Map) return const ClipboardAttachments();
    final files = [
      for (final file in raw['files'] as List? ?? const [])
        if (file is String && file.isNotEmpty) file,
    ];
    if (files.length > 8) throw StateError('单条消息最多添加 8 个附件');
    if (files.isNotEmpty) return ClipboardAttachments(files: files);
    final png = raw['png'];
    if (png is Uint8List) {
      if (png.length > maxClipboardAttachmentBytes) {
        throw StateError('附件总大小不能超过 16 MiB');
      }
      final trimmed = trimPngStream(png);
      if (trimmed != null && trimmed.length >= 24) {
        final dimensions = ByteData.sublistView(trimmed);
        final width = dimensions.getUint32(16),
            height = dimensions.getUint32(20);
        if (width == 0 ||
            height == 0 ||
            width * height > maxClipboardImagePixels) {
          throw StateError('剪贴板图片尺寸无效或像素超过 3200 万');
        }
        return ClipboardAttachments(png: trimmed);
      }
    }
    final bgra = raw['bgra'], width = raw['width'], height = raw['height'];
    if (bgra is Uint8List && width is int && height is int) {
      return ClipboardAttachments(png: await pngFromBgra(bgra, width, height));
    }
    return const ClipboardAttachments();
  }
}
