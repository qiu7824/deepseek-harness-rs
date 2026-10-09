import '../../design/primitives.dart';

import 'package:flutter/widgets.dart';

const artifactCategories = {
  'all': '全部类型',
  'documents': '文档',
  'sheets': '表格',
  'slides': '演示',
  'images': '图片',
  'pages': '网页',
  'code': '代码',
  'other': '其他',
};
String artifactCategory(String path) {
  final ext = path
      .replaceAll('\\', '/')
      .split('/')
      .last
      .split('.')
      .last
      .toLowerCase();
  if (['doc', 'docx', 'pdf', 'txt', 'md', 'rtf', 'wps'].contains(ext)) {
    return 'documents';
  }
  if (['xls', 'xlsx', 'csv', 'tsv', 'et'].contains(ext)) return 'sheets';
  if (['ppt', 'pptx', 'dps'].contains(ext)) return 'slides';
  if ([
    'png',
    'jpg',
    'jpeg',
    'gif',
    'webp',
    'svg',
    'bmp',
    'tif',
    'tiff',
  ].contains(ext)) {
    return 'images';
  }
  if (['html', 'htm'].contains(ext)) return 'pages';
  if ([
    'rs',
    'py',
    'js',
    'ts',
    'tsx',
    'jsx',
    'dart',
    'json',
    'yaml',
    'yml',
    'toml',
    'css',
    'scss',
    'go',
    'java',
    'c',
    'cpp',
    'h',
    'cs',
    'sql',
    'sh',
    'ps1',
  ].contains(ext)) {
    return 'code';
  }
  return 'other';
}

IconData artifactCategoryIcon(String category) => switch (category) {
  'images' => DshIcons.image.data,
  'code' => DshIcons.code.data,
  'pages' => DshIcons.browser.data,
  _ => DshIcons.file.data,
};
