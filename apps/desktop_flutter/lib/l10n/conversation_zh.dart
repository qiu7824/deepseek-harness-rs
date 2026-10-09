/// Chinese copy for conversation, output and tool workspaces.
abstract final class DshConversationZh {
  static const loadingData = '正在加载列表…';
  static String loadingList({required String name}) => '正在加载$name…';
  static const messageNotFound = '无法定位该消息，请重试。';
  static const attachmentCountLimit = '单条消息最多添加 8 个附件';
  static const attachmentBytesLimit = '附件总大小不能超过 16 MiB';
  static const close = '关闭';
  static const newSession = '新会话';
  static const conversation = '会话';
  static const trajectory = '轨迹';
  static const artifacts = '产物';
  static const projectTasks = '项目任务';
  static const codeGraph = '代码图谱';
  static const context = '上下文';
  static const historyWindowLimit = '部分记录超过当前窗口的展示上限，完整内容仍保存在会话日志中。';
  static const welcomeTitle = '探索未至之境';
  static const rustEdition = 'Rust 版';
  static const loadingHistory = '正在读取…';
  static const loadEarlier = '加载更早记录';
  static const jumpToBottom = '回到底部';
  static const removeQueuedMessage = '移除排队消息';
  static const agentPreset = 'Agent 预设';
  static const chooseWorkspaceHint = '选择一个工作区开始';
  static const planPromptHint = '描述你的任务以生成计划';
  static const buildPromptHint = '描述你想要构建的内容';
  static const messagePromptHint = '给智能体发消息';
  static const commands = '命令';
  static const insertSession = '插入对话';
  static const uploadFiles = '上传文件';
  static const startingSpeech = '正在启动语音识别…';
  static const finishingSpeech = '正在结束语音识别…';
  static const listening = '正在聆听，松开结束';
  static const executionDetails = '执行详情';
  static const copyResult = '复制结果';
  static const copyInput = '复制输入';
  static const copy = '复制';
  static const input = '输入';
  static const result = '结果';
  static const modelAndReasoning = '模型与推理等级';
  static const searchModelsHint = '搜索模型名称、ID 或连接';
  static const manageModels = '管理模型';
  static const openLinkInBrowser = '在浏览器中打开链接';
  static const openFile = '打开文件';
  static const copyLinkAddress = '复制链接地址';
  static const copyFilePath = '复制文件路径';
  static const copyLinkText = '复制链接文字';
  static const revealInFileManager = '在资源管理器中显示';
  static const openWithLocalTool = '使用本地工具打开';
  static const saveOriginalCopy = '保存原文件副本';
  static const toolResult = '工具结果';
  static const stopReadAloud = '停止朗读';
  static const readAloud = '朗读';
  static const viewFullMessage = '查看完整消息';
  static const branchConversation = '在新会话中分支';
  static const thinking = '思考';
  static const previousSection = '上一段';
  static const nextSection = '下一段';
  static const submittingApproval = '正在提交审批…';
  static const awaitingApproval = '等待审批';
  static const deny = '拒绝';
  static const allowOnce = '允许一次';
  static const allowAlways = '始终允许';
  static const added = '新增';
  static const modified = '修改';
  static const delivered = '已交付';
  static const removed = '已移除';
  static const running = '运行中';
  static const retained = '保留中';
  static const awaitingDelivery = '待交付';
  static const runInterrupted = '运行中断';
  static const failureArtifacts = '失败材料';
  static const recoveryQueue = '恢复队列';
  static const reclaimed = '已回收';
  static const renameArtifact = '重命名产物';
  static const moveToTrash = '移入垃圾槽';
  static const move = '移入';
  static const refresh = '刷新';
  static const artifactScanLimit = '工作区较大，扫描已达到文件数量上限；工具记录的变更仍会显示。';
  static const noArtifactChanges = '此任务暂未记录文件变更';
  static const preview = '预览';
  static const copyPath = '复制路径';
  static const rename = '重命名';
  static const openInEditor = '在编辑器中打开';
  static const viewGeneratedTrash = '查看产生的垃圾列表';
  static const closeArtifactPreview = '关闭产物预览';
  static const trash = '垃圾槽';
  static const cleanupReclaimable = '清理可回收项';
  static const cleanupReclaimableHint = '移除已到期且未固定、未使用的受管临时材料。';
  static const cleanup = '清理';
  static const backToArtifacts = '返回产物';
  static const noManagedMaterials = '暂无受管临时材料';
  static const measuringStorage = '占用统计中';
  static const unpin = '取消固定';
  static const pin = '固定保留';
  static const markCompleted = '标记已完成';
  static const restoreOriginalFile = '恢复原文件';
  static const restoreMaterial = '恢复临时材料';
  static const queueRecovery = '移入恢复队列';
  static const managedFiles = '受管文件';
  static const parentLevel = '上一级';
  static const directoryDisplayLimit = '目录较大，显示前 500 项';
  static const backToDirectory = '返回目录';
  static const copyFilename = '复制文件名';
  static const imageDisplayLimit = '图片超过显示上限';
  static const image = '图片';
  static const saveImage = '保存图片';
  static const viewOriginalImage = '查看原图';
  static const copyImageName = '复制图片名称';
  static const indexPending = '等待索引';
  static const indexing = '正在索引';
  static const checkingChanges = '检查变更';
  static const indexReady = '索引就绪';
  static const partialIndex = '部分索引';
  static const indexPaused = '索引已暂停';
  static const indexFailed = '索引失败';
  static const nodeDetails = '节点详情';
  static const relationshipDetails = '关系详情';
  static const closeNodeDetails = '关闭节点详情';
  static const fileImport = '文件导入';
  static const staticCodeRelation = '静态代码关联';
  static const viewSymbols = '查看符号';
  static const callers = '调用者';
  static const callees = '被调用者';
  static const impactScope = '影响范围';
  static const sourceSnippet = '源码片段';
  static const loadingSource = '正在读取源码…';
  static const noResolvedRelationships = '当前范围内没有已解析的关联。';
  static const codeCanvas = '代码画布';
  static const codeCanvasHint = '从结构到细节，沿着关系探索代码';
  static const indexedFiles = '索引文件';
  static const codeSymbols = '代码符号';
  static const staticRelationships = '静态关系';
  static const searchGraphFilesHint = '搜索文件或路径…';
  static const searchGraphSymbolsHint = '搜索函数、类型或路径…';
  static const fileDependencies = '文件依赖';
  static const symbolCalls = '符号调用';
  static const inferredRelationships = '推断关系';
  static const update = '更新';
  static const pause = '暂停';
  static const workspaceSubset = '工作区局部视图';
  static const focusedView = '聚焦视图';
  static const backToOverview = '返回概览';
  static const selectGraphEntry = '选择入口以聚焦关系';
  static const searchResults = '搜索结果';
  static const indexingProgress = '正在索引…';
  static const noGraphMatches = '没有匹配的文件或符号';
  static const buildingCodeMap = '正在构建代码地图';
  static const noCodeRelationships = '当前工作区暂无代码关系';
  static const noMatches = '没有找到匹配项';
  static const indexingBackgroundHint = '索引在后台运行，可以继续使用会话。';
  static const graphStartHint = '搜索文件或函数名称，找到探索的起点。';
  static const zoomOutCanvas = '缩小画布';
  static const zoomInCanvas = '放大画布';
  static const fitCanvas = '适应画布';
  static const relayoutCanvas = '重新布局';
  static const staticRelationshipLegend = '实线：静态关联';
  static const inferredRelationshipLegend = '虚线：名称推断';
  static const partialViewSuffix = ' · 局部展示';
  static const file = '文件';
  static const function = '函数';
  static const structure = '结构体';
  static const noUsage = '尚无用量';
  static const autoCompactionThreshold = '自动压缩阈值';
  static const useDefaultThreshold = '使用默认阈值';
  static const customThreshold = '当前模型使用自定义阈值';
  static const restoreDefaults = '恢复默认';
  static const compactionStarted = '已开始压缩';
  static const compacting = '压缩中…';
  static const compactNow = '立即压缩';
  static const clearFeedback = '取消标记';
  static const goodAnswer = '好的回答';
  static const problematicAnswer = '有问题的回答';
  static const addFeedbackNote = '补充说明';
  static const confirmPositiveFeedback = '确认正面评价';
  static const confirmNegativeFeedback = '确认负面评价';
  static const feedbackHint = '记录对这条回复的评价，分类和说明可选。';
  static const optionalFeedbackCategory = '选择分类（可选）';
  static const feedbackNote = '反馈说明';
  static const feedbackNoteHint = '这条回答哪里好，或哪里有问题？（可选）';
  static const cancel = '取消';
  static const savingFeedback = '正在保存…';
  static const confirmFeedback = '确认评价';
  static const feedbackTaskResult = '任务结果';
  static const feedbackInstructions = '指令遵循';
  static const feedbackInteraction = '交互体验';
  static const feedbackReliability = '服务稳定性';
  static const feedbackResources = '资源与费用';
  static const feedbackSecurity = '安全、隐私与权限';
  static const feedbackOther = '其他';
  static const feedbackConflict = '这条反馈已在别处改动，已显示最新状态；请核对后再保存。';
  static const feedbackMessageUnavailable = '此消息无法评价，请刷新会话记录。';
  static const sessionUnavailable = '会话已不存在或尚未加载。';
  static const feedbackNoteTooLong = '反馈说明过长，请缩短后重试。';
  static const feedbackSaveFailed = '反馈保存失败，请重试。';
  static const feedbackConfirmationInvalid = '服务未返回有效的反馈确认。';
  static const feedbackListInvalid = '反馈列表格式无效。';
  static const feedbackHistoryTooLarge = '反馈内容过多，请缩小历史范围。';
  static const feedbackResponseInvalid = '服务未返回有效的消息评价。';
  static const feedbackRevisionMissing = '缺少评价版本。';
  static const feedbackRemovalUnconfirmed = '服务未确认撤销评价。';
  static const imageOnly = '（仅图片）';
  static const accessModeRejected = '访问模式未被接受';
  static const workspaceAccessHint = '工作区内可写，更大范围的操作需要审批';
  static const fullAccessHint = '完整文件访问，无需审批提示';
  static const changingAccessMode = '正在切换访问模式…';
  static const planRequestedHint = '计划模式将在下一步开启；点击取消';
  static const planEnabledHint = '计划模式已开启；点击关闭（/plan off）';
  static const planEnabledAction = '计划模式已开启，按下关闭';
  static const closePlanMode = '关闭计划模式';
  static const discussInConversation = '去聊天里说';
  static const confirmExecution = '确认执行';
  static const planAwaitingReview = '计划待审';
  static const questionsRemaining = '请回答或跳过剩余问题';
  static const submittingAnswers = '正在提交';
  static const abandoningAnswers = '正在放弃';
  static const awaitingAnswers = '等待回答';
  static const answerHint = '输入你的答案';
  static const previousQuestion = '上一题';
  static const nextQuestion = '下一题';
  static const skipQuestion = '跳过本题';
  static const submit = '提交';
  static const reasoningNone = '不推理';
  static const reasoningMinimal = '极低';
  static const reasoningLow = '低';
  static const reasoningMedium = '中';
  static const reasoningHigh = '高';
  static const reasoningExtraHigh = '极高';
  static const reasoningMaximum = '最高';
  static const reasoningStrength = '推理强度';
  static const modelRetried = '已重试模型请求';
  static const modelRetryCancelled = '已取消模型重试';
  static const modelRetrying = '正在重试模型请求';
  static const downloadSessionLog = '下载会话日志';
  static const sessionExportFailed = '会话导出失败';
  static const sessionExportComplete = '会话导出完成';
  static const exportingSession = '正在导出会话';
  static const sessionExportedHint = '会话、子会话和附件已保存到所选 ZIP 文件。';
  static const requestRateUnavailable = '请求速率未提供';
  static const cacheUsageUnavailable = '缓存统计未提供';
  static const partialRequestsSuffix = '（部分请求）';
  static const contextUsageUnavailable = '上下文占用尚未提供';
  static const systemPrompt = '系统提示词';
  static const toolDefinitions = '工具定义';
  static const conversationContent = '对话内容';
  static const contextEstimateHint = '组成按启发式估算，与供应商计费用量可能不同。';
  static const statusDisplayLimit = '部分状态数据超过显示预算，未载入。';
  static const editTask = '编辑任务';
  static const removeTask = '移除任务';
  static const goalSessionChanged = '会话已切换，请重新打开目标。';
  static const clearGoal = '清除目标';
  static const clearGoalHint = '清除当前目标，保留会话历史。';
  static const clear = '清除';
  static const goal = '目标';
  static const saveGoal = '保存目标';
  static const cancelGoalEdit = '取消编辑目标';
  static const activeGoal = '进行中的目标';
  static const pausedGoal = '已暂停的目标';
  static const blockedGoal = '受阻的目标';
  static const pauseGoal = '暂停目标';
  static const resumeGoal = '恢复目标';
  static const editGoal = '编辑目标';
  static const editTaskRequiresHostUpdate = '编辑任务需要更新 Host';
  static const removeTaskRequiresHostUpdate = '移除任务需要更新 Host';
  static const task = '任务';
  static const stopCurrentExecution = '停止当前执行';
  static const saving = '保存中…';
  static const save = '保存';
  static const unavailable = '未提供';
  static const session = '会话';
  static const messageCount = '消息数';
  static const provider = '提供商';
  static const totalTokens = '总 token';
  static const usageRatio = '使用率';
  static const inputTokens = '输入 token';
  static const outputTokens = '输出 token';
  static const reasoningTokens = '推理 token';
  static const cacheReadWriteTokens = '缓存 token（读/写）';
  static const assistantMessages = '助手消息';
  static const totalCost = '总成本';
  static const lastActivity = '最后活动';
  static const contextBreakdown = '上下文细分';
  static const noContextStats = '尚无上下文统计';
  static const contextStatsHint =
      '上下文细分为当前模型可见内容的估算；总 token 为累计计费量，推理 token 包含在输出量中。未报告的用量与价格显示为“未提供”。';
  static const previousPage = '上一页';
  static const nextPage = '下一页';
  static const copyEvent = '复制事件';
  static const plan = '计划';
  static const previewPlan = '预览计划';
  static const planApprovalHint = '批准与拒绝在原计划审批卡中处理。';
  static const stop = '停止';
  static const interrupted = '已中断';
  static const stopped = '已停止';
  static const output = '输出';
  static const runningProgress = '运行中…';
  static const noOutput = '暂无输出';
  static const callDetails = '调用详情';
  static const noTrajectoryMatches = '没有匹配的轨迹';
  static const turns = '轮次';
  static const calls = '调用';
  static const duration = '耗时';
  static const equalWidthOperations = '按等宽操作显示';
  static const actualDurationOperations = '按实际耗时显示';
  static const expandTurns = '展开轮次';
  static const collapseTurns = '收起轮次';
  static const expandCalls = '展开调用';
  static const collapseCalls = '收起调用';
  static const searchTrajectory = '搜索轨迹';
  static const loadingEarlierHistory = '正在加载更早记录…';
  static const backToLatest = '返回最新';
  static const model = '模型';
  static const tool = '工具';
  static const timelineOverviewHint = '时间线概览；横向拖动聚焦事件';
  static const closeEventDetails = '关闭事件详情';
  static const status = '状态';
  static const completed = '已完成';
  static const failed = '失败';
  static const turnAndStep = '轮次／步骤';
  static const startTime = '开始时间';
  static const endTime = '结束时间';
  static const totalDuration = '总耗时';
  static const firstTokenDuration = '首 Token 耗时';
  static const providerAndModel = '提供方／模型';
  static const restoringConnection = '连接中断，正在恢复状态';
  static const awaitingUserAnswer = '等待你的回答';
  static const sending = '正在发送';
  static const compactingContext = '正在压缩上下文';
  static const executingCommand = '正在执行命令';
  static const thinkingProgress = '正在思考';
  static const generatingAnswer = '正在生成回复';
  static const generatingToolArguments = '正在生成工具参数';
  static const preparingModelAuthentication = '正在准备模型认证';
  static const awaitingModelResponse = '正在等待模型响应';
  static const receivingModelResponse = '正在接收模型响应';
  static const preparingAttachments = '正在准备附件';
  static const uploadingAttachments = '正在上传附件';
  static const continuingWork = '正在继续处理';
  static const readingHistory = '正在查看历史消息';
  static const turnDuration = '本轮总用时';
  static const averageOutputRateHint = '请求平均输出速率（发送至结束）';
  static const timeToFirstToken = '首 token 用时（TTFT）';
  static const turnTiming = '本轮用时和速度';
  static const cacheRead = '缓存读取';
  static const cacheWrite = '缓存写入';
  static const turnUsage = '本轮用量';
  static const closeImagePreview = '关闭图片预览';
  static const code = '代码';
  static const diagram = '图表';
  static const source = '源码';
  static const copyCode = '复制代码';
  static const localDesktop = '本机桌面';
  static const boundRemoteDevice = '已绑定的 UU 远程设备';
  static const isolatedBrowser = '隔离浏览器';
  static const takeoverLocalInput = '检测到本机键鼠输入（包括其他窗口）';
  static const takeoverInjectedInput = '检测到其他程序注入键鼠输入';
  static const takeoverEmergencyShortcut = '已触发全局急停快捷键';
  static const takeoverControlInput = '控制画面收到人工输入';
  static const takeoverPanel = '已在控制面板选择人工接管';
  static const takeoverInitialMode = '连接以人工控制模式启动';
  static const takeoverNotReleased = '控制权尚未交还';
  static const takeoverInputReleasePending = '键鼠释放未完成';
  static const takeoverShortcutUnavailable = '全局急停快捷键不可用';
  static const controlProcessDisconnected = '控制进程已断开';
  static const closingControlConnection = '控制连接正在关闭';
  static const takeoverReasonUnknown = '暂停来源尚未确认';
  static const controlConnectionChanged = '控制连接已切换';
  static const computerUseDisabled =
      'Computer Use 未启用：在“设置 → 目录与运行环境”中开启后重启本机服务。';
  static const computerUseAdapterUnavailable =
      'Computer Use 执行器当前不可用，请检查浏览器或外部命令设置';
  static const controlInputLimit = '单次输入最多 8192 个字符';
  static const primaryMonitor = '主显示器';
  static const currentSuffix = ' · 当前';
  static const disconnected = '连接已断开';
  static const awaitingFrame = '等待画面';
  static const notConnected = '未连接';
  static const humanControlStatus = '人工接管中 · 智能体控制暂停';
  static const agentControlReady = '智能体可操作';
  static const controlConnectionClosedHint = '连接已关闭。点击“连接”重新打开。';
  static const frameUnavailable = '暂时无法显示画面';
  static const connectingAndFetchingFrame = '正在连接并获取画面…';
  static const controlFrameHint = '控制画面，操作即接管；画面聚焦时 Esc 暂停智能体';
  static const connect = '连接';
  static const connected = '已连接';
  static const connecting = '连接中';
  static const adapter = '适配器';
  static const unselected = '未选择';
  static const controlOwnership = '控制权';
  static const humanTakeover = '人工接管';
  static const agent = '智能体';
  static const notEstablished = '未建立';
  static const frame = '画面';
  static const notReceived = '未收到';
  static const connectionPhase = '连接阶段';
  static const paused = '已暂停';
  static const controllable = '可操作';
  static const notControllable = '不可操作';
  static const windowFocus = '窗口焦点';
  static const foreground = '前台';
  static const background = '后台';
  static const notApplicable = '不适用';
  static const pauseReason = '暂停原因';
  static const controlGeneration = '控制代次';
  static const copyDiagnostics = '复制诊断';
  static const refreshBrowserSessions = '刷新浏览器会话';
  static const reconnect = '重新连接';
  static const returnControlToAgent = '交还智能体';
  static const selectWindow = '选择窗口';
  static const activateWindow = '激活窗口';
  static const autoRefresh = '自动刷新';
  static const refreshFrame = '刷新画面';
  static const closeControlSession = '关闭控制会话';
  static const controlledBrowserAddress = '受控浏览器地址';
  static const navigate = '转到';
  static const unconfirmed = '尚未确认';
  static const typeIntoFocusedTarget = '输入到当前焦点';
  static const privateInput = '私密输入';
  static const scrollUp = '向上';
  static const scrollDown = '向下';
  static const planUnavailable = '计划内容不可用';
  static const planPreviewLimit = '计划超过预览上限，请在原计划卡中查看。';
  static const planPreviewExpired = '此计划预览已失效，请从原计划卡重新打开。';
  static const backToSource = '返回来源';
  static const copied = '已复制';
  static const planSnapshotHint = '计划内容快照；审批状态以原计划卡为准。';
  static const todo = '待办';
  static const inProgress = '进行中';
  static const needsUpdate = '需更新';
  static const feedback = '反馈';
  static const refreshProjectTasks = '刷新项目任务';
  static const addTask = '新增任务';
  static const projectTaskSharingHint =
      '工作区共用一份 Markdown 清单，其他对话和 AI 的修改会在刷新后显示。';
  static const searchTasks = '搜索任务';
  static const all = '全部';
  static const noProjectTasks = '还没有项目任务';
  static const projectTaskFieldsInvalid = '请填写任务标题（最多 300 字）；说明最多 20000 字。';
  static const taskTitle = '任务标题';
  static const priorityUrgent = 'P0 · 紧急';
  static const priorityHigh = 'P1 · 高';
  static const priorityMedium = 'P2 · 中';
  static const priorityLow = 'P3 · 低';
  static const descriptionAndFeedback = '说明与反馈';
  static const refreshSubagents = '刷新子任务';
  static const noSubagents = '当前会话没有子任务';
  static const corruptRecord = '记录损坏';
  static const unsupportedRecordFormat = '暂不支持的记录格式';
  static const recordUnavailable = '暂时无法读取';
  static const conversationAllowed = '可继续对话';
  static const oneShotTask = '一次性任务';
  static const backToSubagents = '返回子任务';
  static const you = '你';
  static const assistant = '助手';
  static const executionRecords = '执行记录';
  static const childMessageHint = '继续向子任务发送消息…';
  static const queueMessage = '加入队列';
  static const steerExecution = '补充当前执行';
  static const interrupt = '中断';
  static const send = '发送';
  static const terminal = '终端';
  static const backgroundJobs = '后台任务';
  static const subagents = '子任务';
  static const planPreview = '计划预览';
  static const workbenchConnectionHint = '连接服务并选择会话后打开工具。';
  static const openToolTab = '打开工具标签';
  static const closeWorkbench = '关闭工作台';
  static const emptyWorkbenchHint = '使用 + 打开文件、终端或其他工具标签。';
  static const selectFileHint = '选择文件以查看内容';
  static const parentDirectory = '上级目录';
  static const workspace = '工作区';
  static const refreshDirectory = '刷新目录';
  static const pdfNotGenerated = 'PDF 预览尚未生成';
  static const wrapLines = '自动换行';
  static const copyFileContent = '复制文件内容';
  static const exportPdf = '导出 PDF';
  static const findContent = '查找内容';
  static const findNext = '查找下一个';
  static const terminalOutputInvalid = '终端输出格式无效';
  static const terminalInputBacklog = '终端输入积压过多，请等待发送完成后重试';
  static const terminalCountLimit = '最多同时保留 3 个终端，请先关闭不需要的终端';
  static const newTerminal = '新建终端';
  static const closeCurrentTerminal = '关闭当前终端';
  static const noBackgroundJobs = '当前没有后台任务';

  static const chooseWorkspace = '选择工作区';
  static const chooseModel = '选择模型';
  static const approvalOnceOnly = '此请求只支持单次授权';
  static const approvalDirectoryScope = '记忆只覆盖同一目录的写入；其他目录和子目录仍需审批。';
  static const approvalCommandScope = '记忆只覆盖这条命令；其他命令仍需审批。';
  static const approvalMatchingScope = '仅记住与本次请求匹配的授权范围';
  static const inUse = '正在使用';
  static const viewFile = '查看文件';
  static const noSourceAtLocation = '该位置暂无源码';
  static const nameInference = '名称推断';
  static const openSource = '打开源码';
  static const focusRelationships = '聚焦关系';
  static const graphKeyboardHint = '代码关系画布：拖动平移，滚轮缩放，方向键平移，0 适应画布';
  static const feedbackNotSaved = '反馈未保存，请重新读取状态后重试。';
  static const accessRefreshFailedPrefix = '模式已提交，状态刷新失败：';
  static const confirmFullAccess = '确认启用 Full access？';
  static const fullAccessRiskHint =
      '启用 Full access 后，agent 将减少确认步骤，并且可以直接执行更多操作，包括敏感操作、文件修改或外部命令。仅建议在你信任当前任务时使用。';
  static const acknowledgeFullAccess = '我已了解风险，并愿意继续';
  static const enableFullAccess = '启用 Full access';
  static const abandonQuestions = '放弃整组问题';
  static const recommended = '推荐';
  static const chooseExportLocation = '请选择 ZIP 文件的保存位置。';
  static const estimatedCapacitySuffix = ' · 容量估算';
  static const taskSessionChanged = '会话已切换，请保留草稿后重新打开任务。';
  static const remove = '移除';
  static const contextLimit = '上下文限制';
  static const userMessages = '用户消息';
  static const createdAt = '创建时间';
  static const user = '用户';
  static const toolCalls = '工具调用';
  static const outputTokenLabel = '输出 Token';
  static const requestOptionsAndPrompt = '请求选项与系统提示词';
  static const fullResult = '完整结果';
  static const providerModelLabel = '提供方 / 模型';
  static const cacheHit = '缓存命中';
  static const uncachedInput = '未缓存输入';
  static const noAvailableWindows = '没有可用窗口';
  static const interactionState = '交互状态';
  static const emergencyShortcut = '急停快捷键';
  static const capabilities = '能力';
  static const undeclared = '未声明';
  static const error = '错误';
  static const normal = '正常';
  static const waiting = '等待';
  static const back = '后退';
  static const localDesktopTakeoverHint =
      '本机桌面 · 与本机共用键鼠；操作其他窗口也会暂停智能体，避免争抢鼠标或将内容输入错误窗口。';
  static const openSettings = '打开设置';
  static const edit = '编辑';
  static const refreshChildHistory = '刷新子任务历史';
  static const earlierHistory = '更早记录';
  static const noChildHistory = '暂无子任务记录';
  static const viewNestedSubagents = '查看下级子任务';
  static const refreshChanges = '刷新修改';
  static const selectChangeHint = '选择修改文件查看差异';
  static const taskDetails = '任务详情';

  static String truncatedMessage({required Object? prefix}) =>
      '$prefix\n\n…正文过长，已截取显示；打开消息详情查看完整内容。';
  static String latestHistoryFailed({required Object? error}) =>
      '无法返回最新：$error';
  static String folderAttachmentRejected({required Object? name}) =>
      '不能添加文件夹：$name';
  static String unreadActivity({required Object? count}) =>
      '有 $count 条新动态，回到底部';
  static String fileOperationFailed({required Object? error}) =>
      '文件操作失败：$error';
  static String monthDayTime({
    required Object? month,
    required Object? day,
    required Object? time,
  }) => '$month月$day日 $time';
  static String elapsedTime({required Object? duration}) => '用时 $duration';
  static String firstTokenTime({required Object? seconds}) =>
      '首 token $seconds秒';
  static String speechUnavailable({required Object? error}) => '语音播放不可用：$error';
  static String approvalReason({
    required Object? reason,
    required Object? tool,
  }) => '${reason ?? '$tool 请求执行权限'}';
  static String fileCount({required Object? count}) => '$count 个文件';
  static String managePath({required Object? path}) => '管理 $path';
  static String itemStorageSummary({
    required Object? count,
    required Object? bytes,
  }) => '$count 项 · $bytes';
  static String openNamedFile({required Object? name}) => '打开文件：$name';
  static String removeNamedAttachment({required Object? name}) => '移除附件：$name';
  static String previewNamedImage({required Object? name}) => '预览图片：$name';
  static String graphRelationshipSummary({
    required Object? kind,
    required Object? count,
  }) => '$kind · $count 处记录（显示首处）';
  static String neighboringNodes({required Object? count}) =>
      '画布中的相邻节点 · $count';
  static String graphTotals({required Object? nodes, required Object? edges}) =>
      '$nodes 节点 · $edges 关联';
  static String relationCount({required Object? count}) => '$count 关联';
  static String tenThousands({required Object? amount}) => '$amount万';
  static String feedbackReloadHint({required Object? error}) =>
      '$error\n点击重新读取反馈';
  static String railMessageWithImages({
    required Object? text,
    required int imageCount,
  }) => '$text${imageCount > 0 ? '\n含图片 $imageCount 张' : ''}';
  static String railRange({
    required Object? count,
    required Object? first,
    required Object? last,
  }) => '你说过的话：$count 条，当前 $first-$last';
  static String jumpToUserMessage({required Object? snippet}) =>
      '跳到你说的话：$snippet';
  static String accessModeLabel({
    required Object? mode,
    required Object? details,
  }) => '访问模式，当前：$mode$details';
  static String retryDetails({
    required Object? delay,
    required Object? code,
    required Object? message,
  }) => '重试延迟：${delay}ms\n$code\n$message';
  static String exportProgress({required int bytes}) =>
      '正在保存会话、子会话和附件…${bytes > 0 ? ' 已写入 ${(bytes / 1048576).toStringAsFixed(1)} MiB' : ''}';
  static String turnsAndSteps({
    required Object? turns,
    required Object? steps,
  }) => '$turns 轮 · $steps 步';
  static String toolCallTime({required Object? duration}) => '工具调用 $duration';
  static String averageFirstTokenTime({required Object? duration}) =>
      '首 token 平均 $duration';
  static String averageRequestRate({required Object? rate}) =>
      '请求平均 $rate tok/s';
  static String cacheHitSummary({
    required Object? hit,
    required Object? coverage,
  }) => '缓存命中 $hit%$coverage';
  static String inputOutputTokens({
    required Object? input,
    required Object? output,
  }) => '输入 $input tok · 输出 $output tok';
  static String contextPercent({required Object? percent}) => '上下文已用 $percent%';
  static String goalProgress({
    required Object? objective,
    required Object? started,
    required Object? maximum,
    required Object? blockedDetail,
  }) => '$objective\n$started / $maximum 轮$blockedDetail';
  static String completedCount({required Object? count}) => '$count 已完成';
  static String activeCount({required Object? count}) => '$count 进行中';
  static String pendingCount({required Object? count}) => '$count 待处理';
  static String missingContextWithBudget({required Object? budget}) =>
      '未提供（运行预算 $budget）';
  static String turnLabel({required Object? number}) => '第 $number 轮';
  static String collapsedSummary({
    required Object? kind,
    required Object? count,
  }) => '$kind已收起 · $count 项';
  static String eventDetailsForRole({required Object? role}) => '事件详情 · $role';
  static String rawEventCount({required Object? count}) => '原始事件 · $count';
  static String executingTool({required Object? title}) => '正在执行工具 · $title';
  static String secondsDuration({required Object? seconds}) => '$seconds秒';
  static String minutesSecondsDuration({
    required Object? minutes,
    required Object? seconds,
  }) => '$minutes分$seconds秒';
  static String outputWithReasoning({
    required String output,
    required String? reasoning,
  }) => '$output${reasoning == null ? '' : '（其中推理 $reasoning）'}';
  static String tokenUsage({required Object? amount}) => '用量 $amount';
  static String imageUnavailable({required Object? label}) => '$label（图片无法显示）';
  static String diagramParseFailed({required Object? error}) => '图表解析失败：$error';
  static String runtimeDiagnostics({required Object? state}) => '运行诊断 · $state';
  static String browserSession({required Object? name}) => '浏览器会话 $name';
  static String windowSelected({required Object? title}) =>
      '已选择：$title · 重新连接后切换';
  static String remoteEmergencyHint({required Object? shortcut}) =>
      'UU 远程 · 全局急停 $shortcut；画面聚焦时 Esc 暂停智能体。';
  static String controlPaused({required Object? reason}) => '智能体控制暂停 · $reason';
  static String namedSession({required Object? id}) => '会话 $id';
  static String controlTargetSummary({
    required Object? adapter,
    required Object? target,
  }) => '$adapter · $target';
  static String closeNamedPlan({required Object? title}) => '关闭计划 $title';
  static String subagentCount({required Object? count}) => '子任务 · $count';
  static String terminalInputFailed({required Object? error}) =>
      '终端输入发送失败：$error';
  static String closeToolTab({required Object? title}) => '关闭$title标签';
  static String closeNamedFile({required Object? name}) => '关闭 $name';
  static String terminalName({required Object? index}) => '终端 $index';
}
