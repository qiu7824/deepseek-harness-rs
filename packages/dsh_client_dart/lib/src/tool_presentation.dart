import 'dart:convert';
import 'models.dart';
import 'display_path.dart';

class ToolPresentation {
  const ToolPresentation(this.title, this.summary, this.kind, {this.filePath});
  final String title, summary, kind;
  final String? filePath;
}

String planHeading(String text) {
  final end = text.indexOf('\n');
  final heading = (end < 0 ? text : text.substring(0, end))
      .replaceFirst(RegExp(r'^#+\s*'), '')
      .trim();
  return heading.isEmpty ? '计划' : String.fromCharCodes(heading.runes.take(120));
}

ToolPresentation toolPresentation({
  required String name,
  required Json args,
  required Json view,
  required String status,
  required String output,
  String? errorCode,
}) {
  String? pick(Json value, List<String> keys) {
    for (final key in keys) {
      final text = value[key];
      if (text is String && text.isNotEmpty) return text.split('\n').first;
    }
    return null;
  }

  final kind =
      const {
        'read_file': 'read',
        'write_file': 'write',
        'edit_file': 'edit',
        'read_image': 'read_image',
        'read_video': 'read_video',
        'bash': 'command',
        'pwsh': 'command',
        'execute_native': 'command',
        'grep': 'search',
        'glob': 'search',
        'search_files': 'search',
        'web_search': 'search',
        'ask_user_question': 'question',
        'todo_write': 'todo',
      }[name] ??
      name;
  final actions = const {
    'read': ['读取文件', '正在读取文件', '已读取文件', '读取文件失败', '已停止读取文件'],
    'write': ['写入文件', '正在写入文件', '已写入文件', '写入文件失败', '已停止写入文件'],
    'edit': ['编辑文件', '正在编辑文件', '已编辑文件', '编辑文件失败', '已停止编辑文件'],
    'read_image': ['查看图片', '正在查看图片', '已查看图片', '查看图片失败', '已停止查看图片'],
    'read_video': ['查看视频', '正在查看视频', '已查看视频', '查看视频失败', '已停止查看视频'],
  }[kind];
  if (actions != null) {
    final path =
        pick(args, ['path', 'file_path']) ??
        pick(objects(view['locations']).firstOrNull ?? {}, ['path']);
    final state = switch (status) {
      'pending' => 1,
      'complete' => 2,
      'failed' => 3,
      'interrupted' => 4,
      _ => 0,
    };
    return ToolPresentation(
      actions[state],
      path ?? pick(view, ['summary', 'subtitle']) ?? '',
      kind,
      filePath: path,
    );
  }
  if (kind == 'question') {
    var summary = pick(view, ['summary', 'subtitle']) ?? '';
    if (errorCode == 'ASK_CANCELLED') {
      summary = '已取消';
    } else if (errorCode == 'ASK_ABORTED' || status == 'interrupted') {
      summary = '已中断';
    } else if (status == 'pending') {
      summary = '等待回答';
    } else if (status == 'complete' && output.length <= 65536) {
      try {
        final value = object(jsonDecode(output));
        final answers = value['answers'];
        if (answers is List &&
            answers.every(
              (v) =>
                  v is Map &&
                  v['id'] is String &&
                  v['selected'] is List &&
                  (v['selected'] as List).every((s) => s is String) &&
                  (v['custom'] == null || v['custom'] is String),
            )) {
          final answered = answers
              .where(
                (v) =>
                    (v['selected'] as List).isNotEmpty ||
                    (v['custom'] is String &&
                        (v['custom'] as String).isNotEmpty),
              )
              .length;
          summary = '$answered/${answers.length} 已回答';
        }
      } on FormatException {
        /* Keep the original result available in details. */
      }
    }
    return ToolPresentation('提问', summary, kind);
  }
  final titles = const {
    'command': '执行命令',
    'search': '搜索',
    'todo': '更新任务清单',
    'run_code': '执行代码',
    'tool_search': '发现工具',
    'list_directory': '列出目录',
    'present': '展示内容',
    'navigate': '打开网页',
    'web_fetch': '获取网页',
    'task_execution': '任务执行',
  };
  if (name == 'tool_search' || name == 'computer_use') {
    final action = pick(args, ['action']) ?? '';
    final actionLabel =
        const {
          'start': '启动',
          'status': '查看状态',
          'capture': '截取画面',
          'click': '单击',
          'double_click': '双击',
          'type': '输入文字',
          'key': '按键',
          'drag': '拖动',
          'scroll': '滚动',
          'list_sessions': '列出连接',
          'close': '关闭',
          'navigate': '打开网页',
          'list_windows': '列出窗口',
          'focus_window': '聚焦窗口',
        }[action] ??
        action;
    final detail = name == 'tool_search'
        ? pick(args, ['query']) ?? ''
        : actionLabel;
    return ToolPresentation(
      '工具调用',
      detail.isEmpty ? name : '$name · $detail',
      'generic',
    );
  }
  final title = titles[kind] ?? '${view['title'] ?? name}';
  final keys = switch (kind) {
    'command' => ['description', 'command'],
    'search' => ['query', 'pattern', 'url'],
    _ => <String>[],
  };
  var summary = pick(args, keys) ?? pick(view, ['summary', 'subtitle']) ?? '';
  if (kind == 'todo' && view['rawInput'] is List) {
    final todos = objects(view['rawInput']);
    summary =
        '${todos.where((t) => t['status'] == 'completed').length}/${todos.length} 已完成';
    if (todos.isNotEmpty)
      summary +=
          ' · ${todos.where((t) => t['status'] == 'in_progress').firstOrNull?['content'] ?? todos.first['content']}';
  }
  return ToolPresentation(title, summary, '${view['kind'] ?? kind}');
}

String toolPathLabel(String path, String? cwd) {
  if (cwd == null || cwd.isEmpty) return displayPath(path);
  String normalized(String value) {
    value = value.replaceAll('\\', '/');
    if (value.startsWith('//?/UNC/')) return '//${value.substring(8)}';
    return value.startsWith('//?/') ? value.substring(4) : value;
  }

  final root = normalized(cwd).replaceFirst(RegExp(r'/+$'), '');
  final target = normalized(path), prefix = '$root/';
  final windows =
      RegExp(r'^[a-zA-Z]:/').hasMatch(prefix) || root.startsWith('//');
  return (windows
          ? target.toLowerCase().startsWith(prefix.toLowerCase())
          : target.startsWith(prefix))
      ? target.substring(prefix.length)
      : displayPath(path);
}
