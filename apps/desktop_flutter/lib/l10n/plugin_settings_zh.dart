/// Chinese labels for plugin inventory and configuration controls.
abstract final class DshPluginSettingsZh {
  static const names = <String, String>{
    'cordis:noop': '空操作插件',
    'dsh-auto-review': '自动审批',
    'dsh-experimental-auto-review': '自动审批',
    'dsh-time-context': '时间上下文',
    'dsh-schedule': '定时任务',
    'dsh-artifacts': '产物预览',
    'dsh-context-jump': '上下文跳转',
    'dsh-better-sidebar': '增强侧栏',
    'dsh-sidebar-workbench-suite': '侧栏工作台',
    'dsh-voice-input': '语音输入',
  };
  static const descriptions = <String, String>{
    'cordis:noop': '提供不执行额外操作的插件占位。',
    'dsh-auto-review': '自动审查权限请求，并按审批策略作出决定。',
    'dsh-experimental-auto-review': '自动审查权限请求，并按审批策略作出决定。',
    'dsh-time-context': '将当前时间加入上下文，可设置更新间隔。',
    'dsh-schedule': '按计划运行任务，管理执行时间和任务内容。',
    'dsh-artifacts': '查看会话产物，管理临时工作资源。',
    'dsh-context-jump': '按用户消息定位对话，按需载入历史并跳转。',
    'dsh-better-sidebar': '在侧栏打开文件和终端，保留工作区布局。',
    'dsh-sidebar-workbench-suite': '在侧栏查看文档、数据、设备和任务。',
    'dsh-voice-input': '通过麦克风录入语音并转换为输入文本。',
  };
  static String canonical(String name) => name.startsWith('@deepseek-ai/')
      ? name.substring('@deepseek-ai/'.length)
      : name;
  static String title(String name, String fallback) =>
      names[canonical(name)] ?? fallback;
  static String description(String name, String fallback) =>
      descriptions[canonical(name)] ?? fallback;
  static bool retired(String name) => canonical(name) == 'dsh-skin-center';

  /// Plugins with a page or view of their own in the desktop client.
  static const openable = {
    'dsh-artifacts',
    'dsh-context-jump',
    'dsh-better-sidebar',
    'dsh-sidebar-workbench-suite',
    'dsh-voice-input',
    'dsh-schedule',
  };
  static String open(String title) => '打开$title';
  static const disabled = '未启用';

  /// Enablement and runtime phase read as one state: a disabled plugin is
  /// not described by whatever its runtime last reported.
  static String state(bool enabled, Object? phase) =>
      enabled ? runtime(phase) : disabled;
  static String runtime(Object? phase) => switch (phase) {
    'active' => '运行中',
    'pending' => '等待依赖',
    'loading' => '正在加载',
    'failed' => '启动失败',
    'unloading' => '正在停止',
    null || '' => '未运行',
    _ => '状态未知',
  };
  static String toggle(String title) => '启用$title';
  static const pageHint = '管理插件与配置，为工作区扩展能力。';
  static const addPlugin = '添加插件';
  static const backToSession = '返回会话';
  static const currentHost = '当前服务';
  static const searchPlugins = '搜索插件';
  static const experimental = '实验性';
  static String configuredCount(int count) => '$count 个插件';
  static const configuration = '插件配置';
  static const expandConfiguration = '展开配置';
  static const collapseConfiguration = '收起配置';
  static const inactiveHint = '插件未启用。可以预先配置，启用后生效。';
  static const enabledHint = '配置保存后应用于已启用的插件。';
  static const interval = '刷新间隔';
  static const intervalHint = '默认每十分钟更新时间；设为 0 时，在每个适用步骤更新。';
  static const minute = '分钟';
  static const second = '秒';
  static const millisecond = '毫秒';
  static const units = <int, String>{
    60000: minute,
    1000: second,
    1: millisecond,
  };
  static const intervalInvalid = '刷新间隔须为非负数，换算后为整数毫秒，且不超过 9007199254740991 毫秒。';
  static const unitPrecisionHint = '当前数值无法用所选单位精确表示，已保留原单位。';
  static String every(int milliseconds) {
    final factor = milliseconds % 60000 == 0
        ? 60000
        : milliseconds % 1000 == 0
        ? 1000
        : 1;
    return '每 ${milliseconds ~/ factor} ${units[factor]}更新';
  }
}
