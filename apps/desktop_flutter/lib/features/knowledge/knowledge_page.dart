import '../../design/error.dart';
import '../../l10n/zh.dart';
import '../../l10n/conversation_zh.dart';

import 'dart:async';

import 'package:desktop_drop/desktop_drop.dart';
import 'package:dsh_client/dsh_client.dart';
import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

import '../../design/primitives.dart';
import '../../design/loading.dart';
import '../../src/controller.dart';
import '../page_operation.dart';

import 'package:dsh_desktop/design/typography.dart';

/// Local knowledge bases over `/__dsh-knowledge/*`. The desktop client only
/// connects to a Host on this machine, so files and folders are imported by
/// path; imports get a long timeout because extraction can take a while.
class KnowledgeApi {
  KnowledgeApi(this.client, [this._importer]);
  final DshClient client;
  DshClient? _importer;
  bool _closed = false;
  static const _reads = {'catalog', 'documents', 'search'};

  DshClient get importer => _importer ??= DshClient(
    client.baseUri.toString(),
    timeout: const Duration(minutes: 10),
  );

  Future<Json> call(
    String operation, [
    Json body = const {},
    RequestScope? scope,
  ]) async {
    if (_closed) throw DshException('cancelled', DshKnowledgeZh.pageClosed);
    final Json value;
    try {
      value = await (operation == 'importPath' ? importer : client).request(
        '/__dsh-knowledge/$operation',
        body: body,
        scope: scope,
        mutation: !_reads.contains(operation),
        maxBytes: 16 * 1024 * 1024,
      );
    } on DshException catch (error) {
      throw unsupportedHostPage(error, operation, DshKnowledgeZh.title);
    }
    if (_closed) throw DshException('cancelled', DshKnowledgeZh.pageClosed);
    return value;
  }

  Future<void> close() async {
    _closed = true;
    await _importer?.close();
  }
}

String formatKnowledgeBytes(num bytes) => bytes < 1024
    ? '$bytes B'
    : bytes < 1048576
    ? '${(bytes / 1024).toStringAsFixed(1)} KB'
    : '${(bytes / 1048576).toStringAsFixed(1)} MB';

class KnowledgePage extends StatefulWidget {
  const KnowledgePage({
    super.key,
    required this.controller,
    required this.onClose,
    this.api,
  });
  final DesktopController controller;
  final VoidCallback onClose;
  final KnowledgeApi? api;
  @override
  State<KnowledgePage> createState() => _KnowledgePageState();
}

class _KnowledgePageState extends State<KnowledgePage> {
  KnowledgeApi? ownedApi, _api, _injected;
  DshClient? _client;
  RequestScope _scope = RequestScope();
  int _generation = 0, _readRevision = 0;
  DialogRoute<Json>? _createRoute;
  Json? catalog;
  String? error, selected;

  KnowledgeApi? get api => _api;
  List<Json> get bases => objects(catalog?['bases']);

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_hostChanged);
    _bind(force: true, notify: false);
  }

  @override
  void didUpdateWidget(KnowledgePage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != widget.controller) {
      oldWidget.controller.removeListener(_hostChanged);
      widget.controller.addListener(_hostChanged);
    }
    _bind(
      force:
          oldWidget.controller != widget.controller ||
          oldWidget.api != widget.api,
      notify: false,
    );
  }

  void _hostChanged() => _bind();

  void _closeCreate() {
    final route = _createRoute;
    _createRoute = null;
    if (route == null) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final navigator = route.navigator;
      if (navigator != null && navigator.mounted && route.isActive) {
        navigator.removeRoute(route);
      }
    });
  }

  void _bind({bool force = false, bool notify = true}) {
    final client = widget.api?.client ?? widget.controller.client;
    if (!force &&
        identical(client, _client) &&
        identical(widget.api, _injected)) {
      return;
    }
    _scope.cancel();
    _scope = RequestScope();
    _generation++;
    _closeCreate();
    unawaited(ownedApi?.close());
    ownedApi = null;
    _client = client;
    _injected = widget.api;
    _api =
        widget.api ?? (client == null ? null : ownedApi = KnowledgeApi(client));
    catalog = null;
    selected = null;
    error = client == null ? DshKnowledgeZh.connectFirst : null;
    if (notify && mounted) setState(() {});
    unawaited(load());
  }

  @override
  void dispose() {
    widget.controller.removeListener(_hostChanged);
    _generation++;
    _scope.cancel();
    _closeCreate();
    unawaited(ownedApi?.close());
    super.dispose();
  }

  Future<void> load() async {
    final owner = _api,
        generation = _generation,
        request = ++_readRevision,
        scope = _scope;
    if (owner == null) return;
    bool current() =>
        mounted &&
        generation == _generation &&
        request == _readRevision &&
        identical(owner, _api) &&
        !scope.cancelled;
    try {
      final value = await owner.call('catalog', const {}, scope);
      if (!current()) return;
      setState(() {
        catalog = value;
        error = null;
        if (selected == null && bases.isNotEmpty) {
          selected = bases.first['id'] as String?;
        }
      });
    } catch (e) {
      if (current()) {
        setState(
          () => error = isUnsupportedHost(e)
              ? DshKnowledgeZh.unsupportedHost
              : '$e',
        );
      }
    }
  }

  Future<void> create() async {
    final owner = _api, generation = _generation;
    if (owner == null) return;
    bool current() =>
        mounted &&
        generation == _generation &&
        identical(owner, _api) &&
        !_scope.cancelled;
    final route = DialogRoute<Json>(
      context: context,
      builder: (_) => KnowledgeCreateDialog(
        api: owner,
        ownerScope: _scope,
        isCurrent: current,
      ),
    );
    _createRoute = route;
    try {
      final base = await Navigator.of(context).push(route);
      if (base != null && current()) {
        setState(() => selected = base['id'] as String?);
        await load();
      }
    } finally {
      if (identical(_createRoute, route)) _createRoute = null;
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final all = bases;
    final current = all.where((base) => base['id'] == selected).firstOrNull;
    return Material(
      color: colors.base,
      child: LayoutBuilder(
        builder: (context, box) {
          final wide = box.maxWidth >= 880;
          final list = _BaseList(
            bases: all,
            loading: catalog == null && error == null,
            selected: selected,
            onSelect: (id) => setState(() => selected = id),
            onCreate: api == null ? null : create,
          );
          final detail = current == null
              ? null
              : KnowledgeBaseDetail(
                  key: ValueKey((api, current['id'])),
                  ownerScope: _scope,
                  base: current,
                  api: api!,
                  extensions: [
                    for (final ext in (catalog?['extensions'] as List? ?? []))
                      '$ext',
                  ],
                  onChanged: load,
                  onDeleted: () {
                    setState(() => selected = null);
                    unawaited(load());
                  },
                  onClose: () => setState(() => selected = null),
                );
          return Padding(
            padding: EdgeInsets.fromLTRB(
              wide ? 28 : 16,
              20,
              wide ? 28 : 16,
              16,
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    DshIcon(
                      DshIcons.arrowLeft.data,
                      label: DshKnowledgeZh.backToSession,
                      onPressed: widget.onClose,
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          const Text(
                            DshKnowledgeZh.title,
                            style: TextStyle(
                              fontSize: DshTypography.sizeTitle,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                          const SizedBox(height: 4),
                          Text(
                            DshKnowledgeZh.description,
                            style: TextStyle(
                              fontSize: DshTypography.sizeAuxiliary,
                              color: colors.muted,
                            ),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 12),
                    DshButton(
                      key: const Key('knowledge-create'),
                      primary: true,
                      icon: DshIcons.plus.data,
                      onPressed: api == null ? null : create,
                      child: const Text(DshKnowledgeZh.createTitle),
                    ),
                  ],
                ),
                const SizedBox(height: 14),
                if (error != null)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 10),
                    child: DshErrorView(error: error!),
                  ),
                Expanded(
                  child: !wide
                      ? detail ?? list
                      : detail == null
                      ? list
                      : Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            SizedBox(width: 320, child: list),
                            const SizedBox(width: 18),
                            Expanded(child: detail),
                          ],
                        ),
                ),
              ],
            ),
          );
        },
      ),
    );
  }
}

class _BaseList extends StatelessWidget {
  const _BaseList({
    required this.bases,
    required this.loading,
    required this.selected,
    required this.onSelect,
    required this.onCreate,
  });
  final List<Json> bases;
  final bool loading;
  final String? selected;
  final ValueChanged<String> onSelect;
  final VoidCallback? onCreate;
  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    if (loading && bases.isEmpty) {
      return DshListSkeleton(
        label: DshConversationZh.loadingList(name: DshKnowledgeZh.title),
      );
    }
    if (bases.isEmpty) {
      return DshEmpty(DshKnowledgeZh.empty, icon: DshIcons.bookOpen.data);
    }
    return ListView.separated(
      itemCount: bases.length,
      separatorBuilder: (_, _) => const SizedBox(height: 8),
      itemBuilder: (context, index) {
        final base = bases[index];
        final enabled = base['enabled'] == true;
        final description = '${base['description'] ?? ''}';
        return Material(
          key: ValueKey('knowledge-base-${base['id']}'),
          color: colors.layer,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
            side: BorderSide(
              color: base['id'] == selected ? colors.blue : colors.border,
            ),
          ),
          child: InkWell(
            borderRadius: BorderRadius.circular(12),
            onTap: () => onSelect(base['id'] as String),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Container(
                        width: 8,
                        height: 8,
                        decoration: BoxDecoration(
                          color: enabled
                              ? DshTokens.of(context).success.foreground
                              : colors.muted,
                          shape: BoxShape.circle,
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          '${base['name']}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            fontSize: DshTypography.sizeBody,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                      if (!enabled)
                        Text(
                          DshKnowledgeZh.disabledState,
                          style: TextStyle(
                            fontSize: DshTypography.sizeCaption,
                            color: colors.muted,
                          ),
                        ),
                    ],
                  ),
                  if (description.isNotEmpty) ...[
                    const SizedBox(height: 4),
                    Text(
                      description,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: DshTypography.sizeCaption,
                        color: colors.muted,
                      ),
                    ),
                  ],
                  const SizedBox(height: 4),
                  Text(
                    DshKnowledgeZh.documentSummary(
                      documents: base['documentCount'] ?? 0,
                      chunks: base['chunkCount'] ?? 0,
                    ),
                    style: TextStyle(
                      fontSize: DshTypography.sizeCaption,
                      color: colors.muted,
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

/// One base: name, description, search switch, documents and a search try.
class KnowledgeBaseDetail extends StatefulWidget {
  const KnowledgeBaseDetail({
    super.key,
    required this.base,
    required this.api,
    required this.extensions,
    required this.onChanged,
    required this.onDeleted,
    required this.onClose,
    this.ownerScope,
  });
  final RequestScope? ownerScope;
  final Json base;
  final KnowledgeApi api;
  final List<String> extensions;
  final Future<void> Function() onChanged;
  final VoidCallback onDeleted, onClose;
  @override
  State<KnowledgeBaseDetail> createState() => KnowledgeBaseDetailState();
}

class KnowledgeBaseDetailState extends State<KnowledgeBaseDetail> {
  late final name = TextEditingController(text: '${widget.base['name']}');
  late final description = TextEditingController(
    text: '${widget.base['description'] ?? ''}',
  );
  final query = TextEditingController();
  String tab = 'documents';
  String? busy, error, status;
  List<String> failures = [];
  List<Json>? documents, results;
  bool dropping = false;
  RequestScope _scope = RequestScope();
  void Function()? _detach;
  int _generation = 0, _documentsRevision = 0;
  late String _baseline = _signature();
  String _signature() => '${name.text}\u0000${description.text}';
  bool get dirty => _signature() != _baseline;
  String get id => widget.base['id'] as String;

  PageOperation operation() {
    final generation = _generation, api = widget.api, baseId = id;
    return PageOperation(
      _scope,
      () =>
          mounted &&
          generation == _generation &&
          identical(api, widget.api) &&
          baseId == id,
    );
  }

  @override
  void initState() {
    super.initState();
    _detach = widget.ownerScope?.register(_scope.cancel);
    unawaited(loadDocuments());
  }

  @override
  void didUpdateWidget(KnowledgeBaseDetail oldWidget) {
    super.didUpdateWidget(oldWidget);
    final rebound =
        oldWidget.api != widget.api ||
        oldWidget.base['id'] != widget.base['id'] ||
        oldWidget.ownerScope != widget.ownerScope;
    if (rebound) {
      _detach?.call();
      _scope.cancel();
      _scope = RequestScope();
      _detach = widget.ownerScope?.register(_scope.cancel);
      _generation++;
      name.text = '${widget.base['name']}';
      description.text = '${widget.base['description'] ?? ''}';
      _baseline = _signature();
      query.clear();
      tab = 'documents';
      busy = null;
      error = null;
      status = null;
      documents = null;
      results = null;
      failures = [];
      dropping = false;
    } else if (!dirty) {
      name.text = '${widget.base['name']}';
      description.text = '${widget.base['description'] ?? ''}';
      _baseline = _signature();
    }
    if (rebound ||
        oldWidget.base['documentCount'] != widget.base['documentCount'] ||
        oldWidget.base['updatedAt'] != widget.base['updatedAt']) {
      unawaited(loadDocuments());
    }
  }

  @override
  void dispose() {
    _generation++;
    _detach?.call();
    _scope.cancel();
    name.dispose();
    description.dispose();
    query.dispose();
    super.dispose();
  }

  Future<void> loadDocuments() async {
    final op = operation(), request = ++_documentsRevision;
    if (!op.valid) return;
    try {
      final value = await op.request(
        () => widget.api.call('documents', {'id': id}, op.scope),
      );
      if (op.valid && request == _documentsRevision) {
        setState(() => documents = objects(value['documents']));
      }
    } catch (e) {
      if (op.valid && request == _documentsRevision) {
        setState(() => error = '$e');
      }
    }
  }

  Future<void> run(
    String label,
    Future<void> Function(PageOperation op) action,
  ) async {
    if (busy != null) return;
    final op = operation();
    if (!op.valid) return;
    setState(() {
      busy = label;
      error = null;
    });
    try {
      await action(op);
    } catch (e) {
      if (op.valid) error = '$e';
    } finally {
      if (op.valid) setState(() => busy = null);
    }
  }

  Future<void> importPaths(List<String> paths) async {
    if (paths.isEmpty || busy != null) return;
    final owner = operation();
    if (!owner.valid) return;
    var added = 0;
    var truncated = false;
    final failed = <String>[];
    setState(() {
      failures = [];
      status = DshKnowledgeZh.importing;
    });
    await run('import', (op) async {
      for (final path in paths) {
        if (!op.valid) return;
        setState(
          () => status = DshKnowledgeZh.importingFile(
            name: path.split(RegExp(r'[\\/]')).last,
          ),
        );
        try {
          final value = await op.request(
            () => widget.api.call('importPath', {
              'id': id,
              'path': path,
            }, op.scope),
          );
          final report = object(value['report']);
          added += (report['added'] as List? ?? []).length;
          truncated |= report['truncated'] == true;
          for (final skipped in objects(report['skipped'])) {
            failed.add('${skipped['name']}：${skipped['reason']}');
          }
        } catch (e) {
          if (!op.valid) return;
          failed.add('$path：$e');
        }
      }
    });
    if (!owner.valid) return;
    setState(() {
      status = [
        DshKnowledgeZh.importedDocuments(count: added),
        if (failed.isNotEmpty)
          DshKnowledgeZh.skippedDocuments(count: failed.length),
        if (truncated) DshKnowledgeZh.importLimit,
      ].join(' · ');
      failures = failed;
    });
    await loadDocuments();
    if (owner.valid) {
      try {
        await owner.request(widget.onChanged);
      } catch (failure) {
        if (owner.valid) setState(() => error = '$failure');
      }
    }
  }

  Future<void> pickFiles() async {
    final owner = operation();
    final files = await openFiles(
      acceptedTypeGroups: [
        XTypeGroup(
          label: DshKnowledgeZh.documents,
          extensions: widget.extensions,
        ),
      ],
    );
    if (owner.valid) await importPaths([for (final file in files) file.path]);
  }

  Future<void> pickFolder() async {
    final owner = operation();
    final path = await getDirectoryPath();
    if (path != null && owner.valid) await importPaths([path]);
  }

  Future<void> search() => run('search', (op) async {
    final text = query.text;
    final value = await op.request(
      () => widget.api.call('search', {
        'query': text,
        'baseIds': [id],
        'limit': 10,
      }, op.scope),
    );
    if (op.valid && query.text == text) {
      setState(() => results = objects(value['results']));
    }
  });

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final base = widget.base;
    final enabled = base['enabled'] == true;
    return Container(
      key: const Key('knowledge-detail'),
      decoration: BoxDecoration(
        color: colors.layer,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: colors.border),
      ),
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              DshIcon(
                DshIcons.close.data,
                label: DshKnowledgeZh.closeDetails,
                onPressed: widget.onClose,
              ),
              const SizedBox(width: 4),
              Expanded(
                child: Text(
                  '${base['name']}',
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: DshTypography.sizeSectionTitle,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              const Text(
                DshKnowledgeZh.includeInSearch,
                style: TextStyle(fontSize: DshTypography.sizeAuxiliary),
              ),
              const SizedBox(width: 6),
              DshSwitch(
                key: const Key('knowledge-enabled'),
                value: enabled,
                onChanged: busy != null
                    ? null
                    : (value) => run('toggle', (op) async {
                        await op.request(
                          () => widget.api.call('update', {
                            'id': id,
                            'enabled': value,
                          }, op.scope),
                        );
                        await op.request(widget.onChanged);
                      }),
              ),
            ],
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              Expanded(
                child: DshField(
                  key: const Key('knowledge-name'),
                  controller: name,
                  hint: DshKnowledgeZh.name,
                  onChanged: (_) => setState(() {}),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                flex: 2,
                child: DshField(
                  key: const Key('knowledge-description'),
                  controller: description,
                  hint: DshKnowledgeZh.descriptionHint,
                  onChanged: (_) => setState(() {}),
                ),
              ),
            ],
          ),
          if (dirty) ...[
            const SizedBox(height: 8),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                Text(
                  DshKnowledgeZh.unsaved,
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
                ),
                const SizedBox(width: 8),
                DshButton(
                  onPressed: busy != null
                      ? null
                      : () => setState(() {
                          name.text = '${base['name']}';
                          description.text = '${base['description'] ?? ''}';
                          _baseline = _signature();
                        }),
                  child: const Text(DshZh.cancel),
                ),
                const SizedBox(width: 8),
                DshButton(
                  key: const Key('knowledge-save'),
                  primary: true,
                  onPressed: busy != null || name.text.trim().isEmpty
                      ? null
                      : () => run('save', (op) async {
                          final savedSignature = _signature();
                          await op.request(
                            () => widget.api.call('update', {
                              'id': id,
                              'name': name.text,
                              'description': description.text,
                            }, op.scope),
                          );
                          _baseline = savedSignature;
                          await op.request(widget.onChanged);
                        }),
                  child: Text(
                    busy == 'save' ? DshKnowledgeZh.saving : DshZh.save,
                  ),
                ),
              ],
            ),
          ],
          const SizedBox(height: 10),
          Row(
            children: [
              for (final (key, label) in [
                (
                  'documents',
                  DshKnowledgeZh.documentsTab(
                    count: base['documentCount'] ?? 0,
                  ),
                ),
                ('search', DshKnowledgeZh.testSearch),
              ])
                Padding(
                  padding: const EdgeInsets.only(right: 6),
                  child: DshButton(
                    key: ValueKey('knowledge-tab-$key'),
                    height: 30,
                    pill: true,
                    active: tab == key,
                    onPressed: () => setState(() => tab = key),
                    child: Text(
                      label,
                      style: const TextStyle(
                        fontSize: DshTypography.sizeAuxiliary,
                      ),
                    ),
                  ),
                ),
            ],
          ),
          const SizedBox(height: 10),
          if (error != null)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: DshErrorView(error: error!),
            ),
          Expanded(
            child: tab == 'documents'
                ? _documents(colors)
                : _search(colors, enabled),
          ),
          const SizedBox(height: 8),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              DshButton(
                key: const Key('knowledge-delete'),
                destructive: true,
                onPressed: busy != null
                    ? null
                    : () async {
                        final owner = operation();
                        final confirmed = await confirmAction(
                          context,
                          DshKnowledgeZh.deleteLibrary,
                          DshKnowledgeZh.deleteLibraryHint(
                            name: base['name'],
                            count: base['documentCount'] ?? 0,
                          ),
                          action: DshKnowledgeZh.delete,
                        );
                        if (!confirmed || !owner.valid) return;
                        await run('delete', (op) async {
                          await op.request(
                            () =>
                                widget.api.call('delete', {'id': id}, op.scope),
                          );
                          widget.onDeleted();
                        });
                      },
                child: const Text(DshKnowledgeZh.deleteLibrary),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _documents(DshColors colors) {
    final rows = documents;
    return DropTarget(
      onDragEntered: (_) => setState(() => dropping = true),
      onDragExited: (_) => setState(() => dropping = false),
      onDragDone: (detail) {
        setState(() => dropping = false);
        unawaited(importPaths([for (final file in detail.files) file.path]));
      },
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: double.infinity,
            padding: const EdgeInsets.all(12),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(12),
              border: Border.all(
                color: dropping ? colors.blue : colors.border,
                width: dropping ? 2 : 1,
              ),
            ),
            child: Wrap(
              spacing: 8,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                DshButton(
                  key: const Key('knowledge-upload'),
                  primary: true,
                  icon: DshIcons.filePlus.data,
                  onPressed: busy != null ? null : pickFiles,
                  child: const Text(DshKnowledgeZh.uploadFiles),
                ),
                DshButton(
                  key: const Key('knowledge-import-folder'),
                  icon: DshIcons.folderPlus.data,
                  onPressed: busy != null ? null : pickFolder,
                  child: const Text(DshKnowledgeZh.importFolder),
                ),
                Text(
                  DshKnowledgeZh.dropHint,
                  style: TextStyle(
                    fontSize: DshTypography.sizeCaption,
                    color: colors.muted,
                  ),
                ),
              ],
            ),
          ),
          if (status != null)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                status!,
                key: const Key('knowledge-status'),
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: colors.muted,
                ),
              ),
            ),
          for (final line in failures.take(20))
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(
                line,
                style: TextStyle(
                  fontSize: DshTypography.sizeCaption,
                  color: DshTokens.of(context).error.foreground,
                ),
              ),
            ),
          const SizedBox(height: 8),
          Expanded(
            child: rows == null
                ? const Center(child: CircularProgressIndicator(strokeWidth: 2))
                : rows.isEmpty
                ? DshEmpty(
                    DshKnowledgeZh.noDocuments,
                    icon: DshIcons.fileText.data,
                  )
                : ListView.separated(
                    itemCount: rows.length,
                    separatorBuilder: (_, _) =>
                        Divider(height: 1, color: colors.border),
                    itemBuilder: (context, index) {
                      final doc = rows[index];
                      final created = DateTime.tryParse('${doc['createdAt']}')
                          ?.toLocal();
                      return Padding(
                        key: ValueKey('knowledge-doc-${doc['id']}'),
                        padding: const EdgeInsets.symmetric(vertical: 8),
                        child: Row(
                          children: [
                            DshGlyph(
                              DshIcons.fileText.data,
                              size: 16,
                              color: colors.muted,
                            ),
                            const SizedBox(width: 10),
                            Expanded(
                              child: Tooltip(
                                message: '${doc['source'] ?? doc['name']}',
                                child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Text(
                                      '${doc['name']}',
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: const TextStyle(
                                        fontSize: DshTypography.sizeAuxiliary,
                                      ),
                                    ),
                                    Text(
                                      [
                                        formatKnowledgeBytes(
                                          (doc['bytes'] as num?) ?? 0,
                                        ),
                                        DshKnowledgeZh.chunks(
                                          count: doc['chunkCount'],
                                        ),
                                        DshKnowledgeZh.characters(
                                          count: doc['chars'],
                                        ),
                                        if (created != null)
                                          '${created.year}/${created.month}/${created.day}',
                                      ].join(' · '),
                                      style: TextStyle(
                                        fontSize: DshTypography.sizeCaption,
                                        color: colors.muted,
                                      ),
                                    ),
                                  ],
                                ),
                              ),
                            ),
                            DshIcon(
                              DshIcons.trash2.data,
                              label: DshKnowledgeZh.remove,
                              onPressed: busy != null
                                  ? null
                                  : () => run('remove', (op) async {
                                      await op.request(
                                        () => widget.api.call(
                                          'deleteDocument',
                                          {'id': doc['id']},
                                          op.scope,
                                        ),
                                      );
                                      await loadDocuments();
                                      if (op.valid) {
                                        await op.request(widget.onChanged);
                                      }
                                    }),
                            ),
                          ],
                        ),
                      );
                    },
                  ),
          ),
        ],
      ),
    );
  }

  Widget _search(DshColors colors, bool enabled) {
    final rows = results;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Expanded(
              child: DshField(
                key: const Key('knowledge-query'),
                controller: query,
                hint: DshKnowledgeZh.queryHint,
                prefix: DshIcons.search.data,
                onChanged: (_) => setState(() {}),
              ),
            ),
            const SizedBox(width: 8),
            DshButton(
              key: const Key('knowledge-search'),
              primary: true,
              onPressed: busy != null || query.text.trim().isEmpty
                  ? null
                  : search,
              child: const Text(DshKnowledgeZh.search),
            ),
          ],
        ),
        const SizedBox(height: 6),
        Text(
          enabled ? DshKnowledgeZh.searchMeaning : DshKnowledgeZh.disabled,
          style: TextStyle(
            fontSize: DshTypography.sizeCaption,
            color: colors.muted,
          ),
        ),
        const SizedBox(height: 8),
        Expanded(
          child: rows == null
              ? const SizedBox.shrink()
              : rows.isEmpty
              ? DshEmpty(DshKnowledgeZh.noResults, icon: DshIcons.searchX.data)
              : ListView.separated(
                  itemCount: rows.length,
                  separatorBuilder: (_, _) => const SizedBox(height: 8),
                  itemBuilder: (context, index) {
                    final hit = rows[index];
                    return Container(
                      key: ValueKey('knowledge-hit-$index'),
                      padding: const EdgeInsets.all(10),
                      decoration: BoxDecoration(
                        color: colors.base,
                        borderRadius: BorderRadius.circular(10),
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            '${hit['documentName']}  #${((hit['chunk'] as num?) ?? 0) + 1}',
                            style: const TextStyle(
                              fontSize: DshTypography.sizeAuxiliary,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                          const SizedBox(height: 4),
                          SelectableText(
                            '${hit['snippet']}',
                            style: TextStyle(
                              fontSize: DshTypography.sizeCaption,
                              color: colors.muted,
                            ),
                          ),
                        ],
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class KnowledgeCreateDialog extends StatefulWidget {
  const KnowledgeCreateDialog({
    super.key,
    required this.api,
    this.ownerScope,
    this.isCurrent,
  });
  final RequestScope? ownerScope;
  final bool Function()? isCurrent;
  final KnowledgeApi api;
  @override
  State<KnowledgeCreateDialog> createState() => _KnowledgeCreateDialogState();
}

class _KnowledgeCreateDialogState extends State<KnowledgeCreateDialog> {
  final name = TextEditingController(), description = TextEditingController();
  bool busy = false;
  String? error;
  final _scope = RequestScope();
  void Function()? _detach;
  @override
  void initState() {
    super.initState();
    _detach = widget.ownerScope?.register(_scope.cancel);
  }

  PageOperation operation() =>
      PageOperation(_scope, () => mounted && widget.isCurrent?.call() != false);

  @override
  void dispose() {
    _detach?.call();
    _scope.cancel();
    name.dispose();
    description.dispose();
    super.dispose();
  }

  Future<void> submit() async {
    final op = operation();
    if (!op.valid || busy) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final value = await op.request(
        () => widget.api.call('create', {
          'name': name.text,
          'description': description.text,
        }, op.scope),
      );
      if (mounted && op.valid) {
        Navigator.of(context).pop(object(value['base']));
      }
    } on DshException catch (e) {
      if (op.valid) {
        setState(
          () => error = e.outcomeUnknown
              ? DshKnowledgeZh.unknownCreateResult(message: e.message)
              : e.message,
        );
      }
    } catch (e) {
      if (op.valid) setState(() => error = '$e');
    } finally {
      if (op.valid) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    return Dialog(
      backgroundColor: colors.base,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 480),
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                DshKnowledgeZh.createTitle,
                style: TextStyle(
                  fontSize: DshTypography.sizeSectionTitle,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 14),
              DshField(
                key: const Key('knowledge-create-name'),
                controller: name,
                hint: DshKnowledgeZh.nameHint,
                autofocus: true,
                onChanged: (_) => setState(() {}),
              ),
              const SizedBox(height: 10),
              DshField(
                key: const Key('knowledge-create-description'),
                controller: description,
                hint: DshKnowledgeZh.optionalDescription,
                maxLines: 3,
              ),
              if (error != null) ...[
                const SizedBox(height: 8),
                DshErrorView(error: error!),
              ],
              const SizedBox(height: 16),
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  DshButton(
                    onPressed: busy ? null : () => Navigator.of(context).pop(),
                    child: const Text(DshZh.cancel),
                  ),
                  const SizedBox(width: 8),
                  DshButton(
                    key: const Key('knowledge-create-submit'),
                    primary: true,
                    onPressed: busy || name.text.trim().isEmpty ? null : submit,
                    child: Text(
                      busy ? DshKnowledgeZh.creating : DshKnowledgeZh.create,
                    ),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}
