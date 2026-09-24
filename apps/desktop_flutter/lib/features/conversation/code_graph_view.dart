import 'dart:math' as math;

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';

import '../../design/primitives.dart';
import 'artifacts_view.dart' show ArtifactPreview;
import 'code_graph_controller.dart';

const graphPalette = [
  Color(0xff7668cd),
  Color(0xff479cab),
  Color(0xffb38659),
  Color(0xff678cba),
  Color(0xffa16c93),
];
const graphStatuses = {
  'queued': '等待索引',
  'indexing': '正在索引',
  'checking': '检查变更',
  'ready': '索引就绪',
  'partial': '部分索引',
  'cancelled': '索引已暂停',
  'failed': '索引失败',
};

class CodeGraphView extends StatefulWidget {
  const CodeGraphView({super.key, required this.api, required this.session});
  final DshClient api;
  final String session;
  @override
  State<CodeGraphView> createState() => _CodeGraphViewState();
}

class _CodeGraphViewState extends State<CodeGraphView>
    with WidgetsBindingObserver {
  late final c = CodeGraphController(widget.api, widget.session);
  final search = TextEditingController(), stageFocus = FocusNode();
  final positions = <String, Offset>{};
  Offset camera = const Offset(30, 30);
  double scale = 1;
  Size stage = const Size(800, 500);
  String signature = '';
  CodeNode? selectedNode;
  CodeLink? selectedEdge;
  bool results = false,
      inferred = true,
      overFloating = false,
      foreground = true;
  String? get selection => selectedNode?.id ?? selectedEdge?.id;
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    foreground =
        WidgetsBinding.instance.lifecycleState == null ||
        WidgetsBinding.instance.lifecycleState == AppLifecycleState.resumed;
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final enabled = foreground && TickerMode.valuesOf(context).enabled;
    if (c.active != enabled) {
      c.setActive(enabled);
    } else if (c.graph.isEmpty && !c.loading && enabled) {
      c.refresh();
    }
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    foreground = state == AppLifecycleState.resumed;
    c.setActive(foreground && TickerMode.valuesOf(context).enabled);
    if (foreground && selection != null) {
      c.readSource(
        selectedNode?.path ?? selectedEdge?.path,
        selectedNode?.line ?? selectedEdge?.line ?? 1,
      );
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    c.dispose();
    search.dispose();
    stageFocus.dispose();
    positions.clear();
    super.dispose();
  }

  Offset position(String id) {
    final i = c.model.nodes.indexWhere((n) => n.id == id);
    return positions[id] ??
        Offset(
          60 + (i % (c.model.nodes.length <= 4 ? 2 : 3)) * 310,
          70 + (i ~/ (c.model.nodes.length <= 4 ? 2 : 3)) * 145,
        );
  }

  Rect bounds() {
    if (c.model.nodes.isEmpty) return const Rect.fromLTWH(0, 0, 800, 500);
    final points = c.model.nodes.map((n) => position(n.id));
    final minX = points.map((p) => p.dx).reduce(math.min),
        minY = points.map((p) => p.dy).reduce(math.min),
        maxX = points.map((p) => p.dx).reduce(math.max),
        maxY = points.map((p) => p.dy).reduce(math.max);
    return Rect.fromLTWH(
      minX - 30,
      minY - 30,
      maxX - minX + 296,
      maxY - minY + 152,
    );
  }

  void fit() {
    if (!mounted) return;
    final b = bounds(),
        z = math.max(
          .25,
          math.min(
            1.1,
            math.min(
              (stage.width - 64) / b.width,
              (stage.height - 100) / b.height,
            ),
          ),
        );
    setState(() {
      scale = z;
      camera = Offset(
        (stage.width - b.width * z) / 2 - b.left * z,
        (stage.height - b.height * z) / 2 - b.top * z - 12,
      );
    });
  }

  void zoom(double factor, [Offset? point]) {
    final at = point ?? Offset(stage.width / 2, stage.height / 2),
        next = (scale * factor).clamp(.25, 2.5);
    setState(() {
      camera = at - (at - camera) * (next / scale);
      scale = next;
    });
  }

  void choose(CodeNode node) {
    setState(() {
      selectedNode = node;
      selectedEdge = null;
      results = false;
    });
    c.readSource(node.path, node.line);
  }

  void chooseEdge(CodeLink edge) {
    setState(() {
      selectedEdge = edge;
      selectedNode = null;
      results = false;
    });
    c.readSource(edge.path, edge.line);
  }

  void clearSelection() {
    setState(() {
      selectedNode = null;
      selectedEdge = null;
      results = false;
    });
    c.readSource(null, 1);
  }

  void focusNode(CodeNode node, {String? relation}) {
    positions.clear();
    choose(node);
    c.focusNode(node, relation: relation);
    search.text = c.query;
  }

  Future<void> open(String path, int line) async {
    if (path.isEmpty) return;
    await showDialog<void>(
      context: context,
      builder: (_) => ArtifactPreview(
        api: widget.api,
        session: widget.session,
        path: path,
        line: line,
      ),
    );
  }

  Widget button(
    String title,
    VoidCallback? action, {
    IconData? icon,
    bool active = false,
  }) => DshButton(
    height: 32,
    fontSize: 12,
    padding: const EdgeInsets.symmetric(horizontal: 10),
    outline: true,
    onPressed: action,
    child: Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (icon != null) ...[
          DshGlyph(
            icon,
            asset: {
              LucideIcons.refreshCw: 'assets/icons/cg-refresh.svg',
              LucideIcons.pause: 'assets/icons/cg-pause.svg',
              LucideIcons.code: 'assets/icons/cg-code.svg',
            }[icon],
            size: 15,
          ),
          const SizedBox(width: 6),
        ],
        Text(
          title,
          style: TextStyle(color: active ? DshColors(context).blue : null),
        ),
      ],
    ),
  );
  Widget floating(
    Widget child, {
    EdgeInsets padding = const EdgeInsets.all(8),
  }) => MouseRegion(
    onEnter: (_) => overFloating = true,
    onExit: (_) => overFloating = false,
    child: Container(
      padding: padding,
      decoration: BoxDecoration(
        color: DshColors(context).dark ? const Color(0xff232324) : Colors.white,
        border: Border.all(color: DshColors(context).border),
        borderRadius: BorderRadius.circular(12),
        boxShadow: const [
          BoxShadow(
            color: Color(0x0b282238),
            blurRadius: 22,
            offset: Offset(0, 5),
          ),
        ],
      ),
      child: child,
    ),
  );
  Widget segments(
    Map<String, String> values,
    String selected,
    void Function(String) pick,
  ) => Container(
    padding: const EdgeInsets.all(3),
    decoration: BoxDecoration(
      color: DshColors(context).layer,
      borderRadius: BorderRadius.circular(10),
    ),
    child: Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final v in values.entries)
          ShadButton.ghost(
            height: 28,
            padding: const EdgeInsets.symmetric(horizontal: 10),
            backgroundColor: v.key == selected ? DshColors(context).base : null,
            onPressed: () => pick(v.key),
            child: Text(
              v.value,
              style: TextStyle(
                fontSize: 12,
                color: v.key == selected
                    ? DshColors(context).blue
                    : DshColors(context).muted,
              ),
            ),
          ),
      ],
    ),
  );
  Path edgePath(CodeLink edge) {
    final a = position(edge.source),
        b = position(edge.target),
        forward = a.dx < b.dx,
        x1 = a.dx + (forward ? 236 : 0),
        x2 = b.dx + (forward ? 0 : 236),
        bend = math.max(60, (x2 - x1).abs() * .5);
    return Path()
      ..moveTo(x1, a.dy + 46)
      ..cubicTo(
        x1 + (forward ? bend : -bend),
        a.dy + 46,
        x2 - (forward ? bend : -bend),
        b.dy + 46,
        x2,
        b.dy + 46,
      );
  }

  void hitEdge(Offset local, List<CodeLink> edges) {
    final p = (local - camera) / scale;
    for (final edge in edges.reversed) {
      for (final metric in edgePath(edge).computeMetrics()) {
        for (var i = 0; i <= 32; i++) {
          final tangent = metric.getTangentForOffset(metric.length * i / 32);
          if (tangent != null && (tangent.position - p).distance < 8 / scale) {
            chooseEdge(edge);
            return;
          }
        }
      }
    }
    clearSelection();
  }

  Widget inspector(List<CodeLink> edges) {
    final colors = DshColors(context), node = selectedNode, edge = selectedEdge;
    final path = node?.path ?? edge?.path ?? '',
        line = node?.line ?? edge?.line ?? 1;
    final neighbors = {
      for (final e in edges.where(
        (e) => e.source == node?.id || e.target == node?.id,
      ))
        e.source == node?.id ? e.target : e.source,
    };
    return floating(
      SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    edge == null ? '节点详情' : '关系详情',
                    style: TextStyle(fontSize: 11, color: colors.muted),
                  ),
                ),
                DshIcon(
                  LucideIcons.x,
                  label: '关闭节点详情',
                  size: 24,
                  onPressed: clearSelection,
                ),
              ],
            ),
            const SizedBox(height: 12),
            Text(
              node?.name ??
                  (edge!.name.isEmpty
                      ? '${edge.source} → ${edge.target}'
                      : edge.name),
              style: const TextStyle(
                fontFamily: 'Consolas',
                fontSize: 16,
                fontWeight: FontWeight.w600,
              ),
            ),
            const SizedBox(height: 6),
            Text(
              '$path:$line',
              style: TextStyle(fontSize: 11, color: colors.muted),
            ),
            if (edge != null)
              Text(
                '${edge.inferred
                    ? '名称推断'
                    : edge.kind == 'import'
                    ? '文件导入'
                    : '静态代码关联'} · ${edge.count} 处记录（显示首处）',
                style: TextStyle(fontSize: 11, color: colors.muted),
              ),
            const SizedBox(height: 12),
            Wrap(
              spacing: 7,
              runSpacing: 7,
              children: [
                button('打开源码', () => open(path, line), icon: LucideIcons.code),
                if (node != null) button('聚焦关系', () => focusNode(node)),
                if (node?.kind == 'file')
                  button('查看符号', () {
                    clearSelection();
                    positions.clear();
                    c.symbols(node!);
                    search.text = c.query;
                    setState(() => results = true);
                  }),
              ],
            ),
            if (!c.files && node != null)
              Padding(
                padding: const EdgeInsets.only(top: 12),
                child: Wrap(
                  spacing: 4,
                  runSpacing: 4,
                  children: [
                    for (final value in const {
                      'callers': '调用者',
                      'callees': '被调用者',
                      'blast': '影响范围',
                    }.entries)
                      button(
                        value.value,
                        () => focusNode(node, relation: value.key),
                        active: c.direction == value.key,
                      ),
                  ],
                ),
              ),
            const SizedBox(height: 16),
            const Text(
              '源码片段',
              style: TextStyle(fontSize: 11, fontWeight: FontWeight.w600),
            ),
            const SizedBox(height: 7),
            Container(
              padding: const EdgeInsets.all(10),
              decoration: BoxDecoration(
                color: colors.layer,
                borderRadius: BorderRadius.circular(8),
              ),
              constraints: const BoxConstraints(maxHeight: 180),
              child: SingleChildScrollView(
                child: SelectableText(
                  c.snippet.isEmpty ? '正在读取源码…' : c.snippet,
                  style: TextStyle(
                    fontFamily: 'Consolas',
                    fontSize: 11,
                    height: 1.75,
                    color: colors.muted,
                  ),
                ),
              ),
            ),
            if (node != null) ...[
              const SizedBox(height: 16),
              Text(
                '画布中的相邻节点 · ${neighbors.length}',
                style: TextStyle(fontSize: 11, color: colors.muted),
              ),
              for (final n in c.model.nodes.where(
                (n) => neighbors.contains(n.id),
              ))
                DshButton(
                  height: 30,
                  fontSize: 12,
                  onPressed: () => choose(n),
                  child: Text(
                    n.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              if (neighbors.isEmpty)
                Text(
                  '当前范围内没有已解析的关联。',
                  style: TextStyle(fontSize: 11, color: colors.muted),
                ),
            ],
          ],
        ),
      ),
      padding: const EdgeInsets.all(16),
    );
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: c,
    builder: (context, _) {
      final colors = DshColors(context),
          edges = c.model.edges.where((e) => inferred || !e.inferred).toList(),
          neighbors = {if (selectedNode != null) selectedNode!.id};
      if (selectedNode != null) {
        for (final edge in edges) {
          if (edge.source == selectedNode!.id ||
              edge.target == selectedNode!.id) {
            neighbors.add(edge.source);
            neighbors.add(edge.target);
          }
        }
      }
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: const EdgeInsets.fromLTRB(20, 16, 20, 12),
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: colors.border)),
            ),
            child: Row(
              children: [
                Container(
                  width: 38,
                  height: 38,
                  decoration: BoxDecoration(
                    color: colors.blue.withValues(alpha: .10),
                    borderRadius: BorderRadius.circular(12),
                  ),
                  child: DshGlyph(
                    LucideIcons.workflow,
                    asset: 'assets/icons/cg-graph.svg',
                    size: 23,
                    color: colors.blue,
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text(
                        '代码画布',
                        style: TextStyle(
                          fontSize: 17,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      Text(
                        '从结构到细节，沿着关系探索代码',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: colors.muted),
                      ),
                    ],
                  ),
                ),
                for (final stat in {
                  'files': '索引文件',
                  'totalSymbols': '代码符号',
                  'totalCalls': '静态关系',
                }.entries)
                  Padding(
                    padding: const EdgeInsets.only(left: 12),
                    child: Column(
                      children: [
                        Text(
                          '${c.graph[stat.key] ?? '—'}',
                          style: const TextStyle(
                            fontSize: 16,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                        Text(
                          stat.value,
                          style: TextStyle(fontSize: 10, color: colors.muted),
                        ),
                      ],
                    ),
                  ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
            child: Wrap(
              spacing: 10,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                SizedBox(
                  width: 260,
                  child: TextField(
                    controller: search,
                    onTap: () => setState(() => results = true),
                    onChanged: (value) {
                      clearSelection();
                      positions.clear();
                      c.search(value);
                      setState(() => results = true);
                    },
                    inputFormatters: [LengthLimitingTextInputFormatter(500)],
                    style: const TextStyle(fontSize: 13),
                    decoration: InputDecoration(
                      hintText: c.files ? '搜索文件或路径…' : '搜索函数、类型或路径…',
                      isDense: true,
                      prefixIcon: const DshGlyph(LucideIcons.search, size: 15),
                      prefixIconConstraints: const BoxConstraints.tightFor(
                        width: 33,
                        height: 36,
                      ),
                      contentPadding: const EdgeInsets.symmetric(
                        horizontal: 10,
                        vertical: 9,
                      ),
                      border: OutlineInputBorder(
                        borderRadius: BorderRadius.circular(10),
                        borderSide: BorderSide(color: colors.border),
                      ),
                    ),
                  ),
                ),
                segments(
                  const {'files': '文件依赖', 'calls': '符号调用'},
                  c.files ? 'files' : 'calls',
                  (value) {
                    clearSelection();
                    positions.clear();
                    search.clear();
                    c.mode(value == 'files');
                  },
                ),
                button(
                  '推断关系',
                  () => setState(() => inferred = !inferred),
                  active: inferred,
                ),
                button(
                  '更新',
                  () => c.refresh(resume: true),
                  icon: LucideIcons.refreshCw,
                ),
                if (c.indexing) button('暂停', c.pause, icon: LucideIcons.pause),
              ],
            ),
          ),
          if (c.error != null || c.graph['error'] != null)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 5),
              child: Text(
                c.error ?? '${c.graph['error']}',
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(color: Colors.red, fontSize: 11),
              ),
            ),
          Expanded(
            child: LayoutBuilder(
              builder: (context, box) {
                final size = Size(box.maxWidth, box.maxHeight),
                    next = c.model.nodes.map((n) => n.id).join('|');
                if (next != signature || size != stage) {
                  signature = next;
                  stage = size;
                  final ids = c.model.nodes.map((n) => n.id).toSet();
                  positions.removeWhere((id, _) => !ids.contains(id));
                  WidgetsBinding.instance.addPostFrameCallback((_) => fit());
                }
                final small = stage.width < 760,
                    miniSize = Size(small ? 115 : 146, small ? 80 : 100),
                    graphBounds = bounds();
                return Focus(
                  focusNode: stageFocus,
                  onKeyEvent: (_, event) {
                    if (event is! KeyDownEvent) return KeyEventResult.ignored;
                    final key = event.logicalKey;
                    if (key == LogicalKeyboardKey.digit0 ||
                        key == LogicalKeyboardKey.home) {
                      fit();
                      return KeyEventResult.handled;
                    }
                    if (key == LogicalKeyboardKey.add ||
                        key == LogicalKeyboardKey.equal) {
                      zoom(1.2);
                      return KeyEventResult.handled;
                    }
                    if (key == LogicalKeyboardKey.minus) {
                      zoom(1 / 1.2);
                      return KeyEventResult.handled;
                    }
                    if (key == LogicalKeyboardKey.escape) {
                      clearSelection();
                      return KeyEventResult.handled;
                    }
                    final delta = {
                      LogicalKeyboardKey.arrowLeft: const Offset(40, 0),
                      LogicalKeyboardKey.arrowRight: const Offset(-40, 0),
                      LogicalKeyboardKey.arrowUp: const Offset(0, 40),
                      LogicalKeyboardKey.arrowDown: const Offset(0, -40),
                    }[key];
                    if (delta != null) {
                      setState(() => camera += delta);
                      return KeyEventResult.handled;
                    }
                    return KeyEventResult.ignored;
                  },
                  child: Semantics(
                    label: '代码关系画布：拖动平移，滚轮缩放，方向键平移，0 适应画布',
                    child: Listener(
                      onPointerSignal: (event) {
                        if (event is PointerScrollEvent && !overFloating) {
                          GestureBinding.instance.pointerSignalResolver
                              .register(
                                event,
                                (_) => zoom(
                                  math.exp(-event.scrollDelta.dy * .0015),
                                  event.localPosition,
                                ),
                              );
                        }
                      },
                      child: ClipRect(
                        child: Stack(
                          children: [
                            Positioned.fill(
                              child: GestureDetector(
                                key: const ValueKey('code-graph-stage'),
                                behavior: HitTestBehavior.opaque,
                                onTapDown: (_) => stageFocus.requestFocus(),
                                onTapUp: (event) =>
                                    hitEdge(event.localPosition, edges),
                                onPanStart: (_) {
                                  stageFocus.requestFocus();
                                  setState(() => results = false);
                                },
                                onPanUpdate: (event) =>
                                    setState(() => camera += event.delta),
                                child: CustomPaint(
                                  painter: _GraphPainter(
                                    edges
                                        .map(
                                          (e) => (edge: e, path: edgePath(e)),
                                        )
                                        .toList(),
                                    camera,
                                    scale,
                                    selection,
                                    selectedNode != null,
                                    colors,
                                  ),
                                ),
                              ),
                            ),
                            for (final node in c.model.nodes)
                              Positioned(
                                left: camera.dx + position(node.id).dx * scale,
                                top: camera.dy + position(node.id).dy * scale,
                                width: 236 * scale,
                                height: 92 * scale,
                                child: GestureDetector(
                                  key: ValueKey('graph-node-${node.id}'),
                                  onTap: () => choose(node),
                                  onDoubleTap: () => open(node.path, node.line),
                                  onPanUpdate: (event) => setState(() {
                                    final next =
                                        position(node.id) + event.delta / scale;
                                    positions[node.id] = Offset(
                                      next.dx.clamp(-10000, 10000),
                                      next.dy.clamp(-10000, 10000),
                                    );
                                  }),
                                  child: Semantics(
                                    button: true,
                                    label:
                                        '${node.name}，${displayPath(node.path)}:${node.line}',
                                    selected: selectedNode?.id == node.id,
                                    child: Opacity(
                                      opacity:
                                          selectedNode != null &&
                                              !neighbors.contains(node.id)
                                          ? .38
                                          : 1.0,
                                      child: FittedBox(
                                        fit: BoxFit.fill,
                                        child: _CodeNodeCard(
                                          node: node,
                                          selected: selectedNode?.id == node.id,
                                          colors: colors,
                                        ),
                                      ),
                                    ),
                                  ),
                                ),
                              ),
                            Positioned(
                              top: 14,
                              left: 16,
                              child: floating(
                                Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    Text(
                                      c.focus.isEmpty ? '工作区局部视图' : '聚焦视图',
                                      style: TextStyle(
                                        fontSize: 11,
                                        color: colors.muted,
                                      ),
                                    ),
                                    if (c.focus.isNotEmpty)
                                      DshButton(
                                        height: 22,
                                        fontSize: 11,
                                        onPressed: () {
                                          clearSelection();
                                          positions.clear();
                                          search.clear();
                                          c.overview();
                                        },
                                        child: const Text('返回概览'),
                                      ),
                                    const SizedBox(width: 8),
                                    Text(
                                      '${c.model.nodes.length} 节点 · ${edges.length} 关联',
                                      style: const TextStyle(fontSize: 11),
                                    ),
                                  ],
                                ),
                              ),
                            ),
                            if (results)
                              Positioned(
                                left: 18,
                                top: 14,
                                bottom: 70,
                                width: math.min(260, stage.width - 36),
                                child: floating(
                                  Column(
                                    crossAxisAlignment:
                                        CrossAxisAlignment.start,
                                    children: [
                                      Text(
                                        search.text.isEmpty
                                            ? '选择入口以聚焦关系'
                                            : '搜索结果',
                                        style: TextStyle(
                                          fontSize: 11,
                                          color: colors.muted,
                                        ),
                                      ),
                                      const SizedBox(height: 7),
                                      Expanded(
                                        child: c.model.catalog.isEmpty
                                            ? Text(
                                                c.indexing
                                                    ? '正在索引…'
                                                    : '没有匹配的文件或符号',
                                                style: TextStyle(
                                                  color: colors.muted,
                                                  fontSize: 11,
                                                ),
                                              )
                                            : ListView.builder(
                                                itemCount: c
                                                    .model
                                                    .catalog
                                                    .length
                                                    .clamp(0, 60),
                                                itemExtent: 52,
                                                itemBuilder: (context, index) {
                                                  final node =
                                                      c.model.catalog[index];
                                                  return InkWell(
                                                    onTap: () =>
                                                        focusNode(node),
                                                    child: Padding(
                                                      padding:
                                                          const EdgeInsets.all(
                                                            6,
                                                          ),
                                                      child: Column(
                                                        crossAxisAlignment:
                                                            CrossAxisAlignment
                                                                .start,
                                                        children: [
                                                          Text(
                                                            node.name,
                                                            maxLines: 1,
                                                            overflow:
                                                                TextOverflow
                                                                    .ellipsis,
                                                            style:
                                                                const TextStyle(
                                                                  fontFamily: 'Consolas',
                                                                  fontSize: 12,
                                                                ),
                                                          ),
                                                          Text(
                                                            '${displayPath(node.path)}:${node.line}',
                                                            maxLines: 1,
                                                            overflow:
                                                                TextOverflow
                                                                    .ellipsis,
                                                            style: TextStyle(
                                                              fontSize: 10,
                                                              color:
                                                                  colors.muted,
                                                            ),
                                                          ),
                                                        ],
                                                      ),
                                                    ),
                                                  );
                                                },
                                              ),
                                      ),
                                    ],
                                  ),
                                ),
                              ),
                            if (selection != null)
                              Positioned(
                                top: 14,
                                right: 16,
                                bottom: stage.height > 300
                                    ? miniSize.height + 32
                                    : 56,
                                width: math.min(
                                  small ? 230 : 270,
                                  stage.width - 32,
                                ),
                                child: inspector(edges),
                              ),
                            if (c.model.nodes.isEmpty)
                              Positioned.fill(
                                child: IgnorePointer(
                                  child: Center(
                                    child: Padding(
                                      padding: const EdgeInsets.all(35),
                                      child: Column(
                                        mainAxisSize: MainAxisSize.min,
                                        children: [
                                          DshGlyph(
                                            LucideIcons.workflow,
                                            asset: 'assets/icons/cg-graph.svg',
                                            size: 40,
                                            color: colors.muted,
                                          ),
                                          const SizedBox(height: 10),
                                          Text(
                                            c.indexing
                                                ? '正在构建代码地图'
                                                : search.text.isEmpty
                                                ? '当前工作区暂无代码关系'
                                                : '没有找到匹配项',
                                            style: TextStyle(
                                              fontSize: 15,
                                              color: colors.muted,
                                            ),
                                          ),
                                          const SizedBox(height: 8),
                                          Text(
                                            c.indexing
                                                ? '索引在后台运行，可以继续使用会话。'
                                                : '搜索文件或函数名称，找到探索的起点。',
                                            textAlign: TextAlign.center,
                                            style: TextStyle(
                                              fontSize: 11,
                                              color: colors.muted,
                                            ),
                                          ),
                                        ],
                                      ),
                                    ),
                                  ),
                                ),
                              ),
                            Positioned(
                              bottom: 18,
                              left: 18,
                              child: floating(
                                Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    DshIcon(
                                      LucideIcons.minus,
                                      label: '缩小画布',
                                      asset: 'assets/icons/cg-minus.svg',
                                      size: 30,
                                      onPressed: () => zoom(1 / 1.2),
                                    ),
                                    Text(
                                      '${(scale * 100).round()}%',
                                      style: const TextStyle(fontSize: 11),
                                    ),
                                    DshIcon(
                                      LucideIcons.plus,
                                      label: '放大画布',
                                      asset: 'assets/icons/cg-plus.svg',
                                      size: 30,
                                      onPressed: () => zoom(1.2),
                                    ),
                                    DshIcon(
                                      LucideIcons.maximize,
                                      label: '适应画布',
                                      asset: 'assets/icons/cg-fit.svg',
                                      size: 30,
                                      onPressed: fit,
                                    ),
                                    DshIcon(
                                      LucideIcons.layoutGrid,
                                      label: '重新布局',
                                      asset: 'assets/icons/cg-layout.svg',
                                      size: 30,
                                      onPressed: () {
                                        positions.clear();
                                        fit();
                                      },
                                    ),
                                  ],
                                ),
                                padding: const EdgeInsets.all(5),
                              ),
                            ),
                            if (c.model.nodes.isNotEmpty && stage.width > 400)
                              Positioned(
                                bottom: 18,
                                right: 18,
                                width: miniSize.width,
                                height: miniSize.height,
                                child: floating(
                                  LayoutBuilder(
                                    builder: (context, mini) => GestureDetector(
                                      key: const ValueKey('code-graph-minimap'),
                                      onTapUp: (event) {
                                        final area = Size(
                                          mini.maxWidth,
                                          mini.maxHeight,
                                        );
                                        final s = math.min(
                                          area.width / graphBounds.width,
                                          area.height / graphBounds.height,
                                        );
                                        final offset = Offset(
                                          (area.width - graphBounds.width * s) /
                                              2,
                                          (area.height -
                                                  graphBounds.height * s) /
                                              2,
                                        );
                                        final world =
                                            (event.localPosition - offset) / s +
                                            graphBounds.topLeft;
                                        setState(
                                          () => camera =
                                              Offset(
                                                stage.width / 2,
                                                stage.height / 2,
                                              ) -
                                              world * scale,
                                        );
                                      },
                                      child: CustomPaint(
                                        size: Size(
                                          mini.maxWidth,
                                          mini.maxHeight,
                                        ),
                                        painter: _MiniMap(
                                          c.model.nodes
                                              .map((n) => position(n.id))
                                              .toList(),
                                          graphBounds,
                                          Rect.fromLTWH(
                                            -camera.dx / scale,
                                            -camera.dy / scale,
                                            stage.width / scale,
                                            stage.height / scale,
                                          ),
                                          colors.blue,
                                        ),
                                      ),
                                    ),
                                  ),
                                ),
                              ),
                          ],
                        ),
                      ),
                    ),
                  ),
                );
              },
            ),
          ),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 7),
            decoration: BoxDecoration(
              border: Border(top: BorderSide(color: colors.border)),
            ),
            child: Wrap(
              spacing: 14,
              runSpacing: 3,
              children: [
                Text(
                  '实线：静态关联',
                  style: TextStyle(fontSize: 10, color: colors.muted),
                ),
                Text(
                  '虚线：名称推断',
                  style: TextStyle(fontSize: 10, color: colors.muted),
                ),
                Text(
                  '${graphStatuses[c.status] ?? c.status}${c.model.limited || c.graph['resultLimited'] == true ? ' · 局部展示' : ''}',
                  style: TextStyle(fontSize: 10, color: colors.muted),
                ),
              ],
            ),
          ),
          if (object(c.graph['stats'])['reasons'] is List &&
              (object(c.graph['stats'])['reasons'] as List).isNotEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20),
              child: Text(
                (object(c.graph['stats'])['reasons'] as List).join('；'),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 10, color: colors.muted),
              ),
            ),
        ],
      );
    },
  );
}

class _CodeNodeCard extends StatelessWidget {
  const _CodeNodeCard({
    required this.node,
    required this.selected,
    required this.colors,
  });
  final CodeNode node;
  final bool selected;
  final DshColors colors;
  @override
  Widget build(BuildContext context) {
    final color = graphPalette[node.colorIndex];
    return Container(
      width: 236,
      height: 92,
      padding: const EdgeInsets.symmetric(horizontal: 15, vertical: 12),
      decoration: BoxDecoration(
        color: colors.dark ? const Color(0xff232324) : Colors.white,
        border: Border.all(color: selected ? colors.blue : colors.border),
        borderRadius: BorderRadius.circular(13),
        boxShadow: [
          BoxShadow(
            color: selected
                ? colors.blue.withValues(alpha: .12)
                : const Color(0x08222239),
            blurRadius: selected ? 8 : 16,
            spreadRadius: selected ? 2 : 0,
            offset: const Offset(0, 4),
          ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 5),
                decoration: BoxDecoration(
                  color: color.withValues(alpha: .09),
                  borderRadius: BorderRadius.circular(4),
                ),
                child: Text(
                  node.kind == 'file'
                      ? 'FILE'
                      : node.kind == 'function'
                      ? 'ƒ'
                      : node.kind == 'struct'
                      ? '{}'
                      : node.kind,
                  style: TextStyle(
                    fontFamily: 'Consolas',
                    fontSize: 11,
                    color: color,
                  ),
                ),
              ),
              const SizedBox(width: 7),
              Text(
                const {
                      'file': '文件',
                      'function': '函数',
                      'struct': '结构体',
                    }[node.kind] ??
                    node.kind,
                style: TextStyle(fontSize: 10, color: colors.muted),
              ),
              const Spacer(),
              Text(
                '${node.degree} 关联',
                style: TextStyle(fontSize: 10, color: colors.muted),
              ),
            ],
          ),
          const SizedBox(height: 5),
          Text(
            node.name,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(
              fontFamily: 'Consolas',
              fontSize: 14,
              fontWeight: FontWeight.w600,
              height: 1.4,
            ),
          ),
          const SizedBox(height: 3),
          Text(
            node.kind == 'file'
                ? (node.path.replaceFirst(RegExp(r'[^/\\]+$'), '').isEmpty
                      ? '/'
                      : node.path.replaceFirst(RegExp(r'[^/\\]+$'), ''))
                : '${node.path.replaceAll('\\', '/').split('/').last}:${node.line}',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(fontSize: 10, color: colors.muted, height: 1.4),
          ),
        ],
      ),
    );
  }
}

class _GraphPainter extends CustomPainter {
  _GraphPainter(
    this.edges,
    this.camera,
    this.scale,
    this.selected,
    this.nodeSelected,
    this.colors,
  );
  final List<({CodeLink edge, Path path})> edges;
  final Offset camera;
  final double scale;
  final String? selected;
  final bool nodeSelected;
  final DshColors colors;
  @override
  void paint(Canvas canvas, Size size) {
    final p = Paint()..color = colors.muted.withValues(alpha: .24);
    for (var x = 0.0; x < size.width; x += 22) {
      for (var y = 0.0; y < size.height; y += 22) {
        canvas.drawCircle(Offset(x, y), 1, p);
      }
    }
    canvas.save();
    canvas.translate(camera.dx, camera.dy);
    canvas.scale(scale);
    for (final entry in edges) {
      final e = entry.edge,
          active =
              selected == e.id || selected == e.source || selected == e.target;
      final color = colors.blue.withValues(
        alpha: nodeSelected && !active
            ? .22
            : active
            ? 1.0
            : .48,
      );
      p
        ..color = color
        ..style = PaintingStyle.stroke
        ..strokeWidth = active ? 2.5 : 1.8;
      for (final metric in entry.path.computeMetrics()) {
        if (e.inferred) {
          final dash = math.max(11, metric.length / 120);
          for (var d = 0.0; d < metric.length; d += dash) {
            canvas.drawPath(
              metric.extractPath(d, math.min(d + dash * .55, metric.length)),
              p,
            );
          }
        } else {
          canvas.drawPath(entry.path, p);
        }
        final at = metric.getTangentForOffset(metric.length);
        if (at != null) {
          final direction = at.vector / at.vector.distance,
              perpendicular = Offset(-direction.dy, direction.dx),
              tip = at.position;
          canvas.drawPath(
            Path()
              ..moveTo(tip.dx, tip.dy)
              ..lineTo(
                (tip - direction * 7 + perpendicular * 3.5).dx,
                (tip - direction * 7 + perpendicular * 3.5).dy,
              )
              ..lineTo(
                (tip - direction * 7 - perpendicular * 3.5).dx,
                (tip - direction * 7 - perpendicular * 3.5).dy,
              )
              ..close(),
            Paint()..color = color,
          );
        }
      }
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(_GraphPainter old) => true;
}

class _MiniMap extends CustomPainter {
  _MiniMap(this.points, this.bounds, this.viewport, this.color);
  final List<Offset> points;
  final Rect bounds, viewport;
  final Color color;
  @override
  void paint(Canvas canvas, Size size) {
    final scale = math.min(
      size.width / bounds.width,
      size.height / bounds.height,
    );
    canvas.save();
    canvas.translate(
      (size.width - bounds.width * scale) / 2,
      (size.height - bounds.height * scale) / 2,
    );
    canvas.scale(scale);
    canvas.translate(-bounds.left, -bounds.top);
    for (final p in points) {
      canvas.drawRRect(
        RRect.fromRectAndRadius(
          Rect.fromLTWH(p.dx, p.dy, 236, 92),
          const Radius.circular(12),
        ),
        Paint()..color = color.withValues(alpha: .24),
      );
    }
    canvas.drawRect(
      viewport,
      Paint()
        ..color = color
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2 / scale,
    );
    canvas.restore();
  }

  @override
  bool shouldRepaint(_MiniMap old) => true;
}
