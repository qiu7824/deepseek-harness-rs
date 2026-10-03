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
import 'package:path/path.dart' as paths;

import 'primitives.dart';
import 'error.dart';
import 'typography.dart';
import 'bounded_image.dart';

import 'package:dsh_desktop/l10n/conversation_zh.dart';

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
    return SelectionContainer.maybeOf(context) == null
        ? SelectionArea(child: body)
        : body;
  }

  Widget _buildBody(BuildContext context) {
    if (_body != null) return _body!;
    final colors = DshColors(context);
    final fontSize = widget.fontSize;
    final full = widget.conversationStyle;
    final scale =
        fontSize /
        (full ? DshTypography.sizeConversation : DshTypography.sizeBody);
    TextStyle heading(TextStyle role, {FontWeight weight = FontWeight.w700}) =>
        role.copyWith(
          fontSize: role.fontSize! * scale,
          fontWeight: weight,
          color: colors.text,
        );
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
        h1: heading(full ? DshTypography.title : DshTypography.headline),
        h2: heading(full ? DshTypography.sectionTitle : DshTypography.title),
        h3: heading(full ? DshTypography.composer : DshTypography.sectionTitle),
        h4: heading(
          full ? DshTypography.conversation : DshTypography.composer,
          weight: FontWeight.w600,
        ),
        h5: heading(DshTypography.conversation, weight: FontWeight.w600),
        h6: heading(DshTypography.conversation, weight: FontWeight.w600),
        strong: const TextStyle(fontWeight: FontWeight.w600),
        code: DshTypography.code.copyWith(
          fontSize: DshTypography.sizeAuxiliary * scale,
          color: colors.text,
          backgroundColor: colors.layer,
        ),
        // NativeCodeBlock draws its own frame; a second panel around it
        // doubled the border.
        codeblockDecoration: const BoxDecoration(),
        codeblockPadding: EdgeInsets.zero,
        // Tables read as a quiet grid: rounded outline, a tinted header row
        // and lighter inner rules instead of a heavy HTML-style lattice.
        tableBorder: TableBorder(
          top: BorderSide(color: colors.border),
          bottom: BorderSide(color: colors.border),
          left: BorderSide(color: colors.border),
          right: BorderSide(color: colors.border),
          horizontalInside: BorderSide(
            color: colors.border.withValues(alpha: .55),
          ),
          verticalInside: BorderSide(
            color: colors.border.withValues(alpha: .55),
          ),
          borderRadius: BorderRadius.circular(8),
        ),
        tableHead: const TextStyle(fontWeight: FontWeight.w600),
        tableHeadAlign: TextAlign.start,
        tableHeadCellsDecoration: BoxDecoration(color: colors.layer),
        tableCellsPadding: const EdgeInsets.symmetric(
          horizontal: 12,
          vertical: 8,
        ),
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
  final String? imageBaseDirectory;
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
      selectable: false,
      styleSheet: widget.style,
      imageDirectory: null,
      imageBuilder: (uri, title, alt) => MarkdownImage(
        uri: uri,
        baseDirectory: widget.imageBaseDirectory,
        label: alt?.isNotEmpty == true
            ? alt!
            : title ?? DshConversationZh.image,
        onSecondaryTap: (position) => widget.onSecondaryTapLink?.call(
          alt ?? title ?? DshConversationZh.image,
          '$uri',
          position,
        ),
      ),
      checkboxBuilder: null,
      bulletBuilder: null,
      builders: {'pre': _CodeBuilder(), 'math': _MathBuilder()},
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

String? markdownImageFile(Uri uri, String? base, {bool? windows}) {
  final onWindows = windows ?? Platform.isWindows;
  if (!onWindows && RegExp(r'^[a-zA-Z]$').hasMatch(uri.scheme)) return null;
  final context = paths.Context(
    style: onWindows ? paths.Style.windows : paths.Style.posix,
  );
  if (onWindows &&
      RegExp(r'^[a-zA-Z]$').hasMatch(uri.scheme) &&
      !uri.hasAuthority) {
    try {
      final drivePath =
          '${uri.scheme.toUpperCase()}:${Uri.decodeComponent(uri.path)}';
      return context.isAbsolute(drivePath)
          ? context.normalize(drivePath)
          : null;
    } on FormatException {
      return null;
    }
  }
  final path = markdownImageFilePath(uri, windows: onWindows);
  if (path == null || path.isEmpty) return null;
  if (context.isAbsolute(path)) return context.normalize(path);
  if (base == null || base.isEmpty) return null;
  return context.normalize(context.join(base, path));
}

class MarkdownImage extends StatelessWidget {
  const MarkdownImage({
    super.key,
    required this.uri,
    required this.label,
    this.onSecondaryTap,
    this.baseDirectory,
  });
  final Uri uri;
  final String label;
  final String? baseDirectory;
  final ValueChanged<Offset>? onSecondaryTap;

  ImageProvider? provider() {
    if (uri.scheme == 'http' || uri.scheme == 'https') {
      return NetworkImage('$uri');
    }
    final path = baseDirectory == null
        ? markdownImageFilePath(uri)
        : markdownImageFile(uri, baseDirectory);
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

  Widget image(ImageProvider provider, {bool preview = false}) =>
      DshBoundedImage(
        image: provider,
        maxDimension: preview ? 2400 : 1600,
        evictOnDispose: preview || provider is MemoryImage,
        fit: BoxFit.contain,
        semanticLabel: label,
        errorBuilder: (_, _, _) =>
            Text(DshConversationZh.imageUnavailable(label: label)),
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
                        DshIcons.close.data,
                        label: DshConversationZh.closeImagePreview,
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
    if (source == null) {
      return Text(DshConversationZh.imageUnavailable(label: label));
    }
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
      textStyle: TextStyle(
        fontSize: DshTypography.sizeConversation,
        color: DshColors(context).text,
      ),
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
  bool source = false, wrap = false;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final mermaid = widget.language == 'mermaid';
    final shown = widget.code.replaceFirst(RegExp(r'\s+$'), '');
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
                    widget.language.isEmpty
                        ? DshConversationZh.code
                        : widget.language,
                    style: TextStyle(
                      fontSize: DshTypography.sizeCaption,
                      color: colors.muted,
                    ),
                  ),
                ),
                if (mermaid)
                  DshButton(
                    height: 25,
                    onPressed: () => setState(() => source = !source),
                    child: Text(
                      source
                          ? DshConversationZh.diagram
                          : DshConversationZh.source,
                      style: const TextStyle(
                        fontSize: DshTypography.sizeCaption,
                      ),
                    ),
                  ),
                if (!mermaid || source)
                  DshIcon(
                    DshIcons.wrapText.data,
                    label: DshConversationZh.wrapLines,
                    active: wrap,
                    size: 28,
                    glyphSize: 14,
                    onPressed: () => setState(() => wrap = !wrap),
                  ),
                DshIcon(
                  DshIcons.copy.data,
                  label: DshConversationZh.copyCode,
                  size: 28,
                  glyphSize: 14,
                  onPressed: () =>
                      Clipboard.setData(ClipboardData(text: widget.code)),
                ),
              ],
            ),
          ),
          Divider(height: 1, color: colors.border.withValues(alpha: .7)),
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
            child: mermaid && !source && widget.code.length < 32768
                ? MermaidDiagram(
                    code: widget.code,
                    height: 320,
                    style: colors.dark
                        ? MermaidStyle.dark()
                        : const MermaidStyle(),
                    errorBuilder: (_, error) => SingleChildScrollView(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          DshErrorView(
                            error: error,
                            operation: DshConversationZh.diagram,
                          ),
                          Text(widget.code, style: DshTypography.code),
                        ],
                      ),
                    ),
                  )
                : wrap
                ? Text(
                    shown,
                    style: DshTypography.code.copyWith(color: colors.text),
                  )
                : SingleChildScrollView(
                    scrollDirection: Axis.horizontal,
                    child: Text(
                      shown,
                      style: DshTypography.code.copyWith(color: colors.text),
                    ),
                  ),
          ),
        ],
      ),
    );
  }
}
