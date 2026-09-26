import 'dart:async';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/services.dart';

/// Reads native attachments separately from Flutter's ordinary text clipboard.
/// A null result lets the caller retain the platform's text-paste behavior.
class ComposerClipboard {
  static const channel = MethodChannel('dsh/clipboard');

  Future<List<XFile>?> readFiles() async {
    for (var attempt = 0; ; attempt++) {
      try {
        final result = await channel.invokeMapMethod<String, dynamic>('read');
        if (result == null) return null;
        final paths = result['files'];
        if (paths is List && paths.isNotEmpty) {
          return paths.whereType<String>().map(XFile.new).toList();
        }
        final bytes = result['png'];
        if (bytes is Uint8List && bytes.isNotEmpty) {
          final name = '粘贴图片-${DateTime.now().millisecondsSinceEpoch}.png';
          return [
            XFile.fromData(
              bytes,
              // Native XFile derives its name from path even for in-memory data.
              path: name,
              name: name,
              mimeType: 'image/png',
            ),
          ];
        }
        return null;
      } on MissingPluginException {
        return null;
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
  }
}
