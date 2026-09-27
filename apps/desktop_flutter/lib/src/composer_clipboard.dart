import 'package:file_selector/file_selector.dart';
import 'package:flutter/services.dart';

import 'composer_attachments.dart';

/// Reads native attachments separately from Flutter's ordinary text clipboard.
/// A null result lets the caller retain the platform's text-paste behavior.
class ComposerClipboard {
  static const channel = MethodChannel('dsh/clipboard');

  Future<List<XFile>?> readFiles() async {
    final contents = await NativeClipboard.read();
    if (contents.files.isNotEmpty)
      return contents.files.map(XFile.new).toList();
    final bytes = contents.png;
    if (bytes == null) return null;
    final name = pastedImageName(DateTime.now());
    return [
      XFile.fromData(bytes, path: name, name: name, mimeType: 'image/png'),
    ];
  }
}
