import 'dart:async';

import 'package:dsh_client/dsh_client.dart';
import 'package:flutter/material.dart';

import '../design/primitives.dart';

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
      setState(() => error = '请填写地址和工作目录。');
      return;
    }
    final ports = <String, int>{};
    if (ssh) {
      for (final key in ['port', 'remotePort']) {
        final port = int.tryParse(field(key).text);
        if (port == null || port < 1 || port > 65535) {
          setState(() => error = '端口须为 1–65535 的整数。');
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
          throw StateError('服务未返回有效的工作区。');
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
        Text(title, style: const TextStyle(fontSize: 14)),
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
            ? 'SSH 远程工作目录'
            : widget.kind == 'cloud'
            ? 'Cloud · 云端 Git 仓库'
            : '克隆 Git 工作目录',
        style: const TextStyle(fontSize: 18),
      ),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                ssh
                    ? '通过 SSH 隧道连接远端已运行的 Harness。Agent、文件、Shell 和 PTC 均在远端运行；本机模型凭据不会复制到远端。'
                    : '仓库克隆到本机后，Agent 在本机目录运行，不提供云端计算。私有仓库使用已有 Git 凭据或 SSH 密钥；无需填写令牌。',
                style: const TextStyle(fontSize: 13, height: 1.6),
              ),
              const SizedBox(height: 16),
              input(ssh ? 'host' : 'source', ssh ? 'SSH 主机或配置别名' : '仓库地址'),
              input(
                'path',
                ssh ? '远端工作目录' : '本机目标目录',
                hint: ssh ? '/home/developer/project' : '填写尚不存在的绝对目录',
              ),
              if (!ssh) input('branch', '分支（可选）', hint: '留空使用仓库默认分支'),
              if (ssh) ...[
                input('port', 'SSH 端口'),
                input('remotePort', '远端 Harness 端口'),
                DshButton(
                  onPressed: () => setState(() => advanced = !advanced),
                  child: const Text('高级连接设置'),
                ),
                if (advanced) ...[
                  input('user', 'SSH 用户（可选）'),
                  input('configFile', '本机 SSH 配置文件（可选）'),
                  const Text(
                    '请先在远端启动 Harness，配置 SSH 密钥或 ssh-agent，并核对主机密钥；不支持交互密码登录。',
                  ),
                ],
              ],
              if (error != null)
                Text(
                  error!,
                  style: const TextStyle(color: Colors.red, fontSize: 13),
                ),
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
                        ? '已连接 · 远端执行'
                        : item['state'] == 'connecting'
                        ? '连接中'
                        : '已断开',
                  ),
                  if (item['error'] != null)
                    Text(
                      '${item['error']}',
                      style: const TextStyle(color: Colors.red),
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
                          child: const Text('打开远端工作区'),
                        ),
                      if (item['state'] != 'disconnected')
                        DshButton(
                          onPressed: () => connectionAction(
                            'disconnect',
                            '${object(item['connection'])['id']}',
                          ),
                          child: const Text('断开连接'),
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
                          child: const Text('编辑／重连'),
                        ),
                        DshButton(
                          onPressed: busy
                              ? null
                              : () => connectionAction(
                                  'remove',
                                  '${object(item['connection'])['id']}',
                                ),
                          child: const Text('移除连接'),
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
          child: Text(busy ? '取消操作' : '取消'),
        ),
        DshButton(
          primary: true,
          onPressed: busy ? null : submit,
          child: Text(
            busy
                ? '正在处理…'
                : ssh
                ? '连接远端工作目录'
                : '克隆并添加工作区',
          ),
        ),
      ],
    ),
  );
}
