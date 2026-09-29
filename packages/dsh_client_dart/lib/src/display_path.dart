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

/// Comparison key for a folder: the Host may report a session folder with the
/// extended-length prefix, other separators or casing than its Workspace.
String workspacePathKey(Object? path) {
  var value = displayPath('${path ?? ''}');
  final windows = RegExp(r'^[a-zA-Z]:[\\/]|^\\\\').hasMatch(value);
  if (windows) value = value.replaceAll('/', r'\').toLowerCase();
  final separator = windows ? r'\' : '/';
  final root = windows ? RegExp(r'^[a-z]:\\$') : RegExp(r'^/$');
  while (value.endsWith(separator) && !root.hasMatch(value)) {
    value = value.substring(0, value.length - 1);
  }
  return value;
}

/// Normalize recognizable absolute paths in prose, preserving other escapes.
String displayPathText(String text) => text
    .replaceAllMapped(
      RegExp(r'\\\\\?\\UNC\\', caseSensitive: false),
      (_) => r'\\',
    )
    .replaceAllMapped(RegExp(r'\\\\\?\\(?=[a-zA-Z]:[\\/])'), (_) => '');
