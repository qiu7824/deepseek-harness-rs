import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';
import 'package:window_manager/window_manager.dart';

import '../../design/primitives.dart';
import '../../design/typography.dart';
import '../../src/controller.dart';
import '../../src/desktop_updates.dart';

class SettingsUpdatePanel extends StatefulWidget {
  const SettingsUpdatePanel({
    super.key,
    required this.controller,
    this.updates,
  });
  final DesktopController controller;
  final DesktopUpdateController? updates;

  @override
  State<SettingsUpdatePanel> createState() => _SettingsUpdatePanelState();
}

class _SettingsUpdatePanelState extends State<SettingsUpdatePanel> {
  DesktopUpdateController get updates =>
      widget.updates ?? DesktopUpdateController.instance;
  bool mirror = false;
  String? actionError;

  @override
  void initState() {
    super.initState();
    if (updates.phase == 'idle' && !updates.busy) unawaited(updates.check());
  }

  Future<void> openPackage() async {
    try {
      final path = await updates.packagePath();
      if (path != null && !await launchUrl(File(path).parent.uri)) {
        throw StateError('无法打开更新下载目录');
      }
    } catch (failure) {
      if (mounted) setState(() => actionError = '$failure');
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: updates,
    builder: (context, _) {
      final colors = DshColors(context);
      final last = updates.state['lastResult'];
      final lastError = last is Map && last['ok'] == false
          ? last['error'] as String?
          : null;
      final description = switch (updates.phase) {
        'checking' => '正在检查更新…',
        'downloading' =>
          updates.message.isEmpty ? '正在下载和校验更新…' : updates.message,
        'restarting' => '正在准备重启…',
        'ready' => '更新已下载并校验，可以安装',
        'available' => '新版本 ${updates.offeredVersion} 可用',
        'current' => '已是当前渠道的最新版本',
        _ => '检查桌面客户端和随附 Host 的更新',
      };
      return SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(32, 24, 32, 32),
        child: Align(
          alignment: Alignment.topLeft,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 720),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('更新', style: DshTypography.title),
                const SizedBox(height: 20),
                Text(
                  'DeepSeek Harness Desktop',
                  style: DshTypography.body.copyWith(color: colors.text),
                ),
                const SizedBox(height: 6),
                if (updates.currentVersion.isNotEmpty)
                  Text(
                    '版本 ${updates.currentVersion} · ${updates.state['channel'] == 'stable' ? '稳定渠道' : '预览渠道'}',
                    style: DshTypography.auxiliary.copyWith(
                      color: colors.muted,
                    ),
                  ),
                const SizedBox(height: 20),
                if (updates.busy) ...[
                  LinearProgressIndicator(
                    color: colors.blue,
                    backgroundColor: colors.hover,
                  ),
                  const SizedBox(height: 12),
                ],
                Text(description, style: DshTypography.body),
                if (updates.available) ...[
                  const SizedBox(height: 8),
                  Text(
                    '${updates.offeredVersion} · ${(updates.state['bytes'] as num? ?? 0) / (1024 * 1024) > 1 ? '${((updates.state['bytes'] as num? ?? 0) / (1024 * 1024)).toStringAsFixed(1)} MB' : '完整安装包'}',
                    style: DshTypography.auxiliary.copyWith(
                      color: colors.muted,
                    ),
                  ),
                ],
                const SizedBox(height: 20),
                Wrap(
                  spacing: 10,
                  runSpacing: 10,
                  children: [
                    OutlinedButton(
                      onPressed: updates.busy ? null : updates.check,
                      child: const Text('检查更新'),
                    ),
                    if (updates.available && !updates.ready)
                      FilledButton(
                        onPressed: updates.busy
                            ? null
                            : () => updates.prepare(mirror: mirror),
                        child: const Text('下载更新'),
                      ),
                    if (updates.ready && updates.restartSupported)
                      FilledButton(
                        onPressed: updates.busy
                            ? null
                            : () => updates.restart(
                                save: widget.controller.preferences.save,
                                close: windowManager.destroy,
                              ),
                        child: const Text('重启并安装'),
                      ),
                    if (updates.ready)
                      OutlinedButton(
                        onPressed: updates.busy ? null : openPackage,
                        child: const Text('打开下载目录'),
                      ),
                    TextButton(
                      onPressed: () => launchUrl(
                        Uri.parse(
                          'https://github.com/qiu7824/deepseek-harness-rs/releases',
                        ),
                        mode: LaunchMode.externalApplication,
                      ),
                      child: const Text('版本说明'),
                    ),
                  ],
                ),
                const SizedBox(height: 12),
                Row(
                  children: [
                    Checkbox(
                      value: mirror,
                      onChanged: updates.busy
                          ? null
                          : (value) => setState(() => mirror = value ?? false),
                    ),
                    const Flexible(child: Text('使用蓝奏下载线路')),
                  ],
                ),
                const SizedBox(height: 12),
                Text(
                  '检查更新不会自动下载或安装。重启前会保存草稿和本地设置；安装前须结束会话、命令、终端和后台任务，并停止本地 Host。',
                  style: DshTypography.auxiliary.copyWith(color: colors.muted),
                ),
                if (updates.checkedAt != null) ...[
                  const SizedBox(height: 12),
                  Text(
                    '上次检查：${updates.checkedAt!.toLocal().toString().split('.').first}',
                    style: DshTypography.auxiliary.copyWith(
                      color: colors.muted,
                    ),
                  ),
                ],
                for (final error in [
                  updates.error,
                  actionError,
                  lastError,
                ].whereType<String>()) ...[
                  const SizedBox(height: 12),
                  Text(
                    error,
                    style: DshTypography.body.copyWith(color: colors.error),
                  ),
                ],
              ],
            ),
          ),
        ),
      );
    },
  );
}
