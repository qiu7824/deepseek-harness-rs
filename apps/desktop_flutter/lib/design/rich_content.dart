import 'dart:convert';
import 'dart:io';

import 'package:dsh_client/dsh_client.dart' show displayPathText;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:flutter_math_fork/flutter_math.dart';
import 'package:flutter_mermaid/flutter_mermaid.dart';
import 'package:markdown/markdown.dart' as md;
import 'package:shadcn_ui/shadcn_ui.dart';

import 'primitives.dart';
import 'typography.dart';

class DshMarkdown extends StatefulWidget {
  const DshMarkdown({
    super.key,
    required this.data,
    this.onTapLink,
    this.fontSize = 14,
    this.conversationStyle = false,
    this.onSecondaryTapLink,
  });
  final String data;
  final double fontSize;
  final bool conversationStyle;
  final MarkdownTapLinkCallback? onTapLink;
  final void Function(String text, String? href, Offset position)?
  onSecondaryTapLink;
  @override
  State<DshMarkdown> createState() => _DshMarkdownState();
}

class _DshMarkdownState extends State<DshMarkdown> {
  Widget? _body;
  List<md.Node>? _nodes;
  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _body = null;
  }

  @override
  void didUpdateWidget(DshMarkdown oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.data != widget.data ||
        oldWidget.fontSize != widget.fontSize ||
        oldWidget.conversationStyle != widget.conversationStyle) {
      _body = null;
      _nodes = null;
    }
  }

  void _link(String text, String? href, String title) =>
      widget.onTapLink?.call(text, href, title);
  void _linkMenu(String text, String? href, Offset position) =>
      widget.onSecondaryTapLink?.call(text, href, position);

  @override
  Widget build(BuildContext context) {
    if (_body != null) return _body!;
    final colors = DshColors(context);
    final fontSize = widget.fontSize;
    final full = widget.conversationStyle;
    final style = MarkdownStyleSheet.fromTheme(Theme.of(context)).merge(
      MarkdownStyleSheet(
        blockSpacing: 16,
        p: TextStyle(
          fontFamily: DshTypography.family,
          fontFamilyFallback: DshTypography.fallback,
          fontSize: fontSize,
          height: 24 / fontSize,
          color: colors.text,
        ),
        h1: TextStyle(
          fontSize: fontSize * 1.5,
          height: full ? 30 / 21 : 10 / 7,
          fontWeight: FontWeight.w700,
          color: colors.text,
        ),
        h2: TextStyle(
          fontSize: 19,
          height: full ? 28 / 19 : null,
          fontWeight: full ? FontWeight.w700 : FontWeight.w600,
          color: colors.text,
        ),
        h3: TextStyle(
          fontSize: full ? 18 : 16,
          height: full ? 26 / 18 : null,
          fontWeight: full ? FontWeight.w700 : FontWeight.w600,
          color: colors.text,
        ),
        h4: full
            ? TextStyle(
                fontSize: 14,
                height: 24 / 14,
                fontWeight: FontWeight.w600,
                color: colors.text,
              )
            : null,
        h5: full
            ? TextStyle(
                fontSize: 14,
                height: 24 / 14,
                fontWeight: FontWeight.w600,
                color: colors.text,
              )
            : null,
        h6: full
            ? TextStyle(
                fontSize: 14,
                height: 24 / 14,
                fontWeight: FontWeight.w600,
                color: colors.text,
              )
            : null,
        strong: const TextStyle(fontWeight: FontWeight.w600),
        code: TextStyle(
          fontFamily: 'Consolas',
          fontSize: full ? fontSize * .875 : 12,
          color: colors.text,
          backgroundColor: colors.layer,
        ),
        codeblockDecoration: BoxDecoration(
          color: colors.layer,
          borderRadius: BorderRadius.circular(10),
        ),
        tableBorder: TableBorder.all(color: colors.border),
        tableCellsPadding: const EdgeInsets.all(8),
        listIndent: full ? 18 : 24,
        listBulletPadding: full ? EdgeInsets.zero : null,
        blockquoteDecoration: BoxDecoration(
          border: Border(left: BorderSide(color: colors.border, width: 3)),
        ),
      ),
    );
    _nodes ??=
        md.Document(
              inlineSyntaxes: [_MathSyntax()],
              blockSyntaxes: [_MathBlockSyntax()],
              extensionSet: md.ExtensionSet.gitHubFlavored,
              encodeHtml: false,
            )
            .parseLines(const LineSplitter().convert(widget.data))
            .map(_displayPaths)
            .toList();
    return _body = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (var i = 0; i < _nodes!.length; i++) ...[
          if (i > 0)
            SizedBox(
              height: full
                  ? conversationBlockSpacing(_nodes![i - 1], _nodes![i])
                  : style.blockSpacing,
            ),
          DshMarkdownBlock(
            key: ValueKey(i),
            node: _nodes![i],
            signature: jsonEncode(_nodeValue(_nodes![i])),
            style:
                _nodes![i] is md.Element &&
                    ['ul', 'ol'].contains((_nodes![i] as md.Element).tag)
                ? style.copyWith(blockSpacing: 6)
                : style,
            onTapLink: _link,
            onSecondaryTapLink: _linkMenu,
          ),
        ],
      ],
    );
  }
}

md.Node _displayPaths(md.Node node) {
  if (node is md.Text) return md.Text(displayPathText(node.textContent));
  if (node is md.Element && node.tag != 'code' && node.tag != 'pre') {
    final children = node.children;
    if (children != null) {
      for (var i = 0; i < children.length; i++) {
        children[i] = _displayPaths(children[i]);
      }
    }
  }
  return node;
}

double conversationBlockSpacing(md.Node previous, md.Node current) {
  final before = previous is md.Element ? previous.tag : '';
  final after = current is md.Element ? current.tag : '';
  if (before == 'hr' || ['hr', 'h1', 'h2', 'h3'].contains(after)) return 32;
  if (['h4', 'h5', 'h6'].contains(before) && ['ul', 'ol'].contains(after)) {
    return 8;
  }
  return 16;
}

Object _nodeValue(md.Node node) => node is md.Element
    ? [
        node.tag,
        node.attributes,
        [for (final child in node.children ?? <md.Node>[]) _nodeValue(child)],
      ]
    : node.textContent;

/// Reuses each rendered block while its parsed content and theme are stable.
/// Parsing the document as a whole preserves references, nested lists and fences.
class DshMarkdownBlock extends StatefulWidget {
  const DshMarkdownBlock({
    super.key,
    required this.node,
    required this.signature,
    required this.style,
    required this.onTapLink,
    this.onSecondaryTapLink,
  });
  final md.Node node;
  final String signature;
  final MarkdownStyleSheet style;
  final MarkdownTapLinkCallback onTapLink;
  final void Function(String text, String? href, Offset position)?
  onSecondaryTapLink;
  @override
  State<DshMarkdownBlock> createState() => _DshMarkdownBlockState();
}

class _DshMarkdownBlockState extends State<DshMarkdownBlock>
    implements MarkdownBuilderDelegate {
  final _links = <GestureRecognizer>[];
  Widget? _rendered;
  void _clear() {
    for (final link in _links) {
      link.dispose();
    }
    _links.clear();
    _rendered = null;
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _clear();
  }

  @override
  void didUpdateWidget(DshMarkdownBlock oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.signature != widget.signature ||
        oldWidget.style != widget.style) {
      _clear();
    }
  }

  @override
  void dispose() {
    _clear();
    super.dispose();
  }

  @override
  GestureRecognizer createLink(String text, String? href, String title) {
    final link = TapGestureRecognizer()
      ..onTap = () {
        widget.onTapLink(text, href, title);
      }
      ..onSecondaryTapUp = (details) {
        widget.onSecondaryTapLink?.call(text, href, details.globalPosition);
      };
    _links.add(link);
    return link;
  }

  @override
  TextSpan formatText(MarkdownStyleSheet styleSheet, String code) => TextSpan(
    style: styleSheet.code,
    text: code.replaceFirst(RegExp(r'\n$'), ''),
  );
  @override
  Widget build(BuildContext context) {
    if (_rendered != null) return _rendered!;
    final children = MarkdownBuilder(
      delegate: this,
      selectable: true,
      styleSheet: widget.style,
      imageDirectory: null,
      imageBuilder: (uri, title, alt) => MarkdownImage(
        uri: uri,
        label: alt?.isNotEmpty == true ? alt! : title ?? '图片',
        onSecondaryTap: (position) => widget.onSecondaryTapLink?.call(
          alt ?? title ?? '图片',
          '$uri',
          position,
        ),
      ),
      checkboxBuilder: null,
      bulletBuilder: null,
      builders: {
        'pre': _CodeBuilder(),
        'math': _MathBuilder(),
        'code': _InlineCodeBuilder(),
      },
      paddingBuilders: const {},
      listItemCrossAxisAlignment: MarkdownListItemCrossAxisAlignment.baseline,
      fitContent: true,
      softLineBreak: true,
      contextMenuBuilder: (context, state) =>
          AdaptiveTextSelectionToolbar.editableText(editableTextState: state),
    ).build([widget.node]);
    return _rendered = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: children,
    );
  }
}

/// Decode local image URLs once, including drive-letter links emitted on Windows.
/// Invalid file URLs remain a failed image instead of interrupting Markdown layout.
String? markdownImageFilePath(Uri uri, {bool? windows}) {
  try {
    if (RegExp(r'^[a-zA-Z]$').hasMatch(uri.scheme) &&
        uri.path.startsWith('/') &&
        !uri.hasAuthority) {
      return Uri.parse('file:///$uri').toFilePath(windows: true);
    }
    if (uri.scheme != 'file' && uri.scheme.isNotEmpty) return null;
    final local = uri.host.toLowerCase() == 'localhost'
        ? uri.replace(host: '')
        : uri;
    final isWindows =
        windows ??
        (Platform.isWindows ||
            local.hasAuthority && local.host.isNotEmpty ||
            RegExp(r'^/[a-zA-Z]:/').hasMatch(local.path));
    return local.toFilePath(windows: isWindows);
  } on UnsupportedError {
    return null;
  } on ArgumentError {
    return null;
  } on FormatException {
    return null;
  }
}

class MarkdownImage extends StatelessWidget {
  const MarkdownImage({
    super.key,
    required this.uri,
    required this.label,
    this.onSecondaryTap,
  });
  final Uri uri;
  final String label;
  final ValueChanged<Offset>? onSecondaryTap;

  ImageProvider? provider() {
    if (uri.scheme == 'http' || uri.scheme == 'https') {
      return NetworkImage('$uri');
    }
    final path = markdownImageFilePath(uri);
    if (path != null) return FileImage(File(path));
    if (uri.scheme == 'data' && '$uri'.length <= 24 * 1024 * 1024) {
      try {
        return MemoryImage(uri.data!.contentAsBytes());
      } on FormatException {
        return null;
      }
    }
    return null;
  }

  Widget image(ImageProvider provider, {bool preview = false}) => Image(
    image: ResizeImage.resizeIfNeeded(preview ? 2400 : 1600, null, provider),
    fit: BoxFit.contain,
    semanticLabel: label,
    errorBuilder: (_, _, _) => Text('$label（图片无法显示）'),
  );

  Future<void> preview(BuildContext context, ImageProvider provider) =>
      showDialog<void>(
        context: context,
        builder: (context) => Dialog(
          child: SizedBox(
            width: 1000,
            height: 720,
            child: Column(
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
                  child: Row(
                    children: [
                      Expanded(
                        child: Text(
                          label,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                      DshIcon(
                        LucideIcons.x,
                        label: '关闭图片预览',
                        onPressed: () => Navigator.pop(context),
                      ),
                    ],
                  ),
                ),
                Expanded(
                  child: InteractiveViewer(
                    minScale: .1,
                    maxScale: 5,
                    child: Center(child: image(provider, preview: true)),
                  ),
                ),
              ],
            ),
          ),
        ),
      );

  @override
  Widget build(BuildContext context) {
    final source = provider();
    if (source == null) return Text('$label（图片无法显示）');
    return GestureDetector(
      onSecondaryTapUp: (event) => onSecondaryTap?.call(event.globalPosition),
      child: Material(
        type: MaterialType.transparency,
        child: InkWell(
          onTap: () => preview(context, source),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 600),
            child: image(source),
          ),
        ),
      ),
    );
  }
}

class _InlineCodeBuilder extends MarkdownElementBuilder {
  @override
  Widget? visitElementAfterWithContext(
    BuildContext context,
    md.Element element,
    TextStyle? preferredStyle,
    TextStyle? parentStyle,
  ) {
    final colors = DshColors(context);
    return Text.rich(
      TextSpan(
        children: [
          WidgetSpan(
            alignment: PlaceholderAlignment.middle,
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 5),
              decoration: BoxDecoration(
                color: colors.dark
                    ? const Color(0xff2c2c2e)
                    : const Color(0xffebeef2),
                borderRadius: BorderRadius.circular(6),
              ),
              child: SelectableText(
                element.textContent,
                style: (preferredStyle ?? const TextStyle()).copyWith(
                  backgroundColor: Colors.transparent,
                  fontFamily: 'Consolas',
                  fontSize: (parentStyle?.fontSize ?? 14) * .875,
                  height: 19 / ((parentStyle?.fontSize ?? 14) * .875),
                  color: colors.text,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _MathSyntax extends md.InlineSyntax {
  _MathSyntax() : super(r'\$([^\$\n]+)\$');
  @override
  bool onMatch(md.InlineParser parser, Match match) {
    parser.addNode(md.Element.text('math', match[1]!));
    return true;
  }
}

class _MathBlockSyntax extends md.BlockSyntax {
  @override
  RegExp get pattern => RegExp(r'^\s*\$\$\s*$');
  @override
  md.Node parse(md.BlockParser parser) {
    parser.advance();
    final text = <String>[];
    while (!parser.isDone && !pattern.hasMatch(parser.current.content)) {
      text.add(parser.current.content);
      parser.advance();
    }
    if (!parser.isDone) parser.advance();
    return md.Element.text('math', text.join('\n'));
  }
}

class _MathBuilder extends MarkdownElementBuilder {
  @override
  Widget? visitElementAfterWithContext(
    BuildContext context,
    md.Element element,
    TextStyle? preferredStyle,
    TextStyle? parentStyle,
  ) => SingleChildScrollView(
    scrollDirection: Axis.horizontal,
    child: Math.tex(
      element.textContent,
      textStyle: TextStyle(fontSize: 15, color: DshColors(context).text),
      onErrorFallback: (error) => SelectableText(element.textContent),
    ),
  );
}

class _CodeBuilder extends MarkdownElementBuilder {
  @override
  // pre is a built-in block. Registering it again makes the upstream builder
  // append another entry to its global block-tag list on every rebuild.
  bool isBlockElement() => false;
  @override
  Widget? visitElementAfterWithContext(
    BuildContext context,
    md.Element element,
    TextStyle? preferredStyle,
    TextStyle? parentStyle,
  ) {
    final code = element.children?.whereType<md.Element>().firstOrNull;
    final language = (code?.attributes['class'] ?? '').replaceFirst(
      'language-',
      '',
    );
    return NativeCodeBlock(
      code: code?.textContent ?? element.textContent,
      language: language,
    );
  }
}

class NativeCodeBlock extends StatefulWidget {
  const NativeCodeBlock({
    super.key,
    required this.code,
    required this.language,
  });
  final String code, language;
  @override
  State<NativeCodeBlock> createState() => _NativeCodeBlockState();
}

class _NativeCodeBlockState extends State<NativeCodeBlock> {
  bool source = false;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final mermaid = widget.language == 'mermaid';
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 8),
      decoration: BoxDecoration(
        color: colors.layer,
        border: Border.all(color: colors.border),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 4, 6, 4),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    widget.language.isEmpty ? '代码' : widget.language,
                    style: TextStyle(fontSize: 11, color: colors.muted),
                  ),
                ),
                if (mermaid)
                  DshButton(
                    height: 25,
                    onPressed: () => setState(() => source = !source),
                    child: Text(
                      source ? '图表' : '源码',
                      style: const TextStyle(fontSize: 11),
                    ),
                  ),
                DshIcon(
                  LucideIcons.copy,
                  label: '复制代码',
                  size: 25,
                  onPressed: () =>
                      Clipboard.setData(ClipboardData(text: widget.code)),
                ),
              ],
            ),
          ),
          const Divider(height: 1),
          Padding(
            padding: const EdgeInsets.all(12),
            child: mermaid && !source && widget.code.length < 32768
                ? MermaidDiagram(
                    code: widget.code,
                    height: 320,
                    style: colors.dark
                        ? MermaidStyle.dark()
                        : const MermaidStyle(),
                    errorBuilder: (_, error) => Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '图表解析失败：$error',
                          style: const TextStyle(
                            fontSize: 11,
                            color: Colors.orange,
                          ),
                        ),
                        SelectableText(
                          widget.code,
                          style: const TextStyle(
                            fontFamily: 'Consolas',
                            fontSize: 12,
                          ),
                        ),
                      ],
                    ),
                  )
                : SingleChildScrollView(
                    scrollDirection: Axis.horizontal,
                    child: SelectableText(
                      widget.code,
                      style: TextStyle(
                        fontFamily: 'Consolas',
                        fontSize: 12,
                        height: 1.6,
                        color: colors.text,
                      ),
                    ),
                  ),
          ),
        ],
      ),
    );
  }
}
