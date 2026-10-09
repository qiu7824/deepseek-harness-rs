import 'dart:typed_data';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/primitives.dart';
import '../../design/error.dart';
import '../../design/typography.dart';
import '../workbench/workbench_panel.dart' show previewUrl;

class ArtifactDiffLine {
  const ArtifactDiffLine(this.kind, this.text, this.beforeLine, this.afterLine);
  final String kind, text;
  final int? beforeLine, afterLine;
}

/// Trim unchanged edges before a bounded line comparison; large changed blocks
/// retain every removed/added line without allocating a quadratic matrix.
List<ArtifactDiffLine> artifactDiffLines(String? before, String? after) {
  List<String> lines(String? value) {
    if (value == null || value.isEmpty) return [];
    final result = value.split(RegExp(r'\r?\n'));
    if (value.endsWith('\n')) result.removeLast();
    return result;
  }

  final a = lines(before), b = lines(after);
  if (a.length > 20000 || b.length > 20000) return [];
  final result = <ArtifactDiffLine>[];
  var prefix = 0, suffix = 0;
  while (prefix < a.length && prefix < b.length && a[prefix] == b[prefix]) {
    result.add(ArtifactDiffLine('same', a[prefix], prefix + 1, prefix + 1));
    prefix++;
  }
  while (suffix < a.length - prefix &&
      suffix < b.length - prefix &&
      a[a.length - suffix - 1] == b[b.length - suffix - 1]) {
    suffix++;
  }
  final n = a.length - prefix - suffix, m = b.length - prefix - suffix;
  var x = 0, y = 0;
  if (n * m <= 1000000) {
    final width = m + 1;
    final matrix = Uint16List((n + 1) * width);
    for (var i = n - 1; i >= 0; i--) {
      for (var j = m - 1; j >= 0; j--) {
        final down = matrix[(i + 1) * width + j],
            right = matrix[i * width + j + 1];
        matrix[i * width + j] = a[prefix + i] == b[prefix + j]
            ? matrix[(i + 1) * width + j + 1] + 1
            : (down > right ? down : right);
      }
    }
    while (x < n && y < m) {
      if (a[prefix + x] == b[prefix + y]) {
        result.add(
          ArtifactDiffLine(
            'same',
            a[prefix + x],
            prefix + x + 1,
            prefix + y + 1,
          ),
        );
        x++;
        y++;
      } else if (matrix[(x + 1) * width + y] >= matrix[x * width + y + 1]) {
        result.add(
          ArtifactDiffLine('removed', a[prefix + x], prefix + x + 1, null),
        );
        x++;
      } else {
        result.add(
          ArtifactDiffLine('added', b[prefix + y], null, prefix + y + 1),
        );
        y++;
      }
    }
  }
  while (x < n) {
    result.add(
      ArtifactDiffLine('removed', a[prefix + x], prefix + x + 1, null),
    );
    x++;
  }
  while (y < m) {
    result.add(ArtifactDiffLine('added', b[prefix + y], null, prefix + y + 1));
    y++;
  }
  for (var i = suffix; i > 0; i--) {
    result.add(
      ArtifactDiffLine(
        'same',
        a[a.length - i],
        a.length - i + 1,
        b.length - i + 1,
      ),
    );
  }
  return result;
}

class ArtifactChangesView extends StatefulWidget {
  const ArtifactChangesView({
    super.key,
    required this.api,
    required this.session,
    this.initialPath,
    this.onClose,
  });
  final DshClient api;
  final String session;
  final String? initialPath;
  final VoidCallback? onClose;
  @override
  State<ArtifactChangesView> createState() => _ArtifactChangesViewState();
}

class _ArtifactChangesViewState extends State<ArtifactChangesView> {
  RequestScope scope = RequestScope();
  Json data = {};
  Object? error;
  bool loading = true;
  int generation = 0;
  String? selected;
  List<ArtifactDiffLine> diff = [];
  List<Json> get files => objects(data['files']);
  Json? get file => files.where((row) => row['path'] == selected).firstOrNull;
  @override
  void initState() {
    super.initState();
    selected = widget.initialPath;
    load();
  }

  @override
  void didUpdateWidget(ArtifactChangesView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.api, widget.api) ||
        oldWidget.session != widget.session) {
      data = {};
      diff = [];
      selected = widget.initialPath;
      load();
    }
  }

  @override
  void dispose() {
    scope.cancel();
    super.dispose();
  }

  void select(String? path) {
    selected = path;
    final row = file;
    diff = row == null || row['reason'] != null
        ? []
        : artifactDiffLines(row['before'] as String?, row['after'] as String?);
  }

  Future<void> load() async {
    scope.cancel();
    final request = scope = RequestScope();
    final epoch = ++generation;
    setState(() {
      loading = true;
      error = null;
    });
    try {
      final result = await widget.api.request(
        previewUrl('turn-changes', widget.session),
        scope: request,
        maxBytes: 6 * 1024 * 1024,
      );
      if (!mounted || epoch != generation || request.cancelled) return;
      setState(() {
        data = result;
        selected ??= files.firstOrNull?['path'] as String?;
        select(selected);
      });
    } catch (e) {
      if (mounted && epoch == generation) setState(() => error = e);
    } finally {
      if (mounted && epoch == generation) setState(() => loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context), tokens = DshTokens.of(context);
    final row = file;
    final added = diff.where((line) => line.kind == 'added').length;
    final removed = diff.where((line) => line.kind == 'removed').length;
    final before = row?['before'], after = row?['after'];
    final newlineChanged =
        before is String &&
        after is String &&
        (before.endsWith('\n') != after.endsWith('\n') ||
            before.contains('\r\n') != after.contains('\r\n'));
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(child: Text('最近完成回合', style: DshTypography.body)),
            DshIcon(
              DshIcons.refreshCw.data,
              label: '刷新回合改动',
              onPressed: loading ? null : load,
            ),
            if (widget.onClose != null)
              DshIcon(
                DshIcons.close.data,
                label: '关闭差异',
                onPressed: widget.onClose,
              ),
          ],
        ),
        Text(
          '回合开始 → 回合结束',
          style: DshTypography.caption.copyWith(color: colors.muted),
        ),
        if (loading) const LinearProgressIndicator(minHeight: 1),
        if (error != null) DshErrorView(error: error!, onRetry: load),
        if (data['error'] != null)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 10),
            child: Text('${data['error']}', style: DshTypography.body),
          ),
        if (data['incomplete'] == true)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Text(
              '快照范围不完整；只展示具有有效前后记录的差异。',
              style: DshTypography.caption,
            ),
          ),
        if (files.isNotEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: DropdownButton<String>(
              key: const ValueKey('artifact-diff-file'),
              icon: DshGlyph(DshIcons.chevronDown.data, size: 16),
              isExpanded: true,
              value: files.any((f) => f['path'] == selected) ? selected : null,
              hint: const Text('选择文件'),
              items: [
                for (final f in files)
                  DropdownMenuItem(
                    value: '${f['path']}',
                    child: Text(
                      '${f['path']}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: DshTypography.body,
                    ),
                  ),
              ],
              onChanged: (value) => setState(() => select(value)),
            ),
          ),
        if (row != null && row['reason'] == null)
          Row(
            children: [
              Text(
                '+$added',
                style: DshTypography.caption.copyWith(
                  color: tokens.success.foreground,
                ),
              ),
              const SizedBox(width: 10),
              Text(
                '−$removed',
                style: DshTypography.caption.copyWith(
                  color: tokens.error.foreground,
                ),
              ),
              const Spacer(),
              DshButton(
                height: 28,
                onPressed: diff.isEmpty
                    ? null
                    : () => Clipboard.setData(
                        ClipboardData(
                          text: diff
                              .map(
                                (line) =>
                                    '${line.kind == 'added'
                                        ? '+'
                                        : line.kind == 'removed'
                                        ? '-'
                                        : ' '}${line.text}',
                              )
                              .join('\n'),
                        ),
                      ),
                child: const Text('复制差异'),
              ),
            ],
          ),
        if (newlineChanged)
          Text('换行格式或文件末尾换行发生变化', style: DshTypography.caption),
        const SizedBox(height: 6),
        Expanded(
          child: row == null
              ? DshEmpty(
                  loading
                      ? '正在读取回合差异…'
                      : widget.initialPath != null
                      ? '最近完成回合中没有此文件的前后记录'
                      : '尚无已完成回合的文件改动',
                )
              : row['reason'] != null
              ? DshEmpty('${row['reason']}')
              : diff.isEmpty
              ? DshEmpty(
                  row['kind'] == 'added' && after == ''
                      ? '新增空文件'
                      : row['kind'] == 'deleted' && before == ''
                      ? '已删除空文件'
                      : '没有可展示的文本差异，或文件超过预览行数上限',
                )
              : SelectionArea(
                  child: ListView.builder(
                    key: ValueKey('artifact-diff-$selected'),
                    itemCount: diff.length,
                    itemBuilder: (context, i) {
                      final line = diff[i];
                      return ColoredBox(
                        color: line.kind == 'added'
                            ? tokens.success.background
                            : line.kind == 'removed'
                            ? tokens.error.background
                            : colors.base,
                        child: Padding(
                          padding: const EdgeInsets.symmetric(
                            vertical: 2,
                            horizontal: 4,
                          ),
                          child: Row(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              SizedBox(
                                width: 34,
                                child: Text(
                                  '${line.beforeLine ?? ''}',
                                  textAlign: TextAlign.right,
                                  style: DshTypography.caption.copyWith(
                                    color: colors.muted,
                                  ),
                                ),
                              ),
                              SizedBox(
                                width: 34,
                                child: Text(
                                  '${line.afterLine ?? ''}',
                                  textAlign: TextAlign.right,
                                  style: DshTypography.caption.copyWith(
                                    color: colors.muted,
                                  ),
                                ),
                              ),
                              const SizedBox(width: 8),
                              SizedBox(
                                width: 14,
                                child: Text(
                                  line.kind == 'added'
                                      ? '+'
                                      : line.kind == 'removed'
                                      ? '−'
                                      : ' ',
                                  style: DshTypography.code,
                                ),
                              ),
                              Expanded(
                                child: Text(
                                  line.text,
                                  style: DshTypography.code.copyWith(
                                    color: colors.text,
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      );
                    },
                  ),
                ),
        ),
      ],
    );
  }
}
