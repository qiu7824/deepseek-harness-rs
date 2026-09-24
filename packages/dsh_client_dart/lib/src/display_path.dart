/// Windows extended-length prefixes belong to filesystem APIs, not labels.
/// Keep transport paths intact and apply this only at a display/copy boundary.
String displayPath(String path) {
  if (RegExp(r'^\\\\\?\\UNC\\', caseSensitive: false).hasMatch(path)) {
    return r'\\' + path.substring(8);
  }
  if (RegExp(r'^\\\\\?\\[a-zA-Z]:[\\/]').hasMatch(path)) {
    return path.substring(4);
  }
  return path;
}

/// Normalize recognizable absolute paths in prose, preserving other escapes.
String displayPathText(String text) => text
    .replaceAllMapped(
      RegExp(r'\\\\\?\\UNC\\', caseSensitive: false),
      (_) => r'\\',
    )
    .replaceAllMapped(RegExp(r'\\\\\?\\(?=[a-zA-Z]:[\\/])'), (_) => '');
