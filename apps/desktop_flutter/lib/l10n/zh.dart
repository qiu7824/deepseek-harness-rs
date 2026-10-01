/// Chinese product copy. Protocol identifiers and user content stay at their source.
abstract final class DshZh {
  static const session = '会话';
  static const newSession = '新建会话';
  static const searchSessions = '搜索会话';
  static const searchCommands = '搜索会话、页面或命令';
  static const noMatchingSessions = '没有找到匹配的会话';
  static const noMatchingCommands = '没有找到匹配的会话或操作';
  static const clearFilter = '清除筛选';
  static const sessionsGroup = '会话';
  static const pagesGroup = '页面';
  static const commandsGroup = '命令';
  static const auxiliaryModels = '辅助模型';
  static const currentExecution = '当前执行';
  static const stopExecution = '停止当前执行';
  static const send = '发送';
  static const queueMessage = '加入队列';
  static const steerExecution = '补充当前执行';
  static const connecting = '正在连接本地服务…';
  static const disconnected = '连接已断开，正在重连';
  static const reconnect = '立即重连';
  static const running = '正在执行';
  static const awaitingConfirmation = '等待你的确认';
  static const turnFinished = '本轮回复结束';
  static const saveFailed = '保存失败，修改内容已保留';
  static const retry = '重试';
  static const cancel = '取消';
  static const save = '保存';
  static const saving = '保存中…';
  static const syncing = '正在同步…';
  static const details = '技术详情';
  static const viewDetails = '查看详情';
  static const copyDetails = '复制详情';
  static const closeNotice = '关闭提示';
  static const errorTitle = '操作未完成';
  static const operationCancelled = '操作已取消';
  static const unknownError = '操作未能完成，请稍后重试。';
  static const connectionFailure = '无法连接本地服务，请检查服务状态后重试。';
  static const requestTimeout = '等待服务响应超时，请检查连接后重试。';
  static const outcomeUnknown = '服务尚未确认操作结果，请先检查当前状态，避免重复提交。';
  static const conflict = '内容已发生变化，修改内容已保留；请读取最新状态后重试。';
  static const permissionDenied = '没有执行此操作的权限，请检查授权后重试。';
  static const missingResource = '无法找到所需内容，请刷新列表或重新选择。';
  static const sessionSwitchHint = '会话区：切换会话';
  static const tabSwitchHint = '工作台：切换标签';

  static String operationFailed(String operation) => '$operation未完成';
  static String visibleSession(int index) => '切换至可见会话 $index';
  static String results(int count) => '$count 项结果';
  static String syncFailure(String title) => '“$title”同步失败';
}

abstract final class DshWindowZh {
  static const backgroundHint = '关闭窗口只关闭桌面界面，本机服务和正在执行的任务继续在后台运行。';
  static String saveFailed({required String detail}) =>
      '草稿和设置保存失败，窗口仍然打开。请检查存储空间或文件权限，再次关闭窗口以重试。$detail\n$backgroundHint';
  static String closeFailed({required String detail}) =>
      '草稿和设置已保存，但窗口未能关闭；请再次关闭窗口以重试。$detail\n$backgroundHint';
  static String protectionFailed({
    required String detail,
    required String code,
  }) => '窗口关闭时的保存保护未能启用，请重启桌面应用后重试；关闭前请等待草稿保存。$detail（$code）';
}

abstract final class DshShellZh {
  static const scheduledShort = '定时';
  static const disconnect = '断开连接';
  static const removeConnection = '移除连接';
  static const cancelOperation = '取消操作';
  static const processing = '正在处理…';
  static const connectRemoteDirectory = '连接远端工作目录';
  static const cloneWorkspace = '克隆并添加工作区';
  static String minutesAgo({required Object? count}) => '$count分钟';
  static String hoursAgo({required Object? count}) => '$count小时';
  static String daysAgo({required Object? count}) => '$count天';
  static String monthsAgo({required Object? count}) => '$count个月';
  static String yearsAgo({required Object? count}) => '$count年';
  static String accountConnected({required Object? name}) => '$name · 已连接';
  static String latestTitle({required Object? title}) => '当前标题：$title；编辑草稿已保留。';
  static String deleteWorkspaceHint({required Object? title}) =>
      '将把“$title”从工作区列表移除。文件夹和会话记录保留，会话移至未分组。';
  static String configDirectory({required Object? path}) => '配置目录：$path';
  static const session = '会话';
  static const filterSidebar = '筛选侧栏会话';
  static const justNow = '刚刚';
  static const scheduledTasks = '定时任务';
  static const knowledge = '知识库';
  static const settings = '设置';
  static const moreActions = '更多操作';
  static const refreshSessions = '刷新会话';
  static const editShortcuts = '编辑快捷键';
  static const expandSidebar = '展开侧边栏';
  static const sessionFeedback = '会话反馈';
  static const collaboration = '协作';
  static const showWorkbench = '显示工作台';
  static const resizeConversationHint = '拖动调整对话宽度；双击恢复默认';
  static const newSession = '新建会话';
  static const knowledgeShort = '知识';
  static const plugins = '插件';
  static const addWorkspace = '添加工作区';
  static const collapseSidebar = '收起侧边栏';
  static const blankSession = '新会话';
  static const workspace = '工作区';
  static const viewOptions = '视图选项';
  static const hideArchived = '隐藏已归档';
  static const allSessions = '全部会话';
  static const archivedOnly = '仅显示已归档';
  static const archiveManagement = '归档管理';
  static const connecting = '正在连接…';
  static const noSessions = '暂无会话';
  static const archived = '已归档';
  static const copySessionId = '复制会话 ID';
  static const rename = '重命名';
  static const fork = '创建分支';
  static const restoreArchive = '恢复归档';
  static const archive = '归档';
  static const connectFirst = '请先连接服务';
  static const renameSession = '重命名会话';
  static const sessionNameRequired = '请输入会话名称。';
  static const titleNotLoaded = '标题状态尚未加载，请读取最新状态。';
  static const titleConnectionChanged = '服务连接已改变，请重新打开标题编辑。';
  static const titleUnavailable = '无法读取当前标题状态。';
  static const archiveWithSchedulesTitle = '停止提醒和定时任务并归档？';
  static const archiveWithSchedulesHint = '此会话仍有有效提醒。继续归档会停止这些提醒；取消后提醒保持原状。';
  static const stopAndArchive = '停止并归档';
  static const renameWorkspace = '重命名工作区';
  static const openInFileManager = '在文件管理器中打开';
  static const deleteWorkspace = '删除工作区';
  static const workspaceConnectionChanged = '连接已切换，请重新选择工作区。';
  static const addWorkingDirectory = '添加工作目录';
  static const cloneGitDirectory = '从 Git 克隆工作目录';
  static const cloudRepository = 'Cloud · 云端 Git 仓库';
  static const sshDirectory = 'SSH 远程工作目录';
  static const feedbackTaskResult = '任务结果';
  static const feedbackInstructions = '指令遵循';
  static const feedbackInteraction = '交互体验';
  static const feedbackStability = '服务稳定性';
  static const feedbackCost = '资源与费用';
  static const feedbackSecurity = '安全、隐私与权限';
  static const feedbackOther = '其他';
  static const feedbackUnconfirmed = '服务未确认保存反馈。';
  static const feedbackHint = '记录对整个会话的意见，不会启动新的模型请求。';
  static const feedbackCategory = '反馈分类（可选）';
  static const feedbackNote = '补充说明（可选）';
  static const saveFeedback = '保存反馈';
  static const workingDirectory = '工作目录';
  static const advancedSettings = '高级设置';
  static const trashLocation = '垃圾槽位置';
  static const globalLocation = '使用全局位置';
  static const chooseDirectory = '选择目录';
  static const trashLocationHint = '留空使用全局位置。可选择空目录或已有垃圾槽，应用于此工作区的新运行。';
  static const adding = '正在添加…';
  static const add = '添加';
  static const localService = '本机服务';
  static const installedConfiguration = '使用已安装版本的配置';
  static const hostExecutable = 'Host 程序位置';
  static const chooseExecutable = '选择程序';
  static const executable = '程序';
  static const startAndConnect = '启动并连接';
  static const connect = '连接';
  static const addressAndDirectoryRequired = '请填写地址和工作目录。';
  static const portInvalid = '端口须为 1–65535 的整数。';
  static const workspaceResponseInvalid = '服务未返回有效的工作区。';
  static const cloneGitTitle = '克隆 Git 工作目录';
  static const sshExplanation =
      '通过 SSH 隧道连接远端已运行的 Harness。Agent、文件、Shell 和 PTC 均在远端运行；本机模型凭据不会复制到远端。';
  static const cloneExplanation =
      '仓库克隆到本机后，Agent 在本机目录运行，不提供云端计算。私有仓库使用已有 Git 凭据或 SSH 密钥；无需填写令牌。';
  static const sshHost = 'SSH 主机或配置别名';
  static const repositoryUrl = '仓库地址';
  static const remoteDirectory = '远端工作目录';
  static const localDirectory = '本机目标目录';
  static const missingDirectoryHint = '填写尚不存在的绝对目录';
  static const branch = '分支（可选）';
  static const defaultBranch = '留空使用仓库默认分支';
  static const sshPort = 'SSH 端口';
  static const hostPort = '远端 Harness 端口';
  static const advancedConnection = '高级连接设置';
  static const sshUser = 'SSH 用户（可选）';
  static const sshConfig = '本机 SSH 配置文件（可选）';
  static const sshPrerequisites =
      '请先在远端启动 Harness，配置 SSH 密钥或 ssh-agent，并核对主机密钥；不支持交互密码登录。';
  static const remoteConnected = '已连接 · 远端执行';
  static const connectingState = '连接中';
  static const disconnectedState = '已断开';
  static const openRemoteWorkspace = '打开远端工作区';
  static const editReconnect = '编辑／重连';
}

abstract final class DshScheduleZh {
  static String nextRun({required Object? value}) => '下次运行：$value';
  static String recentRun({required Object? value}) => '最近运行：$value';
  static String monthDay({required Object? month, required Object? day}) =>
      '$month月$day日';
  static String yearMonthDay({
    required Object? year,
    required Object? month,
    required Object? day,
  }) => '$year年$month月$day日';
  static String minutesFromNow({required Object? count}) => '$count 分钟后';
  static String hoursFromNow({required Object? count}) => '$count 小时后';
  static String daysFromNow({required Object? count}) => '$count 天后';
  static String onceAt({required Object? time}) => '单次 · $time';
  static String everyDays({required Object? count}) => '每 $count 天';
  static String everyHours({required Object? count}) => '每 $count 小时';
  static String everyMinutes({required Object? count}) => '每 $count 分钟';
  static String dailyAt({required Object? time, required Object? zone}) =>
      '每天 $time$zone';
  static String weeklyAt({
    required Object? days,
    required Object? time,
    required Object? zone,
  }) => '每$days $time$zone';
  static String storageUnavailable({required Object? detail}) =>
      '定时任务存储不可用：$detail';
  static String deliveryPersistenceFailed({required Object? detail}) =>
      '定时任务投递或回执未能持久化：$detail';
  static String sessionLabel({required Object? id}) => '会话 $id';
  static String historyTab({required Object? count}) => '运行记录 $count';
  static String retentionNotice({
    required Object? days,
    required Object? records,
  }) => '更早的记录已按保留策略清理（$days 天 / $records 条）';
  static String newWorkspaceSession({required Object? title}) =>
      '＋ 新会话 · $title';
  static String sessionTitle({required Object? title}) => '定时任务 · $title';
  static String activeCount({required Object? count}) => '定时任务 $count';
  static const enable = '启用';
  static const none = '无';
  static const neverRun = '尚未运行';
  static const title = '定时任务';
  static const monday = '周一';
  static const tuesday = '周二';
  static const wednesday = '周三';
  static const thursday = '周四';
  static const friday = '周五';
  static const saturday = '周六';
  static const sunday = '周日';
  static const upcoming = '即将运行';
  static const once = '单次';
  static const interval = '每隔';
  static const daily = '每天';
  static const weekly = '每周';
  static const runAt = '执行时间';
  static const duration = '间隔';
  static const minutes = '分钟';
  static const hours = '小时';
  static const days = '天';
  static const time = '时间';
  static const weekday = '星期';
  static const cron = 'Cron 表达式';
  static const cronHint = '5 个字段：分 时 日 月 周，例如 0 9 * * 1-5 表示工作日 9:00';
  static const timezone = '时区';
  static const connectFirst = '请先连接本机服务';
  static const unsupportedHost = '本机服务不支持定时任务，请更新到最新版本';
  static const backToSession = '返回会话';
  static const description =
      '到点后把任务作为新消息发送到原会话执行；关闭会话或重启应用后仍会按时运行。也可以直接在对话中让智能体设置。';
  static const newTask = '新建任务';
  static const all = '全部';
  static const enabled = '已启用';
  static const disabled = '已停用';
  static const search = '搜索任务';
  static const empty = '还没有定时任务\n可以在这里新建，也可以在对话中说“每个工作日 9 点汇总昨天的提交”';
  static const noMatches = '没有符合条件的任务';
  static const awaitingDelivery = '等待投递';
  static const conflict = '任务已被其他操作修改，已刷新为最新内容';
  static const closeDetails = '关闭详情';
  static const delivered = '已发送';
  static const deliveryFailed = '发送失败';
  static const agentCreated = '由智能体创建';
  static const userCreated = '由你创建';
  static const targetSessionLabel = '目标会话：';
  static const openSession = '打开会话';
  static const rule = '规则';
  static const name = '名称';
  static const prompt = '任务内容';
  static const frequency = '频率';
  static const deleteTask = '删除定时任务';
  static const deleteHint = '删除后不再运行，已发送的消息保留在会话中。';
  static const delete = '删除';
  static const running = '运行中…';
  static const runNow = '立即运行';
  static const unsaved = '有未保存的修改';
  static const saving = '正在保存…';
  static const noHistory = '还没有运行记录';
  static const deliveryMeaning = '“已发送”表示消息已进入会话，不代表任务已完成。';
  static const manual = '手动';
  static const createTitle = '新建定时任务';
  static const promptHint = '到点后要执行的指令，例如：汇总今天的新闻并列出三条要点';
  static const nameHint = '留空则使用任务内容的开头';
  static const targetSession = '目标会话';
  static const sessionRequired = '请先添加工作区或新建一个会话';
  static const targetSessionHint = '任务会发送到这个会话，由该会话的智能体执行';
  static const creating = '正在创建…';
  static const create = '创建';
}

abstract final class DshKnowledgeZh {
  static String importingFile({required String name}) => '正在导入 $name…';
  static String documentSummary({
    required Object? documents,
    required Object? chunks,
  }) => '$documents 个文档 · $chunks 段';
  static String importedDocuments({required Object? count}) => '已导入 $count 个文档';
  static String skippedDocuments({required Object? count}) => '跳过 $count 个';
  static String documentsTab({required Object? count}) => '文档 $count';
  static String deleteLibraryHint({
    required Object? name,
    required Object? count,
  }) => '删除“$name”及其中的 $count 个文档？此操作无法撤销。';
  static String chunks({required Object? count}) => '$count 段';
  static String characters({required Object? count}) => '$count 字';
  static String unknownCreateResult({required Object? message}) =>
      '$message；操作结果尚未确认，请核对知识库后再试。';
  static const disabledState = '已停用';
  static const includeInSearch = '参与检索';
  static const delete = '删除';
  static const remove = '移除';
  static const pageClosed = '知识库页面已关闭';
  static const title = '知识库';
  static const connectFirst = '请先连接本机服务';
  static const unsupportedHost = '本机服务不支持知识库，请更新到最新版本';
  static const backToSession = '返回会话';
  static const description = '导入文档后，智能体会在回答前检索已启用的知识库，并注明引用的文档。数据只保存在本机。';
  static const createTitle = '新建知识库';
  static const empty =
      '还没有知识库\n新建一个知识库，上传文档或导入整个文件夹。\n支持 PDF、Word、PowerPoint、Excel、网页、Markdown、纯文本和代码。';
  static const importing = '正在导入…';
  static const importLimit = '文件夹内容过多，只导入了前 500 个文件';
  static const documents = '文档';
  static const closeDetails = '关闭详情';
  static const name = '名称';
  static const descriptionHint = '说明（可选，告诉智能体这个知识库包含什么）';
  static const unsaved = '有未保存的修改';
  static const saving = '正在保存…';
  static const testSearch = '检索测试';
  static const deleteLibrary = '删除知识库';
  static const uploadFiles = '上传文件';
  static const importFolder = '导入文件夹';
  static const dropHint = '也可以把文件或文件夹拖到这里';
  static const noDocuments = '还没有文档';
  static const queryHint = '输入问题或关键词';
  static const search = '检索';
  static const searchMeaning = '这里的结果就是智能体调用 knowledge_search 时看到的内容。';
  static const disabled = '知识库已停用，智能体不会检索它。';
  static const noResults = '没有找到相关内容';
  static const nameHint = '名称，例如：产品手册';
  static const optionalDescription = '说明（可选）';
  static const creating = '正在创建…';
  static const create = '创建';
}

abstract final class DshSettingsZh {
  static String serverStatus({
    required Object? status,
    required Object? tools,
    required bool hasSecrets,
  }) => '$status · $tools 个工具${hasSecrets ? credentialsSuffix : ''}';
  static String operationLabel({required String action}) => '操作：$action';
  static String weekdayLabel({required int day}) =>
      '周${const ['一', '二', '三', '四', '五', '六', '日'][day - 1]}';
  static String deliveryDetails({
    required Object? deliveredAt,
    required Object? scheduledAt,
    required Object? messageId,
    Object? prompt,
  }) =>
      '已发送到会话：$deliveredAt\n计划时间：$scheduledAt\n消息：$messageId${prompt == null ? '' : '\n$prompt'}';
  static String everyMillis({required Object? millis}) => '每 $millis 毫秒更新';
  static String errorDraftRetained({required Object? message}) =>
      '$message；草稿已保留。';
  static String latestConfiguration({required Object? description}) =>
      '已读取最新配置：$description。草稿已保留，请核对后保存。';
  static String driverModelSuffix({required Object? model}) => ' · 驱动 $model';
  static String unavailableProvider({required Object? provider}) =>
      '$provider · 不可用';
  static String afterSeconds({required Object? seconds}) => '延迟 $seconds 秒，一次';
  static String everySeconds({required Object? seconds}) => '每 $seconds 秒';
  static String dailyAt({required Object? time, required Object? timezone}) =>
      '每天 $time · $timezone';
  static String weeklyAt({
    required Object? days,
    required Object? time,
    required Object? timezone,
  }) => '周 $days $time · $timezone';
  static String deleteReminderHint({required Object? title}) =>
      '删除“$title”及其发送回执，后续不再发送。会话中的消息保留。';
  static String sessionLabel({required Object? title}) => '会话：$title';
  static String lastDelivery({required Object? time}) => '最近发送到会话：$time';
  static String secondsInvalid({required Object? minimum}) =>
      '秒数必须为 $minimum 到 9007199254740991 的整数。';
  static String latestReminder({
    required Object? title,
    required Object? prompt,
    required Object? scheduledAt,
  }) => '已读取最新版本，草稿保留，请比较后保存。\n最新标题：$title\n最新提示：$prompt\n最新计划：$scheduledAt';
  static String boundSessionHint({
    required Object? title,
    required Object? id,
  }) => '绑定会话：$title\n$id\n提醒始终绑定此会话。';
  static String latestRetention({
    required Object? days,
    required Object? records,
  }) => '已读取最新配置，草稿保留。最新值：$days 天 / $records 条；请比较后保存。';
  static String retentionSaveFailed({required Object? detail}) =>
      '$detail\n草稿已保留，可读取最新配置后再保存。';
  static String receiptsTitle({required Object? title}) => '发送回执 · $title';
  static String retentionSummary({
    required Object? days,
    required Object? records,
  }) => '保留范围：$days 天，最多 $records 条。';
  static String searchItems({required Object? title}) => '搜索$title';
  static String updatedAtSuffix({required Object? time}) => '   更新于 $time';
  static String permanentDeleteTitle({required Object? title}) =>
      '永久删除“$title”？';
  static String mcpTestSucceeded({
    required Object? name,
    required Object? count,
  }) => '$name 连接成功，可用工具 $count 个';
  static String mcpSavedConnectionFailed({required Object? detail}) =>
      '配置已保存，连接失败：$detail';
  static String removeResourceTitle({
    required Object? kind,
    required Object? name,
  }) => '移除$kind“$name”？';
  static String stringMapRequired({required Object? label}) =>
      '$label必须为字符串键值组成的 JSON 对象';
  static String pluginStatusFailed({required Object? detail}) =>
      '无法读取插件操作状态：$detail';
  static String operationId({required Object? id}) => '操作标识：$id';
  static String showModel({required Object? id}) => '显示模型 $id';
  static String deleteModelLabel({required Object? id}) => '删除模型 $id';
  static String deleteLearningHint({required Object? title}) =>
      '删除“$title”后将停止复用此条记录。';
  static String learningRecordCount({required Object? count}) =>
      '经验记录 · $count';
  static String candidateCount({
    required Object? title,
    required Object? count,
  }) => '$title · $count 条';
  static String learningBudget({
    required Object? used,
    required Object? budget,
  }) => '预览不会增加复用次数 · $used / $budget 字符';
  static String excludedLearning({required Object? count}) =>
      '另有 $count 条因工具可用性、规则或预算条件未纳入。';
  static String learningUsage({
    required Object? workspace,
    required Object? occurrences,
    required Object? applications,
  }) => '$workspace · 发生 $occurrences 次 · 复用 $applications 次';
  static String errorCode({required Object? code}) => '错误代码：$code';
  static String diagnosticSession({required Object? id}) => '任务：$id';
  static String diagnosticCall({required Object? id}) => '调用：$id';
  static String credentialsUnavailable({required Object? detail}) =>
      '凭据状态暂不可用：$detail';
  static String modelsRefreshFailed({required Object? detail}) =>
      '设置已保存，模型列表刷新失败：$detail';
  static String removeModelHint({required Object? name}) => '移除 $name，保留历史记录。';
  static String deleteConnectionHint({
    required Object? name,
    required Object? keyEffect,
  }) => '删除 $name 的连接配置$keyEffect，保留会话记录。';
  static String modelCatalogCount({
    required Object? enabled,
    required Object? total,
  }) => '模型目录 · $enabled / $total 显示';
  static String moreModels({required Object? count}) => '还有 $count 个模型';
  static String logoutRecheckRequired({required Object? detail}) =>
      '$detail；请重新查询影响并确认后再退出。';
  static String logoutImpact({required Object? count}) =>
      '退出将停止绑定此账号的 $count 个运行中任务。';
  static String unknownLogoutImpact({required Object? detail}) =>
      '影响未知，请重试后再退出。 $detail';
  static String accountRefreshFailed({required Object? detail}) =>
      '账号已连接，但刷新列表失败：$detail';
  static const intervalMillisInvalid = '刷新间隔须为 0–9007199254740991 的整数（毫秒）。';
  static const skillFieldsRequired = '请填写名称、说明、项目目录和技能正文。';
  static const skillRevisions = '技能版本';
  static const closeSkillRevisions = '关闭技能版本';
  static const skillRevisionsHint = '手动选择对应项目使用的技能版本。编辑会保存新版本，启用、撤回和恢复均由你选择。';
  static const enableProjectSkills = '启用项目技能版本';
  static const viewRevision = '查看版本';
  static const skillPreviewTruncated = '正文较长，显示前 64000 个字符。';
  static const saveFullContent = '保存完整正文';
  static const defaultPermissionHint = '选择新会话的默认权限模式';
  static const timestampOffsetRequired =
      '指定时间须含时区偏移，例如 2030-01-01T09:00:00+08:00。';
  static const timezoneRequired =
      '请输入明确的 IANA 时区，例如 Asia/Shanghai、America/New_York 或 UTC；不使用 CST 等缩写。';
  static const timeFormatInvalid = '时间使用 24 小时制 HH:mm 或 HH:mm:ss，可带最多三位毫秒。';
  static const dateFormatInvalid = '日期使用 YYYY-MM-DD。';
  static const cronInvalid = 'Cron 需要五段：分 时 日 月 周。';
  static const retentionInvalid = '保留天数须为 1–3650，条数须为 1–10000，均为整数。';
  static const restore = '恢复';
  static const permanentDeleteHint =
      '此操作会永久删除此会话及其所有子智能体的历史记录，并移除对应列表引用；独立分支会话和工作区文件保留，无法恢复。';
  static const permanentDelete = '永久删除';
  static const testConnection = '测试';
  static const removeMcpServer = '移除 MCP 服务器';
  static const removeSkillHint = '技能文件将移入本机回收目录。';
  static const removeMcpHint = '移除服务器配置并断开连接，其工具将不再可用。';
  static const remove = '移除';
  static const mcpNameInvalid = '名称须为 1–32 个字母、数字、下划线或连字符';
  static const mcpArgumentsInvalid = '参数必须为字符串组成的 JSON 数组';
  static const pluginSourceTooLong = '请输入不超过 400 字节的插件来源或包名。';
  static const pluginSourceInvalid = '安装来源须为 github:owner/repo#完整的 40 位提交 SHA。';
  static const providerIdInvalid = '请使用不重复、以字母开头的提供方 ID。';
  static const noLearningMatches = '没有匹配的自动经验记录';
  static const learningLoadFailed = '经验目录读取失败，可刷新重试';
  static const noVerifiedLearning = '当前没有符合条件的已验证经验。';
  static const viewCandidateContext = '查看候选上下文内容';
  static const diagnosticOnly = '仅用于排障，不注入后续模型上下文。';
  static const deleteRecord = '删除记录';
  static const correctionLengthInvalid = '请填写 1–1000 字的修正步骤与适用条件。';
  static const refreshCatalog = '刷新目录';
  static const searchModelNames = '搜索模型名称或 ID';
  static const addModelProvider = '添加提供方';
  static const subscriptionLoginHint = '使用供应商订阅登录，凭据保存在本机；支持续期的供应商会自动续期。';
  static const loginAnotherAccount = '登录另一个账号';
  static const signOut = '退出登录';
  static const browserUnavailable = '无法打开系统浏览器';
  static const copyVerificationCode = '复制验证码';
  static const awaitingAuthorization = '等待授权完成…';
  static const checkAgain = '重新检查';
  static const loginAgain = '重新登录';
  static const everyApplicableStep = '每个适用步骤更新';
  static const outcomeUnknownSuffix = ' 操作结果尚未确认，请刷新核对。';
  static const nextScheduledTime = '下次计划时间';
  static const scheduledTime = '计划时间';
  static const skill = '技能';
  static const loginRequestInvalid = '服务未返回有效的登录请求';
  static const loginUrlInvalid = '服务未返回有效的 HTTPS 授权地址';
  static const loginExpired = '授权已过期，请重新登录。';
  static const loginRetrying = '连接暂时中断，正在重试当前登录请求。';
  static const loginEnded = '登录已取消或结束，请重新登录。';
  static const connectionChanged = '连接已变化，请关闭后重新打开设置。';
  static const learningResponseInvalid = '自动经验目录返回的数据不完整。';
  static const learningPreviewMismatch = '经验预览与当前任务不匹配。';
  static const deleteLearningTitle = '删除自动经验记录？';
  static const thisRecord = '此记录';
  static const learning = '自动经验';
  static const refreshLearning = '刷新自动经验';
  static const learningDescription =
      '记录工具失败与恢复，在后续任务中复用已验证的修正建议。运行诊断单独保留，不作为长期规则。';
  static const captureLearning = '自动捕获与复用';
  static const memoryDisabledLearning = '持久记忆已关闭，自动经验暂停捕获与复用；已有记录仍可查看。';
  static const searchLearning = '搜索工具、错误代码或修正建议';
  static const allStatuses = '全部状态';
  static const unverified = '待验证';
  static const verified = '已验证';
  static const moreLearning = '显示更多经验';
  static const currentLearning = '当前任务候选经验';
  static const selectLearningSession = '选择任务后查看下次请求的候选经验';
  static const learningBudgetHint = '实际请求会重新检查匹配条件与预算。';
  static const historicalLearningHint = '历史预览仅覆盖有界记录，实际执行前重新读取工具目录。';
  static const learningShort = '经验';
  static const diagnostics = '运行诊断';
  static const learningRecords = '经验记录';
  static const unnamedWorkspace = '工作区未命名';
  static const userVerified = '验证方式：用户确认';
  static const recoveredTool = '验证方式：已观察到匹配工具恢复';
  static const noRecord = '未记录';
  static const noDiagnosticDetails = '未记录诊断详情';
  static const diagnosticChanged = '新增事件，需重新核对';
  static const diagnosticChecked = '已记录诊断核查结论';
  static const confirmLearningTitle = '确认修正建议';
  static const confirmLearningHint = '只确认已经检查的修正建议；记录为用户确认，不代表已观察到工具恢复。';
  static const correctionSteps = '修正步骤与适用条件';
  static const confirmCorrection = '确认此建议';
  static const connectFirst = '请先连接服务';
  static const modelConnectionChanged = '连接已变化，请关闭后重新打开模型设置。';
  static const switchConnection = '切换连接';
  static const discardModelHint = '模型修改尚未保存，切换将放弃修改。';
  static const discardAndSwitch = '放弃并切换';
  static const connectionSaved = '连接已保存';
  static const modelFieldsInvalid = '请修正模型 ID 或容量；容量支持正整数或 K/M。';
  static const connectionConflict = '连接配置已在其他窗口改变，请先刷新并核对草稿。';
  static const modelsSaved = '模型设置已保存';
  static const models = '模型';
  static const modelsDescription = '选择连接，管理模型显示与参数；订阅登录在账号页管理。';
  static const apiConnections = 'API 连接';
  static const subscriptionAccounts = '订阅账号';
  static const taskRoles = '辅助模型';
  static const unsavedModels = '未保存的模型修改';
  static const discardCatalogHint = '切换页面将放弃模型目录草稿。';
  static const discardChanges = '放弃修改';
  static const deleteModel = '删除模型';
  static const deleteApiConnection = '删除 API 连接';
  static const deleteApiKeyAlso = '和此连接保存的 API 密钥';
  static const subscriptionManaged = '此连接由订阅账号管理，请在订阅账号页续期或退出。';
  static const apiKey = 'API 密钥';
  static const replaceApiKey = '已配置——输入新值可替换';
  static const apiKeyHint = '输入 API 密钥';
  static const customSettings = '自定义设置';
  static const displayName = '显示名称';
  static const unsavedChanges = '未保存的修改';
  static const discardConnectionHint = '添加连接前是否放弃当前模型和连接草稿？';
  static const discardAndContinue = '放弃并继续';
  static const searchModels = '搜索连接或模型 ID';
  static const repairOnly = '仅待修复';
  static const noProviders = '没有匹配的提供方';
  static const apiKeyConfigured = '已配置 API 密钥';
  static const apiKeyMissing = '缺少 API 密钥';
  static const editConnection = '编辑连接';
  static const deleteConnection = '删除连接';
  static const collapseModels = '收起模型';
  static const expandModels = '展开模型';
  static const allModels = '全部模型';
  static const show = '显示';
  static const hide = '隐藏';
  static const showFiltered = '显示筛选结果';
  static const hideFiltered = '隐藏筛选结果';
  static const loadingModels = '正在读取模型目录…';
  static const noModels = '没有匹配的模型';
  static const addManualModel = '添加手动模型';
  static const visibilityHint = '显示开关仅控制模型选择列表；保存后生效，不会删除会话或更改运行中的任务。';
  static const invalidCapacity = '容量必须为正整数，可使用 K/M。';
  static const dirtyModels = '模型有未保存修改';
  static const saveModels = '保存模型';
  static const accountLogin = '账号登录';
  static const connected = '已连接';
  static const disconnected = '未连接';
  static const subagentSuffix = ' · 子智能体';
  static const refresh = '刷新';
  static const reconnect = '重新连接';
  static const login = '登录';
  static const installClient = '安装官方客户端';
  static const loginRequired = '需要重新登录';
  static const currentAccount = '当前账号';
  static const switchAccount = '切换';
  static const removeAccount = '移除账号';
  static const accountRemoved = '已退出账号；该账号任务已停止，排队内容保留且不会自动继续。';
  static const accountConnectionChanged = '连接已变化，请重新打开账号设置。';
  static const signOutTitle = '确认退出账号';
  static const checkingAccount = '正在核对该账号的任务…';
  static const unknownAccountImpact = '影响未知，请重试后再退出。';
  static const signOutHint = '排队内容和会话记录保留，任务不会自动继续；API 密钥任务和其他账号的任务不受影响。';
  static const retryAccountImpact = '重试影响查询';
  static const confirmSignOut = '确认退出此账号';
  static const connectSubscription = '连接订阅账号';
  static const officialClientLoginHint = '请在官方客户端中完成授权；此处会自动同步登录结果。';
  static const browserLoginHint = '在系统浏览器中完成授权；登录结果和账号配置由本机服务统一保存。';
  static const openAuthorization = '打开授权页面';
  static const newManualModel = '新手动模型';
  static const modelId = '模型 ID';
  static const capacity = '容量';
  static const contextLength = '上下文长度';
  static const maxOutputTokens = '最大输出 Token';
  static const removeDraft = '移除草稿';
  static const providerFieldsInvalid = '请填写有效 URL、协议和不重复的模型 ID；容量支持正整数或 K/M。';
  static const addProvider = '添加自定义提供方';
  static const customProvider = '自定义提供方';
  static const noApiKey = '无需 API 密钥';
  static const providerId = '提供方 ID';
  static const protocol = '接口协议';
  static const addModel = '添加模型';
  static const retryKeySave = '重试保存密钥';
  static const add = '添加';
  static const processing = '正在处理';
  static const cancellingCleanup = '正在取消并清理';
  static const awaitingWebConfirmation = '等待网页客户端确认';
  static const restoring = '正在恢复原状态';
  static const saving = '正在保存';
  static const completed = '已完成';
  static const operationFailed = '操作失败';
  static const cancelled = '已取消';
  static const interrupted = '操作被中断';
  static const recoveryRequired = '需要恢复检查';
  static const pluginStatusInvalid = '插件操作状态不完整或版本不受支持';
  static const pluginConnectFirst = '请先连接服务，再重新打开插件设置。';
  static const pluginConnectionChanged = '连接已变化，请关闭后重新打开插件设置。';
  static const pluginResponseInvalid = '插件管理响应不完整';
  static const pluginOperationMissing = '插件管理响应缺少操作标识';
  static const pluginCancelMismatch = '取消结果与指定操作不一致';
  static const pluginOutcomeUnknown = '操作结果需要核对，正在重新读取后台状态。';
  static const installUpdateRemove = '安装、更新与卸载';
  static const pluginSourceHint = '支持纯 Web 插件，安装来源须固定到完整提交 SHA；插件界面在网页端运行。';
  static const pluginBackgroundHint = '关闭面板不会取消后台操作，重新打开可继续查看进度。';
  static const installUpdate = '安装 / 更新';
  static const uninstall = '卸载';
  static const refreshStatus = '刷新状态';
  static const githubSource = 'GitHub 来源';
  static const installedPackage = '已安装包名';
  static const pinnedSourceHint = 'github:owner/repo#完整提交 SHA';
  static const inspectOperation = '检查操作';
  static const restoreConfiguration = '恢复上次有效配置';
  static const pluginInstallConfirmation = '插件将在网页端运行，请确认信任此来源并安装或更新：';
  static const pluginUninstallConfirmation = '确认卸载此插件？提交后须重启应用使变更生效。';
  static const pluginRestoreConfirmation = '确认用上次有效配置恢复当前插件配置？';
  static const pluginCancelConfirmation = '确认取消这项后台操作？清理完成前请勿重复提交。';
  static const confirmOperation = '确认操作';
  static const backToEdit = '返回编辑';
  static const restoreSettings = '恢复配置';
  static const enable = '启用';
  static const disable = '停用';
  static const cancelBackground = '取消后台操作';
  static const pluginRestartRequired = '变更已提交，请重启应用使插件配置生效。';
  static const plugins = '插件';
  static const agentPresets = 'Agent 预设';
  static const skillsMcp = '技能与 MCP';
  static const archivedSessions = '归档会话';
  static const memoryContext = '记忆与上下文';
  static const toolDiscovery = '工具发现';
  static const installationMaintenance = '安装与维护';
  static const revisionsValidation = '版本与验证';
  static const addSkill = '添加技能';
  static const archiveHint = '归档只会隐藏会话，完整记录仍会保留。你可以恢复或永久删除记录。';
  static const noRecords = '暂无记录';
  static const voiceInput = '使用语音输入';
  static const openPlugin = '打开插件';
  static const editSkill = '编辑技能';
  static const removeSkill = '移除技能';
  static const viewPreset = '查看预设';
  static const copyPreset = '复制预设';
  static const editMemory = '编辑记忆';
  static const deleteMemory = '删除记忆';
  static const delete = '删除';
  static const pluginMaintenance = '插件安装与维护';
  static const closePluginManagement = '关闭插件管理';
  static const ungrouped = '未分组';
  static const unnamedSession = '未命名会话';
  static const mcpServers = 'MCP 服务器';
  static const addServer = '添加服务器';
  static const disabled = '已停用';
  static const connectionFailed = '连接失败';
  static const awaitingConnection = '等待连接';
  static const credentialsSuffix = ' · 已配置凭证';
  static const discoverToolsOnDemand = '按需发现工具';
  static const restartRequired = '重启服务后生效';
  static const duplicateMcpServer = '同名 MCP 服务器已存在，请使用编辑。';
  static const name = '名称';
  static const content = '内容';
  static const close = '关闭';
  static const executableRequired = '请填写可执行程序';
  static const serverUrlInvalid = '请填写有效的 HTTP 或 HTTPS 服务器地址';
  static const environmentVariables = '环境变量';
  static const requestHeaders = '请求头';
  static const addMcpServer = '添加 MCP 服务器';
  static const editMcpServer = '编辑 MCP 服务器';
  static const localCommand = '本地命令（stdio）';
  static const executable = '可执行程序';
  static const executableHint = 'npx / python / 可执行文件路径';
  static const argumentsJson = '参数（JSON 数组）';
  static const workingDirectory = '工作目录';
  static const defaultWorkingDirectory = '留空使用运行目录';
  static const environmentJson = '环境变量（JSON 对象）';
  static const retainSecretHint = '留空保留已有值；{} 清空';
  static const serverUrl = '服务器地址';
  static const headersJson = '请求头（JSON 对象）';
  static const enableServer = '启用服务器';
  static const serverSaveHint = '保存并启用将启动本地命令或连接服务器；凭证保存在本机。';
  static const delayedOnce = '延迟一次';
  static const scheduledOnce = '指定时间';
  static const fixedInterval = '固定间隔';
  static const daily = '每天';
  static const weekly = '每周';
  static const reminderConflict = '提醒已被其他操作更新；草稿已保留，请读取最新版本后再保存。';
  static const reminderDeleted = '提醒已被删除，请刷新目录。';
  static const reminderEnded = '此提醒已结束，不能修改；可新建提醒。';
  static const reminderPluginDisabled = '提醒插件已停用；请在目录中手动启用后再保存。';
  static const reminderSessionArchived = '绑定会话已归档，不能接收提醒。';
  static const reminderSessionDeleted = '绑定会话已删除，不能接收提醒。';
  static const onceAt = '指定时间，一次';
  static const reminderConnectionChanged = '连接已变化，请重新打开全局提醒。';
  static const pluginCatalogInvalid = '插件目录返回的数据不完整。';
  static const multipleReminderPlugins = '存在多个提醒插件，请先在插件设置中核对。';
  static const deleteReminderTitle = '删除提醒？';
  static const reminderDeletedNotice = '提醒已删除。';
  static const reminders = '全局提醒';
  static const remindersDescription =
      '提醒绑定会话，由 Host 到时发送；客户端可关闭，Host 服务需要保持运行。发送回执表示消息已送入会话，不代表模型已执行成功。';
  static const reminderEnabled = '提醒已启用';
  static const reminderDisabledHint =
      '提醒默认关闭；启用后才会发送或允许新建、修改。停用期间仍可查看目录、回执和删除提醒。';
  static const reminderEnabledNotice = '提醒已启用。';
  static const reminderDisabledNotice = '提醒已停用。';
  static const reminderPluginMissing = '未找到提醒插件；请检查 Host 插件库存。';
  static const createReminder = '新建提醒';
  static const receiptRetention = '回执保留设置';
  static const remindersRetryRequested = '已请求重新检查待发送提醒，请查看发送回执。';
  static const retryReminders = '重试待发送提醒';
  static const all = '全部';
  static const active = '有效';
  static const ended = '已结束';
  static const searchReminders = '搜索标题、提示或会话';
  static const noReminders = '没有匹配的提醒';
  static const edit = '编辑';
  static const deliveryReceipts = '发送回执';
  static const reminderDraftStale = '会话或连接已变化，此草稿已停止提交；可复制内容后重新打开。';
  static const weekdaysRequired = '至少选择一个星期。';
  static const reminderFieldsRequired = '请选择绑定会话，并填写标题和发送给会话的提示。';
  static const reminderMissing = '提醒不存在';
  static const reminderEndedState = '提醒已结束';
  static const editReminder = '编辑提醒';
  static const boundSession = '绑定会话';
  static const reminderSessionRequired = '请先创建会话，然后再设置提醒。';
  static const title = '标题';
  static const sessionPrompt = '发送给会话的提示';
  static const editTrigger = '修改触发规则';
  static const trigger = '触发规则';
  static const delayedRescheduleHint = '延迟提醒若需改期，请指定新的发送时间。';
  static const delaySeconds = '延迟秒数（至少 1 秒）';
  static const intervalSeconds = '间隔秒数（至少 60 秒）';
  static const dateTimeZone = '日期、时间和 IANA 时区';
  static const zonedTime = '含时区偏移的时间';
  static const timestamp = '发送时间（RFC 3339）';
  static const date = '日期';
  static const time24 = '时间（24 小时制）';
  static const cron = 'Cron 表达式（分 时 日 月 周）';
  static const cronHint = '例如 0 9 * * 1-5 表示工作日 09:00；最小间隔为 1 分钟。';
  static const ianaTimezone = 'IANA 时区';
  static const timezoneHint =
      '明确使用 Asia/Shanghai、America/New_York 或 UTC；不使用 CST 等歧义缩写。';
  static const reminderDraftDisabled = '提醒已停用，草稿保留；请在目录中手动启用后再保存。';
  static const refreshKeepDraft = '读取最新版本，保留草稿';
  static const saveReminder = '保存提醒';
  static const retentionDraftStale = '会话或连接已变化，草稿停止提交；请重新打开设置。';
  static const retentionResponseInvalid = '回执配置返回的数据不完整。';
  static const retentionSaved = '保留设置已保存；新投递时应用，不会立即清理已有回执。';
  static const retentionHint =
      '默认保留 30 天、最多 200 条。新投递时应用保留策略；查看记录和保存设置不会立即清理回执，也不会启用提醒。';
  static const retentionDays = '保留天数（1–3650）';
  static const retentionCount = '最多条数（1–10000）';
  static const refreshConfiguration = '读取最新配置';
  static const saveRetention = '保存保留设置';
  static const receiptsConnectionChanged = '会话或连接已变化，请重新打开回执。';
  static const receiptsMeaning = '按发送顺序从新到旧排列；仅表示消息已发送到绑定会话，不代表模型执行成功。';
  static const receiptsPruned = '更早回执已按保留策略清理。';
  static const receiptsUnavailable = '部分更早回执不可用。';
  static const noReceipts = '暂无发送回执';
  static const earlierReceipts = '加载更早回执';
  static const refreshReceipts = '刷新回执';
  static const general = '通用设置';
  static const runtimeEnvironment = '目录与运行环境';
  static const collaboration = '协作';
  static const security = '安全盾';
  static const archiveManagement = '归档管理';
  static const menuSettings = '小菜单设置';
  static const trash = '垃圾槽';
  static const discardModelConnectionHint = '切换页面将放弃模型和连接草稿。';
  static const settingsConnectionChanged = '连接已变化，修改内容已保留，请重新打开设置。';
  static const settingsSaved = '设置已保存';
  static const discardTitle = '保留未保存的修改？';
  static const discardHint = '当前设置尚未保存，关闭将放弃这些修改。';
  static const settings = '设置';
  static const openConfig = '打开配置文件';
  static const searchSettings = '搜索设置页面';
  static const unsaved = '有未保存的修改';
  static const conversationFontSize = '对话字号';
  static const conversationFontHint = '调整回复正文的字号；界面继续跟随系统文字缩放。';
  static const defaultFontSize = '15（默认）';
  static const defaultPresetHint = '对此后新建的会话生效。运行中的会话保持它开始时的预设。';
  static const language = '语言';
  static const chinese = '中文';
  static const appearance = '外观';
  static const dark = '深色';
  static const light = '浅色';
  static const busyEnter = '繁忙时 Enter 键行为';
  static const busyEnterHint = '仅在智能体运行时生效；Cmd/Ctrl+Enter 使用另一行为';
  static const replyHints = '回复提示显示';
  static const replyHintsDescription = '工具、思考、任务和运行提示的显示方式。';
  static const iconAndText = '图标＋文字';
  static const iconsOnly = '仅图标';
  static const textOnly = '仅文字';
  static const composerTips = '输入提示';
  static const composerTipsHint = '输入框获得焦点时显示一条随机使用提示，可随时关闭。';
  static const on = '开启';
  static const directoryPicker = '目录选择器';
  static const directoryPickerHint = '选择工作区、垃圾槽等目录时使用 Windows 目录选择器。';
  static const systemDirectoryPicker = '系统目录选择器';
  static const shortcuts = '快捷键';
  static const shortcutsHint = '设置侧边栏、搜索、新会话及工作台的组合键。';
  static const shortcutBindings = '快捷键绑定';
  static const computerUseHint =
      '模型与工作台共用控制环境。本机桌面与 UU 自连都会共享这台电脑的键鼠；需要同时使用本机其他软件时，可改用另一台 UU 设备或隔离浏览器。';
  static const enableComputerUse = '启用 Computer Use';
  static const nativeComputerProtocol = '原生 Computer 协议';
  static const nativeControlTarget = '原生协议控制目标';
  static const executionAdapter = '执行适配器';
  static const externalControlCommand = '外部控制命令';
  static const browserExecutable = '浏览器可执行文件';
  static const headlessBrowser = '后台运行浏览器';
  static const maxBrowserSessions = '最大浏览器会话数';
  static const operationTimeout = '操作超时（秒）';
  static const permission = '权限';
  static const workbenchWidth = '工作台宽度';
  static const rememberWidth = '记住宽度';
  static const openFullscreen = '打开时全屏';
  static const showFiles = '显示文件入口';
  static const showGit = '显示 Git 入口';
  static const showWeb = '显示网页入口';
  static const showTerminal = '显示终端入口';
  static const dataDirectory = '数据目录';
  static const cacheDirectory = '缓存目录';
  static const runtimeDirectory = '运行环境目录';
  static const testDirectory = '测试目录';
  static const maxTeamMembers = '团队成员上限';
  static const showTeamButton = '显示团队按钮';
  static const defaultCollaboration = '默认协作模式';
  static const parallelAgents = '并行子智能体';
  static const nestingDepth = '嵌套深度';
  static const turnLimit = '回合上限';
  static const timeout = '超时（秒）';
  static const defaultProvider = '默认提供方';
  static const defaultModel = '默认模型';
  static const reasoningEffort = '推理强度';
  static const outputTokenLimit = '输出 Token 上限';
  static const toolPresentation = '工具调用呈现';
  static const serviceTier = '服务等级';
  static const requestRetries = '请求重试次数';
  static const approvalTimeout = '审批等待时长（秒）';
  static const unattendedPolicy = '无人值守策略';
  static const highRiskPolicy = '高风险工具策略';
  static const outsideWorkspacePolicy = '工作区外写入策略';
  static const sensitiveFilePolicy = '敏感文件读取策略';
  static const credentialCommandPolicy = '凭据命令策略';
  static const userProfile = '用户资料';
  static const memoryBudget = '记忆预算';
  static const profileBudget = '资料预算';
  static const provider = '提供方';
  static const contextEngine = '上下文引擎';
  static const autoCompaction = '自动压缩';
  static const compactionThreshold = '压缩阈值';
  static const compactionTarget = '压缩目标';
  static const recentMessages = '保护最近消息数';
  static const roleDescription = '角色描述';
  static const roleSupplement = '角色补充';
  static const includeHarnessIdentity = '包含 Harness 身份';
  static const includeEnvironment = '包含运行环境';
  static const autoCleanup = '自动清理';
  static const compactContext = '精简上下文';
  static const retainedDays = '保留天数';
  static const failedArtifactDays = '失败产物保留天数';
  static const recoveryDays = '恢复期（天）';
  static const storageSoftLimit = '容量软上限（GiB）';
  static const location = '位置';
  static const clientPath = '客户端程序路径';
  static const account = '账号';
  static const deviceId = '设备 ID';
  static const trace = '轨迹';
  static const artifacts = '产物';
  static const codeGraph = '代码图谱';
  static const context = '上下文';
  static const tasks = '任务';
  static const localDesktop = '本机桌面';
  static const isolatedBrowser = '隔离浏览器';
  static const builtInBrowser = '内置浏览器';
  static const nativeDesktop = '本机桌面（Rust 原生）';
  static const remoteDesktop = 'UU 远程桌面';
  static const externalCommand = '外部命令';
  static const steerExecution = '转向当前任务';
  static const readOnly = '只读';
  static const workspaceWrite = '工作区内修改';
  static const fullAccess = '完全访问';
  static const ask = '询问';
  static const deny = '拒绝';
  static const allow = '允许';
  static const auto = '自动';
  static const standardMode = '标准模式';
  static const codeMode = '代码模式';
  static const blankMode = '空白模式';
  static const native = '原生';
  static const inherit = '继承';
  static const settingsRestartRequired = '这些设置需要重启服务后生效';
  static const skillConnectionChanged = '连接已变化，请关闭后重新打开技能版本。';
  static const skillVersionSaved = '版本已保存，尚未启用';
  static const operationSaved = '操作已保存';
  static const withdrawn = '已撤回';
  static const enabled = '已启用';
  static const inactive = '未启用';
  static const refreshRevisions = '刷新版本';
  static const createRevision = '创建技能版本';
  static const noRevisions = '暂无技能版本。';
  static const enableRevision = '启用版本';
  static const restoreRevision = '恢复版本';
  static const withdrawRevision = '撤回版本';
  static const saveRevision = '保存技能版本';
  static const revisionHint = '同名技能的旧版本仍会保留，保存后需要单独启用。';
  static const skillName = '技能名称';
  static const description = '说明';
  static const projectDirectory = '项目目录';
  static const skillBody = '技能正文';
  static const saveNewRevision = '保存新版本';
  static const cancelEdit = '取消编辑';
  static const editAsRevision = '编辑为新版本';
  static const closeDetails = '关闭详情';
  static const debugRole = '排错';
  static const optimizeRole = '优化';
  static const visionRole = '看图';
  static const imageRole = '生图';
  static const searchRole = '搜索';
  static const capabilityVerified = '已验证可用';
  static const temporaryFailure = '暂时失败';
  static const unsupported = '不支持';
  static const authorizationRequired = '需要授权';
  static const unconfigured = '未配置';
  static const credentialsConfigured = '凭据已配置';
  static const credentialsNotRequired = '无需凭据';
  static const credentialsRequired = '需要登录或更新凭据';
  static const capabilityUnverified = '尚未验证';
  static const toolRegistered = '工具已注册';
  static const toolUnregistered = '工具未注册';
  static const capabilityScope = '记录仅适用于下列模型与操作；执行权限在每次调用时检查。';
  static const expiredRecord = '（记录已过期）';
  static const saved = '已保存';
  static const incompatibleConnection = '此连接不支持该图像或托管搜索工具，请选择兼容连接或清空分工。';
  static const unavailableConnection = '该连接已不可用，请重新选择。';
  static const compatibleConnection = '连接协议兼容；具体模型能力和账号权限仍需验证。';
  static const followConnectionHint = '跟随当前会话连接；分工留空不会增加该连接的工具能力。';
  static const capabilityUnknown = '当前服务尚未提供兼容性信息，工具可用性未验证。';
  static const followSession = '跟随当前会话';
  static const unsupportedSuffix = ' · 不支持';
  static const reasoningLevel = '推理等级';
  static const reload = '重新加载';
  static const refreshCapabilities = '刷新能力记录';
  static const saveAuxiliaryModels = '保存辅助模型';
  static const timeContextStale = '连接或插件已变化，草稿已保留；请重新打开时间上下文设置。';
  static const timeContextResponseInvalid = '时间上下文配置返回的数据不完整，请重新读取。';
  static const timeContextInvalid = '时间上下文配置格式无效，请检查插件配置。';
  static const systemTimezone = '系统时区';
  static const timeContextConflict = '配置已在其他位置更改，草稿已保留；请读取最新配置并核对后再保存。';
  static const timeContextOutcomeUnknown = '保存结果尚未确认，草稿已保留；请读取最新配置后核对。';
  static const timeContextFailure = '配置操作失败，草稿已保留；请重新读取后重试。';
  static const timeContextSaved = '时间上下文配置已保存，启停状态保持不变。';
  static const timeContext = '时间上下文';
  static const timeContextHint = '默认每十分钟更新时间；间隔为 0 时在每个适用步骤更新。';
  static const refreshInterval = '刷新间隔（毫秒）';
  static const fallbackTimezone = '备用时区（IANA）';
  static const systemTimezoneHint = '留空使用系统时区';
  static const timeContextDefaultsHint = '留空使用默认值，保存配置保持当前启停状态。';
  static const saveConfiguration = '保存配置';
  static const cancelChanges = '取消修改';
  static const reloadKeepDraft = '重新读取（保留草稿）';
}

abstract final class DshShortcutZh {
  static const toggleSidebar = '展开／收起侧边栏';
  static const focusComposer = '聚焦消息输入框';
  static const toggleWorkbench = '展开／收起工作台';
  static const focusNext = '聚焦下一区域';
  static const focusPrevious = '聚焦上一区域';
  static const cycleNext = '切换下一会话或工具标签';
  static const cyclePrevious = '切换上一会话或工具标签';
  static const macModifierRequired = '请使用 Cmd、Ctrl 或 Option 组合键，避免影响文字输入。';
  static const modifierRequired = '请使用 Ctrl 或 Alt 组合键，避免影响文字输入。';
  static const reservedKey = '此组合键用于系统或文字编辑，请选择其他组合。';
  static const sendKeyReserved = '此组合键用于消息发送或换行，请选择其他组合。';
  static const duplicateKey = '此快捷键已绑定其他操作。';
  static const title = '快捷键';
  static const search = '搜索操作或组合键';
  static const captureHint =
      '点击组合键后按下新组合；Esc 取消绑定。F6 切换区域，Ctrl+Tab 按焦点切换会话或工作台标签。终端和输入法组词优先。';
  static const noResults = '未找到匹配的快捷键';
  static const capturing = '按下组合键…';
  static const restoreDefaults = '恢复默认';
  static String conflict({required String first, required String second}) =>
      '$first与$second使用了同一组合键，请重新绑定。';
  static String saveFailure({required String detail}) =>
      '${DshZh.saveFailed}；原快捷键仍然有效。$detail';
  static String sendHint({required String modifier}) =>
      'Enter 发送 · Shift+Enter 换行\n忙碌时 $modifier+Enter 使用另一发送行为；空闲时可续写编号列表。';
  static String paletteHint({required int count}) =>
      '${DshZh.results(count)} · ↑↓ 选择 · Enter 打开 · Esc 返回';
}
