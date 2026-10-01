/// Runtime feedback. Wire identifiers, file paths and model content retain
/// their original values at the call site.
abstract final class DshRuntimeZh {
  static const workspaceWrite = '工作区内修改';
  static const fullAccess = '完全访问';
  static const readOnly = '只读';
  static const standardPreset = '标准模式';
  static const blankPreset = '空白模式';
  static const codePreset = '代码模式';
  static const messageNotFound = '无法定位该消息，请刷新索引后重试。';
  static const executionFailed = '任务执行失败';
  static const eventStreamError = '事件流错误';
  static const connectLocalService = '请先连接本机服务';
  static const workingDirectoryRequired = '请输入工作目录';
  static const messageRejected = '消息未被接受';
  static const selectWorkspace = '请先选择工作区';
  static const runCommandBeforeAttachments = '请先执行命令，再发送附件。';
  static const planCommandRejected = '计划命令未被接受';
  static const taskEditsUnsupported = '当前 Host 不支持安全保存任务编辑，请更新 Host 后重试。';
  static const selectSession = '请先选择会话';
  static const taskUpdateRejected = '任务更新未被接受';
  static const connectService = '请先连接服务';
  static const invalidWorkspaceResponse = '服务未返回有效的工作区。';
  static const titleConnectionChanged = '服务连接已改变，请重新打开标题编辑。';
  static const titleNotLoaded = '标题状态尚未加载，请读取最新状态。';
  static const openSession = '请先打开一个会话';
  static const compactionNotStarted = '压缩未开始';
  static const interactionAlreadyHandled = '此请求已经结束或已在其他客户端处理。';
  static const preferencesDirectoryUnknown =
      '无法确定用户设置目录，请配置 DSH_DESKTOP_PREFERENCES。';
  static const fixedPortRequired = '请指定大于 0 的固定端口';
  static const localServiceAddressRequired = '启动本机服务时请使用 http://127.0.0.1:端口';
  static const hostNotExecutable = '所选服务程序没有执行权限。';
  static const invalidClipboardImage = '剪贴板图片尺寸无效';
  static const clipboardEncodingFailed = '无法编码剪贴板图片';
  static const attachmentsTooLarge = '附件总大小不能超过 16 MiB';
  static const clipboardBusy = '剪贴板正被其他程序使用，请重试。';
  static const clipboardTooLarge = '附件总大小不能超过 16 MiB，图片像素不能超过 3200 万。';
  static const tooManyAttachments = '单条消息最多添加 8 个附件';
  static const clipboardReadFailed = '无法读取剪贴板图片或文件，请重新复制后重试。';
  static const clipboardImagePixelLimit = '剪贴板图片尺寸无效或像素超过 3200 万';
  static const voicePluginUnavailable = '此客户端未加载语音组件，请重新启动最新版本。';
  static const voiceReleasing = '语音识别正在释放资源，请稍后重试。';
  static const voiceTextLimit = '语音文本已达到长度上限，请结束后分段输入。';
  static const voiceUnsupported = '当前系统不支持语音识别';
  static const voiceStarting = '正在启动语音识别';
  static const voiceStopping = '正在停止语音识别';
  static const voiceListeningHint = '松开结束，识别文字实时写入';
  static const voiceIdleHint = '按住说话；空格键也可按住输入';

  static String hostVersionMismatch({
    required String address,
    required String running,
    required String expected,
  }) =>
      '$address 上运行的是 $running 版本的本机服务，与桌面版内置的 $expected 不一致，'
      '定时任务、知识库等功能可能无法使用。请在任务管理器结束旧的 deepseek-harness-rs 进程'
      '（或关闭旧版核心版服务）后重新打开桌面版。';
  static String eventConnectionInterrupted({required Object? error}) =>
      '事件连接中断，正在恢复：$error';
  static String sentButRefreshFailed({required Object? error}) =>
      '消息已发送，但刷新失败：$error';
  static String defaultModelSaveFailed({required Object? error}) =>
      '当前会话模型已切换，但默认模型保存失败：$error';
  static String planRefreshFailed({required Object? error}) =>
      '计划命令已被接受，但状态刷新失败：$error';
  static String hostExecutableRequired({required String name}) =>
      '请选择完整安装目录中的 $name';
  static String hostNotReady({required int processId}) =>
      '服务进程 $processId 尚未就绪，请检查服务日志或端口占用。';
  static String voiceStartFailed({required Object? error}) => '无法启动语音识别：$error';
  static String voiceRecognizerUnavailable({required Object? detail}) =>
      'Windows 语音识别不可用，请检查麦克风和语音语言包（$detail）。';
  static String voicePollFailed({required Object? error}) =>
      '读取语音识别状态失败：$error';
  static String voiceStopFailed({required Object? error}) => '停止语音识别失败：$error';
  static String preferencesReadFailed({required Object? error}) =>
      '无法读取桌面设置：$error';
}
