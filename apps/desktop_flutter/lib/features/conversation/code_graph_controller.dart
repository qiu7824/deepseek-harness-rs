import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/foundation.dart';
import 'package:dsh_desktop/l10n/conversation_zh.dart';

class CodeGraphController extends ChangeNotifier {
  CodeGraphController(this.api, this.session);
  final DshClient api;
  final String session;
  Json graph = {};
  CodeGraphModel model = const CodeGraphModel([], [], [], 0);
  bool files = true, active = true, loading = false, disposed = false;
  String query = '',
      focus = '',
      rootSymbol = '',
      direction = 'chain',
      snippet = '';
  Object? error, sourceError;
  bool mutationFailed = false;
  RequestScope? reader, source;
  final mutations = RequestScope();
  Timer? timer;
  int epoch = 0, sourceEpoch = 0;
  bool resumePending = false;
  String get status => '${graph['status'] ?? 'queued'}';
  bool get indexing => ['queued', 'indexing', 'checking'].contains(status);
  void emit() {
    if (!disposed) notifyListeners();
  }

  void cancelRead() {
    timer?.cancel();
    timer = null;
    reader?.cancel();
    reader = null;
    epoch++;
    loading = false;
  }

  void setActive(bool value) {
    if (active == value) return;
    active = value;
    if (value) {
      refresh();
    } else {
      cancelRead();
      source?.cancel();
      source = null;
      sourceEpoch++;
    }
  }

  void rebuild() {
    model = CodeGraphModel.fromJson(
      graph,
      files: files,
      focus: focus,
      hops: direction == 'blast' ? 3 : 1,
    );
  }

  void search(String value) {
    query = value;
    focus = '';
    rootSymbol = '';
    cancelRead();
    timer = Timer(const Duration(milliseconds: 250), refresh);
    emit();
  }

  void mode(bool value) {
    files = value;
    query = '';
    focus = '';
    rootSymbol = '';
    direction = 'chain';
    graph = {};
    rebuild();
    refresh();
  }

  void overview() {
    query = '';
    focus = '';
    rootSymbol = '';
    rebuild();
    refresh();
  }

  void focusNode(CodeNode node, {String? relation}) {
    focus = node.id;
    query = files ? node.path : '';
    if (!files) rootSymbol = node.id;
    if (relation != null) direction = relation;
    rebuild();
    refresh();
  }

  void symbols(CodeNode file) {
    files = false;
    query = file.path;
    focus = '';
    rootSymbol = '';
    graph = {};
    rebuild();
    refresh();
  }

  Future<void> refresh({bool resume = false}) async {
    resumePending = resumePending || resume;
    cancelRead();
    if (disposed || !active) return;
    final scope = RequestScope(), generation = epoch;
    reader = scope;
    loading = true;
    if (!mutationFailed) error = null;
    emit();
    final resumeNow = resumePending;
    resumePending = false;
    try {
      final result = await api.request(
        Uri(
          path: '/__dsh-preview/code-graph',
          queryParameters: {
            'sessionId': session,
            'mode': files
                ? 'deps'
                : rootSymbol.isEmpty
                ? 'symbols'
                : direction,
            'q': query,
            'selected': files ? '' : rootSymbol,
            if (resumeNow) 'resume': '1',
          },
        ).toString(),
        scope: scope,
        maxBytes: 2 * 1024 * 1024,
      );
      if (disposed || generation != epoch || !active) return;
      graph = result;
      if (!mutationFailed) error = null;
      rebuild();
    } catch (e) {
      if (!disposed && generation == epoch && !mutationFailed) error = e;
    } finally {
      if (!disposed && generation == epoch) {
        reader = null;
        loading = false;
        emit();
        if (active && error == null && indexing) {
          timer = Timer(const Duration(milliseconds: 1400), refresh);
        }
      }
    }
  }

  Future<void> pause() async {
    cancelRead();
    error = null;
    mutationFailed = false;
    try {
      await api.request(
        '/__dsh-preview/code-graph-cancel',
        body: {'sessionId': session},
        scope: mutations,
        mutation: true,
        maxBytes: 65536,
      );
      if (!disposed) await refresh();
    } catch (e) {
      if (!disposed) {
        error = e;
        mutationFailed = true;
        emit();
      }
    }
  }

  Future<void> readSource(String? path, int line) async {
    source?.cancel();
    source = null;
    final generation = ++sourceEpoch;
    snippet = '';
    sourceError = null;
    emit();
    if (path == null || path.isEmpty || !active || disposed) return;
    final scope = RequestScope();
    source = scope;
    try {
      final value = await api.request(
        Uri(
          path: '/__dsh-preview/source',
          queryParameters: {'sessionId': session, 'path': path},
        ).toString(),
        scope: scope,
        maxBytes: 8 * 1024 * 1024,
      );
      if (!disposed && generation == sourceEpoch) {
        snippet = codeExcerpt('${value['text'] ?? ''}', line);
        if (snippet.isEmpty) snippet = DshConversationZh.noSourceAtLocation;
        emit();
      }
    } catch (e) {
      if (!disposed && generation == sourceEpoch) {
        sourceError = e;
        emit();
      }
    }
  }

  @override
  void dispose() {
    disposed = true;
    cancelRead();
    sourceEpoch++;
    source?.cancel();
    mutations.cancel();
    graph = {};
    model = const CodeGraphModel([], [], [], 0);
    snippet = '';
    super.dispose();
  }
}
