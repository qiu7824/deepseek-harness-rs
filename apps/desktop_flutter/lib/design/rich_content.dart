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
    this.imageBaseDirectory,
  });
  final String data;
  final double fontSize;
  final bool conversationStyle;

  /// Directory that relative local image paths resolve against, normally the
  /// session workspace.
  final String? imageBaseDirectory;
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
        oldWidget.conversationStyle != widget.conversationStyle ||
        oldWidget.imageBaseDirectory != widget.imageBaseDirectory) {
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
    final body = _buildBody(context);
    // Inside the transcript the surrounding SelectionArea owns selection;
    // standalone documents (plans, questions, panels) provide their own.
    return SelectionContainer.maybeOf(context) == null
        ? SelectionArea(child: body)
        : body;
  }

  Widget _buildBody(BuildContext context) {
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
            imageBaseDirectory: widget.imageBaseDirectory,
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
    this.imageBaseDirectory,
  });
  final md.Node node;
  final String signature;
  final MarkdownStyleSheet style;
  final MarkdownTapLinkCallback onTapLink;
  final void Function(String text, String? href, Offset position)?
  onSecondaryTapLink;
  final String? imageBaseDirectory;
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
        oldWidget.style != widget.style ||
        oldWidget.imageBaseDirectory != widget.imageBaseDirectory) {
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
      // Plain text joins the enclosing SelectionArea, so one drag can cover
      // paragraphs, lists, code and neighbouring messages.
      selectable: false,
      styleSheet: widget.style,
      imageDirectory: null,
      imageBuilder: (uri, title, alt) {
        Widget failed(BuildContext context, Object error, StackTrace? stack) =>
            Text(alt ?? '图片无法显示');
        Widget content;
        ImageProvider? full;
        final local = markdownImageFile(uri, widget.imageBaseDirectory);
        if (uri.scheme == 'http' || uri.scheme == 'https') {
          full = NetworkImage('$uri');
          content = Image.network(
            '$uri',
            cacheWidth: 1600,
            fit: BoxFit.contain,
            errorBuilder: failed,
          );
        } else if (local != null) {
          full = FileImage(File(local));
          content = Image.file(
            File(local),
            cacheWidth: 1600,
            fit: BoxFit.contain,
            errorBuilder: failed,
          );
        } else if (uri.scheme == 'data' &&
            uri.toString().length <= 24 * 1024 * 1024) {
          try {
            content = Image.memory(
              uri.data!.contentAsBytes(),
              cacheWidth: 1600,
              errorBuilder: failed,
            );
          } catch (_) {
            content = Text(alt ?? '图片无法显示');
          }
        } else {
          content = Text(alt ?? '图片无法显示');
        }
        final preview = full;
        return GestureDetector(
          onTap: preview == null
              ? null
              : () => showMarkdownImage(
                  context,
                  preview,
                  alt ?? title ?? local ?? '$uri',
                ),
          onSecondaryTapUp: (event) => widget.onSecondaryTapLink?.call(
            alt ?? title ?? '图片',
            local ?? '$uri',
            event.globalPosition,
          ),
          child: MouseRegion(
            cursor: preview == null
                ? MouseCursor.defer
                : SystemMouseCursors.zoomIn,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 600),
              child: content,
            ),
          ),
        );
      },
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

/// Local file behind a Markdown image source, or null for remote and inline
/// images. Handles `file:` URIs, Windows drive paths (which parse as a
/// one-letter scheme), percent-encoded names and paths relative to [base].
String? markdownImageFile(Uri uri, String? base, {bool? windows}) {
  final onWindows = windows ?? Platform.isWindows;
  String decode(String value) {
    try {
      return Uri.decodeComponent(value);
    } on ArgumentError {
      return value;
    }
  }

  String path;
  if (uri.scheme == 'file') {
    try {
      path = uri.toFilePath(windows: onWindows);
    } on UnsupportedError {
      return null;
    }
  } else if (onWindows && RegExp(r'^[a-zA-Z]$').hasMatch(uri.scheme)) {
    path = '${uri.scheme.toUpperCase()}:${decode(uri.path)}';
  } else if (uri.scheme.isEmpty && !uri.hasAuthority) {
    path = decode(uri.path);
  } else {
    return null;
  }
  if (path.isEmpty) return null;
  final absolute =
      path.startsWith('/') ||
      (onWindows &&
          (path.startsWith(r'\') || RegExp(r'^[a-zA-Z]:[\\/]').hasMatch(path)));
  if (absolute) return onWindows ? path.replaceAll('/', r'\') : path;
  if (base == null || base.isEmpty) return null;
  final separator = onWindows ? r'\' : '/';
  var relative = onWindows ? path.replaceAll('/', r'\') : path;
  if (relative.startsWith('.$separator')) relative = relative.substring(2);
  final root = base.endsWith('/') || base.endsWith(r'\')
      ? base.substring(0, base.length - 1)
      : base;
  return '$root$separator$relative';
}

/// Opens one Markdown image at full size with zoom and pan.
Future<void> showMarkdownImage(
  BuildContext context,
  ImageProvider image,
  String title,
) => showDialog<void>(
  context: context,
  builder: (context) => Dialog(
    child: SizedBox(
      width: 1000,
      height: 720,
      child: Column(
        children: [
          Padding(
            padding: const EdgeInsets.all(10),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 14),
                  ),
                ),
                DshIcon(
                  LucideIcons.x,
                  label: '关闭',
                  onPressed: () => Navigator.pop(context),
                ),
              ],
            ),
          ),
          Expanded(
            child: InteractiveViewer(
              minScale: .1,
              maxScale: 5,
              child: Image(
                image: ResizeImage.resizeIfNeeded(2000, null, image),
                fit: BoxFit.contain,
                errorBuilder: (_, _, _) => const Center(child: Text('图片无法显示')),
              ),
            ),
          ),
        ],
      ),
    ),
  ),
);

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
              child: Text(
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
                        Text(
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
                    child: Text(
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
