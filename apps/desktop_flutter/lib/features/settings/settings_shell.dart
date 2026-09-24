import 'dart:convert';
import 'dart:math';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_ui/shadcn_ui.dart';
import 'package:dsh_client/dsh_client.dart';

import '../../design/primitives.dart';
import '../../design/select.dart';
import '../../design/shortcuts.dart';
import '../../src/controller.dart';
import 'models_page.dart';
import 'resource_page.dart';

const settingsPages = <({String id, String title, IconData icon})>[
  (id: 'general', title: '通用设置', icon: LucideIcons.settings),
  (id: 'models', title: '模型', icon: LucideIcons.database),
  (id: 'plugins', title: '插件', icon: LucideIcons.slidersHorizontal),
  (id: 'environment', title: '目录与运行环境', icon: LucideIcons.folderCog),
  (id: 'memory', title: '记忆与上下文', icon: LucideIcons.brain),
  (id: 'presets', title: 'Agent 预设', icon: LucideIcons.workflow),
  (id: 'collaboration', title: '协作', icon: LucideIcons.users),
  (id: 'security', title: '安全盾', icon: LucideIcons.shieldCheck),
  (id: 'skills', title: '技能与 MCP', icon: LucideIcons.briefcase),
  (id: 'discovery', title: '工具发现', icon: LucideIcons.wrench),
  (id: 'archive', title: '归档管理', icon: LucideIcons.archive),
  (id: 'menu', title: '小菜单设置', icon: LucideIcons.listFilter),
  (id: 'trash', title: '垃圾槽', icon: LucideIcons.trash2),
];

class SettingsShell extends StatefulWidget {
  const SettingsShell({
    super.key,
    required this.controller,
    this.initialPage = 'general',
    this.initialModelTab = 'api',
    this.onOpenPlugin,
  });
  final DesktopController controller;
  final String initialPage;
  final String initialModelTab;
  final ValueChanged<Json>? onOpenPlugin;
  @override
  State<SettingsShell> createState() => _SettingsShellState();
}

class _SettingsShellState extends State<SettingsShell> {
  DesktopController get c => widget.controller;
  late String page = settingsPages.any((p) => p.id == widget.initialPage)
      ? widget.initialPage
      : 'general';
  final namespaces = <String, Json>{}, edits = <String, List<Json>>{};
  final scope = RequestScope();
  final keyboardFocus = FocusNode();
  bool loading = true, saving = false;
  bool modelDirty = false;
  String? error;
  String? saved;
  bool get formDirty => edits.values.any((e) => e.isNotEmpty);
  bool get dirty => formDirty || modelDirty;
  Future<void> selectPage(String next) async {
    if (next == page) return;
    if (modelDirty &&
        !await confirmAction(
          context,
          '未保存的模型修改',
          '切换页面将放弃模型和连接草稿。',
          action: '放弃并切换',
        )) {
      return;
    }
    if (mounted) {
      setState(() {
        page = next;
        modelDirty = false;
      });
    }
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) keyboardFocus.requestFocus();
    });
    load();
  }

  @override
  void dispose() {
    scope.cancel();
    keyboardFocus.dispose();
    super.dispose();
  }

  Future<void> load() async {
    try {
      final api = c.client;
      if (api == null) throw StateError('请先连接服务');
      final value = await api.rpc('settings.describe', scope: scope);
      if (!mounted) return;
      setState(() {
        for (final row in objects(value['namespaces'])) {
          namespaces[row['ns'] as String] = row;
        }
        loading = false;
        error = null;
      });
    } catch (e) {
      if (mounted) {
        setState(() {
          error = '$e';
          loading = false;
        });
      }
    }
  }

  void edit(String ns, List<String> path, Object? value) {
    setState(() {
      saved = null;
      final list = edits.putIfAbsent(ns, () => []);
      list.removeWhere((op) => jsonEncode(op['path']) == jsonEncode(path));
      list.add({'op': 'set', 'path': path, 'value': value});
    });
  }

  Future<void> save() async {
    setState(() {
      saving = true;
      error = null;
      saved = null;
    });
    try {
      for (final ns in edits.keys.toList()) {
        final ops = edits[ns]!;
        if (ops.isEmpty) continue;
        final updated = await c.client!.call('settings.mutate', {
          'ns': ns,
          'expectedRevision': namespaces[ns]?['revision'],
          'ops': ops,
        }, true);
        if (!mounted) return;
        setState(() {
          namespaces[ns] = updated;
          edits.remove(ns);
        });
      }
      await c.loadCatalogs();
      if (mounted) setState(() => saved = '设置已保存');
    } catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => saving = false);
    }
  }

  Future<void> close() async {
    if (dirty &&
        !await confirmAction(
          context,
          '保留未保存的修改？',
          '当前设置尚未保存，关闭将放弃这些修改。',
          action: '放弃修改',
        )) {
      return;
    }
    if (mounted) Navigator.pop(context);
  }

  @override
  Widget build(BuildContext context) {
    final colors = DshColors(context);
    final small = MediaQuery.sizeOf(context).width <= 768;
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: Focus(
        focusNode: keyboardFocus,
        autofocus: true,
        child: PopScope(
          canPop: !dirty,
          onPopInvokedWithResult: (didPop, _) {
            if (!didPop) close();
          },
          child: BackdropFilter(
            filter: ui.ImageFilter.blur(sigmaX: 2, sigmaY: 2),
            child: ColoredBox(
              color: Colors.black.withValues(alpha: colors.dark ? .5 : .24),
              child: Dialog(
                insetPadding: small
                    ? EdgeInsets.zero
                    : const EdgeInsets.all(24),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(small ? 0 : 24),
                ),
                child: SizedBox(
                  key: const ValueKey('settings-panel'),
                  width: 1040,
                  height: small
                      ? MediaQuery.sizeOf(context).height
                      : min(800, MediaQuery.sizeOf(context).height - 48),
                  child: Column(
                    children: [
                      Padding(
                        padding: const EdgeInsets.fromLTRB(24, 20, 14, 6),
                        child: Row(
                          children: [
                            const Text(
                              '设置',
                              style: TextStyle(
                                fontSize: 16,
                                height: 24 / 16,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                            const Spacer(),
                            DshButton(
                              outline: true,
                              height: 28,
                              pill: true,
                              fontSize: 12,
                              padding: const EdgeInsets.symmetric(
                                horizontal: 10,
                              ),
                              onPressed: () => c.run(() async {
                                await c.client!.call(
                                  'settings.openDocument',
                                  {},
                                  true,
                                );
                              }),
                              child: const Text(
                                '打开配置文件',
                                style: TextStyle(fontSize: 12),
                              ),
                            ),
                            const SizedBox(width: 12),
                            DshIcon(
                              LucideIcons.x,
                              label: '关闭',
                              onPressed: saving ? null : close,
                            ),
                          ],
                        ),
                      ),
                      if (small)
                        SizedBox(
                          height: 42,
                          child: ListView(
                            scrollDirection: Axis.horizontal,
                            children: [
                              for (final item in settingsPages)
                                DshButton(
                                  onPressed: () => selectPage(item.id),
                                  child: Text(item.title),
                                ),
                            ],
                          ),
                        ),
                      Expanded(
                        child: Row(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            if (!small)
                              SizedBox(
                                width: 188,
                                child: ListView(
                                  padding: const EdgeInsets.fromLTRB(
                                    12,
                                    10,
                                    12,
                                    12,
                                  ),
                                  children: [
                                    for (final item in settingsPages)
                                      Padding(
                                        padding: const EdgeInsets.only(
                                          bottom: 4,
                                        ),
                                        child: Material(
                                          color: page == item.id
                                              ? colors.hover
                                              : Colors.transparent,
                                          borderRadius: BorderRadius.circular(
                                            8,
                                          ),
                                          child: InkWell(
                                            borderRadius: BorderRadius.circular(
                                              8,
                                            ),
                                            onTap: () => selectPage(item.id),
                                            child: Padding(
                                              padding:
                                                  const EdgeInsets.symmetric(
                                                    horizontal: 12,
                                                    vertical: 9,
                                                  ),
                                              child: Row(
                                                children: [
                                                  DshGlyph(
                                                    item.icon,
                                                    size:
                                                        [
                                                          'environment',
                                                          'trash',
                                                          'memory',
                                                          'collaboration',
                                                          'security',
                                                          'skills',
                                                          'archive',
                                                        ].contains(item.id)
                                                        ? 18
                                                        : 16,
                                                    asset: const {
                                                      'environment': 'assets/icons/web-nav-runtime-paths.svg',
                                                      'memory': 'assets/icons/web-nav-memory.svg',
                                                      'collaboration': 'assets/icons/web-nav-subagent.svg',
                                                      'security': 'assets/icons/web-nav-security.svg',
                                                      'skills': 'assets/icons/web-nav-capabilities.svg',
                                                      'archive': 'assets/icons/web-nav-archived-sessions.svg',
                                                      'trash': 'assets/icons/web-nav-trash.svg',
                                                      'plugins': 'assets/icons/web-IconPersonalizationOutline16.svg',
                                                      'discovery': 'assets/icons/web-IconSettingsOutline16.svg',
                                                      'menu': 'assets/icons/web-IconSettingsOutline16.svg',
                                                    }[item.id],
                                                  ),
                                                  const SizedBox(width: 11),
                                                  Expanded(
                                                    child: Text(
                                                      item.title,
                                                      style: const TextStyle(
                                                        fontSize: 14,
                                                        height: 22 / 14,
                                                      ),
                                                    ),
                                                  ),
                                                ],
                                              ),
                                            ),
                                          ),
                                        ),
                                      ),
                                  ],
                                ),
                              ),
                            Expanded(
                              child: Padding(
                                padding: const EdgeInsets.fromLTRB(
                                  24,
                                  0,
                                  32,
                                  24,
                                ),
                                child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    if (error != null)
                                      Container(
                                        padding: const EdgeInsets.all(10),
                                        color: Theme.of(context)
                                            .colorScheme
                                            .errorContainer,
                                        child: Row(
                                          children: [
                                            Expanded(
                                              child: Text(
                                                error!,
                                                style: const TextStyle(
                                                  fontSize: 12,
                                                ),
                                              ),
                                            ),
                                            DshIcon(
                                              LucideIcons.rotateCw,
                                              label: '重试',
                                              onPressed: load,
                                            ),
                                          ],
                                        ),
                                      ),
                                    if (saved != null)
                                      Text(
                                        saved!,
                                        style: const TextStyle(
                                          color: Colors.green,
                                          fontSize: 12,
                                        ),
                                      ),
                                    Expanded(
                                      child: loading
                                          ? const Center(
                                              child: CircularProgressIndicator(
                                                strokeWidth: 2,
                                              ),
                                            )
                                          : body(),
                                    ),
                                    if (formDirty)
                                      Container(
                                        padding: const EdgeInsets.only(top: 12),
                                        decoration: BoxDecoration(
                                          border: Border(
                                            top: BorderSide(
                                              color: colors.border,
                                            ),
                                          ),
                                        ),
                                        child: Row(
                                          children: [
                                            const Text(
                                              '有未保存的修改',
                                              style: TextStyle(fontSize: 12),
                                            ),
                                            const Spacer(),
                                            DshButton(
                                              onPressed: saving
                                                  ? null
                                                  : () {
                                                      setState(edits.clear);
                                                      load();
                                                    },
                                              child: const Text('取消'),
                                            ),
                                            const SizedBox(width: 8),
                                            DshButton(
                                              primary: true,
                                              onPressed: saving ? null : save,
                                              child: Text(
                                                saving ? '保存中…' : '保存',
                                              ),
                                            ),
                                          ],
                                        ),
                                      ),
                                  ],
                                ),
                              ),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget body() {
    if (page == 'models') {
      return ModelsPage(
        controller: c,
        onSettingsChanged: load,
        initialTab: widget.initialModelTab,
        onDirtyChanged: (value) {
          if (mounted && modelDirty != value) {
            setState(() => modelDirty = value);
          }
        },
      );
    }
    if ([
      'plugins',
      'presets',
      'skills',
      'discovery',
      'archive',
      'memory',
    ].contains(page)) {
      return SettingsResourcePage(
        key: ValueKey(page),
        controller: c,
        page: page,
        onOpenPlugin: widget.onOpenPlugin,
        footer: page == 'memory' ? forms(['memory', 'system-prompt']) : null,
      );
    }
    if (page == 'general') {
      return general();
    }
    final selection = switch (page) {
      'environment' => [
        'storage-paths',
        'workspace-scratch-paths',
        'uu-remote',
      ],
      'collaboration' => ['agent-teams', 'subagent'],
      'security' => ['security', 'permission'],
      'menu' => ['mini-menu', 'dsh-better-sidebar'],
      'trash' => ['workspace-scratch'],
      _ => <String>[],
    };
    return SingleChildScrollView(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            settingsPages.firstWhere((p) => p.id == page).title,
            style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w500),
          ),
          const SizedBox(height: 20),
          forms(selection),
        ],
      ),
    );
  }

  Object? setting(String ns, String field, Object? fallback) {
    final pending = edits[ns]
        ?.where((op) => jsonEncode(op['path']) == jsonEncode([field]))
        .lastOrNull;
    return pending == null
        ? object(namespaces[ns]?['value'])[field] ?? fallback
        : pending['value'];
  }

  Widget generalSelect(
    String ns,
    String field,
    String fallback,
    Map<String, String> options,
  ) => DshSelect<String>(
    options: options,
    value: '${setting(ns, field, fallback)}',
    onChanged: saving || namespaces[ns] == null
        ? null
        : (value) => edit(ns, [field], value),
  );

  Widget generalRow(String title, String hint, Widget control) => Container(
    constraints: const BoxConstraints(minHeight: 69),
    decoration: BoxDecoration(
      border: Border(bottom: BorderSide(color: DshColors(context).border)),
    ),
    padding: const EdgeInsets.symmetric(vertical: 16),
    child: Row(
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                title,
                style: const TextStyle(fontSize: 14, height: 22 / 14),
              ),
              if (hint.isNotEmpty) ...[
                const SizedBox(height: 5),
                Text(
                  hint,
                  style: TextStyle(
                    fontSize: 12,
                    height: 1.5,
                    color: DshColors(context).muted,
                  ),
                ),
              ],
            ],
          ),
        ),
        const SizedBox(width: 20),
        control,
      ],
    ),
  );

  Widget general() {
    final colors = DshColors(context);
    return ListView(
      children: [
        generalRow(
          'Agent 预设',
          '对此后新建的会话生效。运行中的会话保持它开始时的预设。',
          generalSelect('agent-presets', 'default', c.preset, {
            for (final p in c.presets)
              if (p['broken'] == null) '${p['id']}': '${p['name'] ?? p['id']}',
          }),
        ),
        generalRow(
          '权限',
          '选择新会话的默认权限模式',
          generalSelect(
            'permission',
            'defaultPreset',
            'workspace-write',
            permissionDefaults(),
          ),
        ),
        generalRow(
          '语言',
          '',
          generalSelect('locale', 'preference', 'zh', const {
            'zh': '中文',
            'en': 'English',
          }),
        ),
        Container(
          padding: const EdgeInsets.symmetric(vertical: 16),
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: colors.border)),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('外观', style: TextStyle(fontSize: 14, height: 22 / 14)),
              const SizedBox(height: 8),
              Row(
                children: [
                  for (final dark in [false, true]) ...[
                    if (dark) const SizedBox(width: 8),
                    Expanded(
                      child: Semantics(
                        checked: c.preferences.dark == dark,
                        label: dark ? '深色' : '浅色',
                        child: Material(
                          color: c.preferences.dark == dark
                              ? colors.layer
                              : colors.dark
                              ? const Color(0xff2c2c2e)
                              : colors.base,
                          shape: RoundedRectangleBorder(
                            borderRadius: BorderRadius.circular(18),
                            side: BorderSide(
                              color: c.preferences.dark == dark
                                  ? colors.muted.withValues(alpha: .6)
                                  : colors.border,
                            ),
                          ),
                          child: InkWell(
                            borderRadius: BorderRadius.circular(18),
                            onTap: () {
                              setState(() => c.preferences.dark = dark);
                              c.emit();
                              c.run(c.preferences.save);
                            },
                            child: SizedBox(
                              height: 82,
                              child: Column(
                                mainAxisAlignment: MainAxisAlignment.center,
                                children: [
                                  DshGlyph(
                                    dark ? LucideIcons.moon : LucideIcons.sun,
                                    size: 18,
                                  ),
                                  const SizedBox(height: 4),
                                  Text(
                                    dark ? '深色' : '浅色',
                                    style: const TextStyle(fontSize: 14),
                                  ),
                                ],
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ],
                ],
              ),
            ],
          ),
        ),
        generalRow(
          '繁忙时 Enter 键行为',
          '仅在智能体运行时生效；Cmd/Ctrl+Enter 使用另一行为',
          generalSelect('ui-conversation', 'busyEnter', 'queue', const {
            'queue': '排队发送',
            'steer': '立即引导',
          }),
        ),
        generalRow(
          '回复提示显示',
          '工具、思考、任务和运行提示的显示方式。',
          generalSelect('ui-conversation', 'hintDisplay', 'both', const {
            'both': '图标＋文字',
            'icons': '仅图标',
            'text': '仅文字',
          }),
        ),
        generalRow(
          '输入提示',
          '输入框获得焦点时显示一条随机使用提示，可随时关闭。',
          generalSelect('ui-conversation', 'composerTips', 'on', const {
            'on': '开启',
            'off': '关闭',
          }),
        ),
        generalRow(
          '目录选择器',
          '选择工作区、垃圾槽等目录时使用 Windows 目录选择器。',
          const Text('系统目录选择器', style: TextStyle(fontSize: 14)),
        ),
        generalRow(
          '快捷键',
          '设置侧边栏、搜索、新会话及工作台的组合键。',
          DshButton(
            outline: true,
            pill: true,
            onPressed: () => showDialog<void>(
              context: context,
              builder: (_) => ShortcutEditor(controller: c),
            ),
            child: const Text('快捷键绑定'),
          ),
        ),
      ],
    );
  }

  Map<String, String> permissionDefaults() {
    final schema = object(namespaces['permission']?['schema']),
        refs = object(object(namespaces['permission']?['schema'])['refs']);
    final root = object(refs['${schema['uid']}']);
    final field = object(refs['${object(root['dict'])['defaultPreset']}']);
    final options = <String, String>{
      for (final ref in field['list'] as List? ?? [])
        if (object(refs['$ref'])['value'] is String)
          object(refs['$ref'])['value'] as String: permissionName(
            object(refs['$ref'])['value'] as String,
          ),
    };
    final current =
        '${setting('permission', 'defaultPreset', 'workspace-write')}';
    return options.isEmpty ? {current: permissionName(current)} : options;
  }

  Widget forms(List<String> names) => Column(
    children: [
      for (final ns in names)
        if (namespaces[ns] != null)
          NamespaceForm(
            key: ValueKey('$ns:${namespaces[ns]!['revision']}'),
            namespace: namespaces[ns]!,
            pending: edits[ns] ?? [],
            onChange: (path, value) => edit(ns, path, value),
          ),
    ],
  );
}

Widget _row(String title, String hint, Widget control) => Padding(
  padding: const EdgeInsets.symmetric(vertical: 14),
  child: Row(
    crossAxisAlignment: CrossAxisAlignment.center,
    children: [
      Expanded(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(title, style: const TextStyle(fontSize: 14)),
            if (hint.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(top: 5),
                child: Text(
                  hint,
                  style: const TextStyle(fontSize: 12, color: Colors.grey),
                ),
              ),
          ],
        ),
      ),
      const SizedBox(width: 20),
      ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 270),
        child: control,
      ),
    ],
  ),
);

const fieldLabels = <String, String>{
  'default': 'Agent 预设',
  'defaultPreset': '权限',
  'busyEnter': '繁忙时 Enter 键行为',
  'composerTips': '输入提示',
  'hintDisplay': '回复提示显示',
  'enabled': '启用',
  'width': '工作台宽度',
  'rememberWidth': '记住宽度',
  'fullscreenOnOpen': '打开时全屏',
  'showFiles': '显示文件入口',
  'showGit': '显示 Git 入口',
  'showBrowser': '显示网页入口',
  'showTerminal': '显示终端入口',
  'dataDirectory': '数据目录',
  'cacheDirectory': '缓存目录',
  'environmentDirectory': '运行环境目录',
  'testDirectory': '测试目录',
  'maxMembers': '团队成员上限',
  'showButton': '显示团队按钮',
  'defaultMode': '默认协作模式',
  'maxParallel': '并行子智能体',
  'maxDepth': '嵌套深度',
  'maxTurns': '回合上限',
  'timeoutSeconds': '超时（秒）',
  'defaultProvider': '默认提供方',
  'defaultModel': '默认模型',
  'defaultReasoningEffort': '推理强度',
  'defaultMaxTokens': '输出 Token 上限',
  'toolCallMode': '工具调用呈现',
  'serviceTier': '服务等级',
  'apiRetryCount': '请求重试次数',
  'approvalTimeoutSeconds': '审批等待时长（秒）',
  'unattendedPolicy': '无人值守策略',
  'riskToolPolicy': '高风险工具策略',
  'outsideWritePolicy': '工作区外写入策略',
  'sensitiveReadPolicy': '敏感文件读取策略',
  'credentialShellPolicy': '凭据命令策略',
  'userProfileEnabled': '用户资料',
  'memoryBudget': '记忆预算',
  'profileBudget': '资料预算',
  'provider': '提供方',
  'contextEngine': '上下文引擎',
  'autoCompact': '自动压缩',
  'compactThreshold': '压缩阈值',
  'compactTarget': '压缩目标',
  'protectRecentMessages': '保护最近消息数',
  'persona': '角色描述',
  'personaSuffix': '角色补充',
  'includeHarnessIdentity': '包含 Harness 身份',
  'includeRuntimeContext': '包含运行环境',
  'autoClean': '自动清理',
  'reduceContext': '精简上下文',
  'keepDays': '保留天数',
  'failedDays': '失败产物保留天数',
  'recoveryDays': '恢复期（天）',
  'softLimitGib': '容量软上限（GiB）',
  'location': '位置',
  'cliPath': '客户端程序路径',
  'account': '账号',
  'deviceId': '设备 ID',
  'trajectory': '轨迹',
  'artifacts': '产物',
  'code-graph': '代码图谱',
  'context': '上下文',
  'tasks': '任务',
};
const optionLabels = <String, String>{
  'queue': '排队发送',
  'steer': '转向当前任务',
  'on': '开启',
  'off': '关闭',
  'both': '图标＋文字',
  'text': '仅文字',
  'icons': '仅图标',
  'read-only': '只读',
  'workspace-write': '工作区内修改',
  'full-access': '完全访问',
  'ask': '询问',
  'deny': '拒绝',
  'allow': '允许',
  'auto': '自动',
  'standard': '标准模式',
  'code': '代码模式',
  'blank': '空白模式',
  'native': '原生',
  'inherit': '继承',
  'zh-CN': '中文',
  'en-US': 'English',
};

class NamespaceForm extends StatefulWidget {
  const NamespaceForm({
    super.key,
    required this.namespace,
    required this.pending,
    required this.onChange,
  });
  final Json namespace;
  final List<Json> pending;
  final void Function(List<String>, Object?) onChange;
  @override
  State<NamespaceForm> createState() => _NamespaceFormState();
}

class _NamespaceFormState extends State<NamespaceForm> {
  final inputs = <String, TextEditingController>{};
  @override
  void dispose() {
    for (final input in inputs.values) {
      input.dispose();
    }
    super.dispose();
  }

  Object? current(List<String> path, Object? fallback) {
    for (final op in widget.pending.reversed) {
      if (jsonEncode(op['path']) == jsonEncode(path)) return op['value'];
    }
    return fallback;
  }

  @override
  Widget build(BuildContext context) {
    final schema = object(widget.namespace['schema']),
        refs = object(schema['refs']);
    final node = object(refs['${schema['uid']}']);
    final fields = object(node['dict']);
    final value = object(widget.namespace['value']);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final key in {...fields.keys, ...value.keys})
          if (![
            'sessionLayouts',
            'providers',
            'models',
            'modelPreferences',
            'profiles',
          ].contains(key))
            field(
              context,
              [key],
              current([key], value[key]),
              object(refs['${fields[key]}']),
              refs,
            ),
        if (widget.namespace['applies'] == 'restart')
          const Padding(
            padding: EdgeInsets.only(bottom: 12),
            child: Text(
              '这些设置需要重启服务后生效',
              style: TextStyle(fontSize: 12, color: Colors.grey),
            ),
          ),
      ],
    );
  }

  Widget field(
    BuildContext context,
    List<String> path,
    Object? value,
    Json schema,
    Json refs,
  ) {
    final key = path.last,
        label =
            fieldLabels[key] ??
            '${object(schema['meta'])['description'] ?? key}';
    var options = <Object?>[];
    if (schema['type'] == 'union') {
      options = (schema['list'] as List? ?? [])
          .map((id) => object(refs['$id']))
          .where((n) => n['type'] == 'const')
          .map((n) => n['value'])
          .toList();
    }
    if (schema['type'] == 'boolean' || value is bool) {
      return _row(
        label,
        '',
        DshSwitch(
          value: value == true,
          onChanged: (v) => widget.onChange(path, v),
        ),
      );
    }
    if (options.isNotEmpty) {
      return _row(
        label,
        '',
        DshSelect<Object>(
          value: options.contains(value) ? value : null,
          options: {
            for (final item in options) ?item: optionLabels['$item'] ?? '$item',
          },
          onChanged: (v) => widget.onChange(path, v),
        ),
      );
    }
    if (value is Map) {
      final dict = object(schema['dict']);
      return ExpansionTile(
        title: Text(label, style: const TextStyle(fontSize: 14)),
        tilePadding: EdgeInsets.zero,
        children: [
          for (final child in value.keys)
            field(
              context,
              [...path, '$child'],
              current([...path, '$child'], value[child]),
              object(refs['${dict[child]}']),
              refs,
            ),
        ],
      );
    }
    if (value is List) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(label, style: const TextStyle(fontSize: 14)),
            const SizedBox(height: 8),
            for (final (index, item) in value.indexed)
              if (item is String)
                Padding(
                  padding: const EdgeInsets.only(bottom: 6),
                  child: DshField(
                    controller: inputs.putIfAbsent(
                      '${path.join('/')}/$index',
                      () => TextEditingController(text: displayPath(item)),
                    ),
                    onChanged: (text) {
                      final list = List<dynamic>.of(value);
                      list[index] = text;
                      widget.onChange(path, list);
                    },
                  ),
                ),
            DshButton(
              icon: LucideIcons.plus,
              onPressed: () => widget.onChange(path, [...value, '']),
              child: const Text('添加'),
            ),
          ],
        ),
      );
    }
    final input = inputs.putIfAbsent(
      path.join('/'),
      () => TextEditingController(
        text: value == null ? '' : displayPath('$value'),
      ),
    );
    final isNumber = schema['type'] == 'number' || value is num;
    return _row(
      label,
      '',
      SizedBox(
        width: 260,
        child: DshField(
          controller: input,
          hint: object(schema['meta'])['default']?.toString(),
          onChanged: (text) {
            if (isNumber) {
              final number = num.tryParse(text);
              if (number != null) widget.onChange(path, number);
            } else {
              widget.onChange(path, text);
            }
          },
        ),
      ),
    );
  }
}
