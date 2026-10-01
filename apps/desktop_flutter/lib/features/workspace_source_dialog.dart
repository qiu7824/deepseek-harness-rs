import '../design/error.dart';
import '../l10n/zh.dart';

import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../design/primitives.dart';

import 'package:dsh_desktop/design/typography.dart';

class WorkspaceSourceDialog extends StatefulWidget {
  const WorkspaceSourceDialog({
    super.key,
    required this.api,
    required this.kind,
  });
  final DshClient api;
  final String kind;
  @override
  State<WorkspaceSourceDialog> createState() => _WorkspaceSourceDialogState();
}

class _WorkspaceSourceDialogState extends State<WorkspaceSourceDialog> {
  final fields = <String, TextEditingController>{};
  final scope = RequestScope();
  Timer? timer;
  bool busy = false, polling = false, advanced = false, cancelling = false;
  String? error;
  String operation = newRequestId();
  List<Json> connections = [];
  bool get ssh => widget.kind == 'ssh';
  TextEditingController field(String key) => fields.putIfAbsent(
    key,
    () => TextEditingController(
      text: key == 'port'
          ? '22'
          : key == 'remotePort'
          ? '58080'
          : '',
    ),
  );
  @override
  void initState() {
    super.initState();
    if (ssh) {
      refresh();
      timer = Timer.periodic(
        const Duration(milliseconds: 2500),
        (_) => refresh(),
      );
    }
  }

  @override
  void dispose() {
    timer?.cancel();
    scope.cancel();
    for (final c in fields.values) {
      c.dispose();
    }
    super.dispose();
  }

  Future<void> refresh() async {
    if (polling || scope.cancelled) return;
    polling = true;
    try {
      final result = await widget.api.request(
        '/__dsh-workspaces/ssh',
        scope: scope,
      );
      if (mounted) {
        setState(() {
          connections = objects(result['items']);
          if (result['error'] != null) error = '${result['error']}';
        });
      }
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      polling = false;
    }
  }

  Future<bool> connectionAction(String name, String id) async {
    try {
      await widget.api.request(
        '/__dsh-workspaces/ssh/$name',
        body: {'id': id},
        mutation: true,
        scope: scope,
      );
      await refresh();
      return true;
    } catch (e) {
      if (mounted) setState(() => error = '$e');
      return false;
    }
  }

  Future<void> submit() async {
    if (busy) return;
    final path = field('path').text.trim();
    final source = field(ssh ? 'host' : 'source').text.trim();
    if (path.isEmpty || source.isEmpty) {
      setState(() => error = DshShellZh.addressAndDirectoryRequired);
      return;
    }
    final ports = <String, int>{};
    if (ssh) {
      for (final key in ['port', 'remotePort']) {
        final port = int.tryParse(field(key).text);
        if (port == null || port < 1 || port > 65535) {
          setState(() => error = DshShellZh.portInvalid);
          return;
        }
        ports[key] = port;
      }
    }
    setState(() {
      busy = true;
      error = null;
      cancelling = false;
      if (!ssh) operation = newRequestId();
    });
    try {
      if (ssh) {
        await widget.api.request(
          '/__dsh-workspaces/ssh/connect',
          body: {
            'id': operation,
            'host': source,
            'path': path,
            ...ports,
            'user': field('user').text.trim(),
            'configFile': field('configFile').text.trim(),
          },
          scope: scope,
          mutation: true,
        );
        await refresh();
      } else {
        final result = await widget.api.rpc(
          'workspace.create',
          payload: {
            'kind': widget.kind,
            'source': source,
            'path': path,
            'operationId': operation,
            if (field('branch').text.trim().isNotEmpty)
              'branch': field('branch').text.trim(),
          },
          mutation: true,
          scope: scope,
        );
        final workspace = object(result['workspace']);
        if (workspace['workspaceId'] is! String) {
          throw StateError(DshShellZh.workspaceResponseInvalid);
        }
        if (mounted) Navigator.pop(context, workspace);
      }
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) {
        setState(() {
          busy = false;
          cancelling = false;
        });
      }
    }
  }

  Future<void> cancel() async {
    if (!busy) {
      Navigator.pop(context);
      return;
    }
    if (cancelling) return;
    setState(() => cancelling = true);
    try {
      if (ssh) {
        if (!await connectionAction('disconnect', operation) && mounted) {
          setState(() => cancelling = false);
        }
      } else {
        await widget.api.rpc(
          'workspace.cancelCreate',
          payload: {'operationId': operation},
          mutation: true,
          scope: scope,
        );
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          error = '$e';
          cancelling = false;
        });
      }
    }
  }

  Widget input(String key, String title, {String? hint}) => Padding(
    padding: const EdgeInsets.only(bottom: 12),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(title, style: const TextStyle(fontSize: DshTypography.sizeBody)),
        const SizedBox(height: 6),
        DshField(
          key: ValueKey('workspace-source-$key'),
          controller: field(key),
          hint: hint,
          enabled: !busy,
        ),
      ],
    ),
  );
  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !busy,
    child: AlertDialog(
      title: Text(
        ssh
            ? DshShellZh.sshDirectory
            : widget.kind == 'cloud'
            ? DshShellZh.cloudRepository
            : DshShellZh.cloneGitTitle,
        style: const TextStyle(fontSize: DshTypography.sizeSectionTitle),
      ),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                ssh ? DshShellZh.sshExplanation : DshShellZh.cloneExplanation,
                style: const TextStyle(
                  fontSize: DshTypography.sizeAuxiliary,
                  height: 1.6,
                ),
              ),
              const SizedBox(height: 16),
              input(
                ssh ? 'host' : 'source',
                ssh ? DshShellZh.sshHost : DshShellZh.repositoryUrl,
              ),
              input(
                'path',
                ssh ? DshShellZh.remoteDirectory : DshShellZh.localDirectory,
                hint: ssh
                    ? '/home/developer/project'
                    : DshShellZh.missingDirectoryHint,
              ),
              if (!ssh)
                input(
                  'branch',
                  DshShellZh.branch,
                  hint: DshShellZh.defaultBranch,
                ),
              if (ssh) ...[
                input('port', DshShellZh.sshPort),
                input('remotePort', DshShellZh.hostPort),
                DshButton(
                  onPressed: () => setState(() => advanced = !advanced),
                  child: const Text(DshShellZh.advancedConnection),
                ),
                if (advanced) ...[
                  input('user', DshShellZh.sshUser),
                  input('configFile', DshShellZh.sshConfig),
                  const Text(DshShellZh.sshPrerequisites),
                ],
              ],
              if (error != null) DshErrorView(error: error!),
              if (ssh)
                for (final item in connections) ...[
                  const Divider(),
                  Text(
                    displayPathText(
                      '${object(item['connection'])['host']} · ${object(item['connection'])['path']}',
                    ),
                  ),
                  Text(
                    item['state'] == 'connected'
                        ? DshShellZh.remoteConnected
                        : item['state'] == 'connecting'
                        ? DshShellZh.connectingState
                        : DshShellZh.disconnectedState,
                  ),
                  if (item['error'] != null)
                    Text(
                      '${item['error']}',
                      style: TextStyle(
                        color: DshTokens.of(context).error.foreground,
                      ),
                    ),
                  Wrap(
                    spacing: 8,
                    children: [
                      if (item['state'] == 'connected' &&
                          RegExp(r'^http://127\.0\.0\.1:\d+$')
                              .hasMatch('${item['url']}'))
                        DshButton(
                          onPressed: busy
                              ? null
                              : () => Navigator.pop(context, {
                                  'url': item['url'],
                                }),
                          child: const Text(DshShellZh.openRemoteWorkspace),
                        ),
                      if (item['state'] != 'disconnected')
                        DshButton(
                          onPressed: () => connectionAction(
                            'disconnect',
                            '${object(item['connection'])['id']}',
                          ),
                          child: const Text(DshShellZh.disconnect),
                        ),
                      if (item['state'] == 'disconnected') ...[
                        DshButton(
                          onPressed: busy
                              ? null
                              : () {
                                  final value = object(item['connection']);
                                  setState(() {
                                    operation = '${value['id']}';
                                    for (final key in [
                                      'host',
                                      'path',
                                      'port',
                                      'remotePort',
                                      'user',
                                      'configFile',
                                    ]) {
                                      field(key).text = '${value[key] ?? ''}';
                                    }
                                  });
                                },
                          child: const Text(DshShellZh.editReconnect),
                        ),
                        DshButton(
                          onPressed: busy
                              ? null
                              : () => connectionAction(
                                  'remove',
                                  '${object(item['connection'])['id']}',
                                ),
                          child: const Text(DshShellZh.removeConnection),
                        ),
                      ],
                    ],
                  ),
                ],
            ],
          ),
        ),
      ),
      actions: [
        DshButton(
          onPressed: cancelling ? null : cancel,
          child: Text(busy ? DshShellZh.cancelOperation : DshZh.cancel),
        ),
        DshButton(
          primary: true,
          onPressed: busy ? null : submit,
          child: Text(
            busy
                ? DshShellZh.processing
                : ssh
                ? DshShellZh.connectRemoteDirectory
                : DshShellZh.cloneWorkspace,
          ),
        ),
      ],
    ),
  );
}
