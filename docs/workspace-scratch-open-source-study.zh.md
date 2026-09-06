# Agent 工作区隔离：开源实现学习指南

资料日期：2026-09-06。结论基于官方文档和公开源码阅读；不包含本地部署、运行测试或完整安全审计。

## 1. 选择结论

Deep Agents 与 Backlot 最接近“项目内容与 Agent 内部材料分开存放”的需求。Bazel 提供明确输入、独立执行目录和指定输出收集的工程范式；Vibe Kanban 提供可直接阅读的 Rust 工作区管理实现；OpenSandbox 和 Aider 分别补充执行生命周期与上下文过滤。

在下列已查阅实现中，没有确认某一个项目完整覆盖“选择目录预设、临时文件外置、默认检索隔离、成果交付、保留恢复、Windows 执行约束”全部要求。各项目适合作为不同层的设计依据。

| 阅读顺序 | 项目 | 学习重点 | 对应边界 |
|---|---|---|---|
| 1 | [Deep Agents](https://github.com/langchain-ai/deepagents) | 以文件系统后端区分项目文件和内部材料 | 路由文件操作不等于限制 Shell |
| 2 | [Backlot](https://github.com/massivemoose/backlot) | 将脚本、草稿和 Agent 状态集中存入项目外部 | 项目中保留目录链接，主要解决私有归档 |
| 3 | [Bazel](https://bazel.build/docs/sandboxing) | 独立执行区，只导出声明的输出 | 构建系统范式，需要适配开放式 Agent 任务 |
| 4 | [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) | Rust 的 worktree 分配、恢复和清理 | Git 工作副本隔离不等于文件用途分类 |
| 5 | [OpenSandbox](https://github.com/opensandbox-group/OpenSandbox) | 执行环境的创建、续期、暂停和终止 | 环境终止与成果保留需要上层协调 |
| 6 | [Aider](https://aider.chat/docs/faq.html#can-i-use-aider-in-a-large-mono-repo) | 专用忽略规则和限量代码上下文 | 控制模型可见内容，不负责临时文件归位 |

## 2. Deep Agents：最接近存储分流的实现

官方后端文档明确讨论了内部大输出、对话历史与项目文件混在一起的问题，并展示以 `StateBackend` 作为默认后端、将 `/workspace/` 路由到 `FilesystemBackend` 的配置；跨会话材料可以使用单独的持久后端。[后端设计](https://docs.langchain.com/oss/python/deepagents/backends)

源码入口是 [composite.py](https://github.com/langchain-ai/deepagents/blob/main/libs/deepagents/deepagents/backends/composite.py)。先阅读路径路由、`grep` 和 `execute`：指定路由的搜索进入对应后端，根范围搜索可能汇总所有后端；Shell 执行委托给默认执行后端，不按文件路径分流。由此可以确认，存储分流和检索隔离需要独立设计。

建议借鉴后端接口和路径命名空间，将正式项目、当前运行、保留对象分别暴露给工具。项目默认搜索应固定在正式项目范围，不继承“搜索虚拟根就遍历全部挂载”的行为。

阅读时重点区分：`StateBackend` 的材料属于会话状态，可能随 checkpoint 保存；“不是项目里的文件”不代表“退出会话后已经物理删除”。[状态后端源码](https://github.com/langchain-ai/deepagents/blob/main/libs/deepagents/deepagents/backends/state.py)

项目许可证为 [MIT](https://github.com/langchain-ai/deepagents/blob/main/LICENSE)。

## 3. Backlot：最接近外置脚本与笔记的使用体验

Backlot 将项目笔记、脚本、草稿和 Agent 状态集中存入外部私有档案，在项目中建立 `.backlot` 符号链接，并通过 `.git/info/exclude` 避免进入项目历史。README 明确表示目前不支持 Windows。[项目说明与限制](https://github.com/massivemoose/backlot#how-it-works)

两个适合起步的小文件：

- [attach.go](https://github.com/massivemoose/backlot/blob/ead7faf02ca08c0d3b51d6d62d67740f1d15bc02/internal/commands/attach.go)：外部根检查、项目关联、初始化记录模板、建立受管链接与本地忽略项。
- [detach.go](https://github.com/massivemoose/backlot/blob/ead7faf02ca08c0d3b51d6d62d67740f1d15bc02/internal/commands/detach.go)：移除关联和本地忽略项，保留档案内容。

建议借鉴“统一外部根、项目稳定关联、解除关联不等于删除内容”。临时区仍应由宿主直接提供入口，以满足项目目录不保留链接的要求；档案内容也不应全部进入默认检索。

Backlot 将私有材料长期保存的目标，与垃圾槽中可到期回收的环境和日志不同。可借鉴其档案层，不宜将全部临时环境纳入私有 Git 档案。

项目许可证为 [Apache-2.0](https://github.com/massivemoose/backlot/blob/main/LICENSE)。

## 4. Bazel：学习成果交付边界

Bazel 的 `processwrapper-sandbox` 将动作放在单独目录执行，结束后仅搬出已知输出，再清理该沙箱；更强的平台沙箱还限制目录外写入。其官方说明明确区分了执行策略的保证程度。[沙箱机制](https://bazel.build/docs/sandboxing)

源码从 [SandboxHelpers.java 的 moveOutputs](https://github.com/bazelbuild/bazel/blob/master/src/main/java/com/google/devtools/build/lib/sandbox/SandboxHelpers.java) 入手，观察输出清单、文件与目录处理，以及重命名失败后的复制路径。该实现管理的是构建输出区，其中的覆盖策略不能直接照搬到存有人工修改的正式项目目录。

对 Agent 的适配建议是建立候选成果清单：开放式探索可以临时生成未知文件，只有被登记、完成验证并满足目标版本条件的文件才能进入正式目录。无需在任务开始前猜出全部成果，但不能把执行目录整树同步回项目。

项目许可证为 [Apache-2.0](https://github.com/bazelbuild/bazel/blob/master/LICENSE)。

## 5. Vibe Kanban：Rust 工作区管理实现

Vibe Kanban 为 Agent 工作建立工作区和 Git 分支，并提供审阅与合并流程。仓库 README 已显示项目正在收尾（sunsetting）；适合阅读现有实现，采用产品或建立长期依赖前应重新核对维护状态。[项目状态](https://github.com/BloopAI/vibe-kanban)

固定源码版本：`4deb7eca8f381f7cbc1f9d15515a9ab8f8009053`。

- [worktree_manager.rs](https://github.com/BloopAI/vibe-kanban/blob/4deb7eca8f381f7cbc1f9d15515a9ab8f8009053/crates/worktree-manager/src/worktree_manager.rs)：按路径加锁创建、核对文件系统与 Git 元数据、自定义存储根、移动和清理。自定义根下再加应用专属子目录，是值得借鉴的所有权边界。
- [workspace_manager.rs](https://github.com/BloopAI/vibe-kanban/blob/4deb7eca8f381f7cbc1f9d15515a9ab8f8009053/crates/workspace-manager/src/workspace_manager.rs)：组织多个仓库的执行副本、部分创建失败回滚、冷启动恢复、删除上下文与孤立目录处理。

学习重点是资源管理结构和异常路径。已读清理代码包含强制删除，不能直接当作保护未交付草稿的策略；垃圾槽需要额外核实租约、成果引用和恢复状态。仅从分支恢复工作副本也不能证明未提交、未跟踪材料得到了保全。

项目许可证为 [Apache-2.0](https://github.com/BloopAI/vibe-kanban/blob/main/LICENSE)。

## 6. OpenSandbox：环境生命周期与文件传输

OpenSandbox 提供沙箱平台与多语言 SDK，支持 Docker/Kubernetes 执行环境。先阅读 [sandbox-lifecycle.yml](https://github.com/opensandbox-group/OpenSandbox/blob/main/specs/sandbox-lifecycle.yml)：创建、暂停、恢复、终止、绝对过期时间和续期都有明确接口。

然后阅读 [execd-api.yaml](https://github.com/opensandbox-group/OpenSandbox/blob/main/specs/execd-api.yaml)：命令与文件操作独立提供，文件可上传、下载及按范围读取。它提供从环境取回文件的能力，文件是否成为正式成果仍由应用决定。

建议借鉴“运行资源有状态和续期接口”，同时将运行环境期限与文件保留期分开。环境不再运行时，可以先保全候选成果和必要证据，再回收计算资源；临时环境到期不能直接充当成果删除条件。

项目许可证为 [Apache-2.0](https://github.com/opensandbox-group/OpenSandbox/blob/main/LICENSE)。容器机制可作为远程或容器执行适配器，Windows 原生进程与 WPS 工作流仍需独立适配，不能由容器案例推断已受保护。

## 7. Aider：检索范围与上下文预算

Aider 提供专用 `.aiderignore`，可指定忽略文件或限制到仓库子树；其 repo map 将相关符号和关系组织成受预算约束的上下文。[忽略与子树范围](https://aider.chat/docs/faq.html#can-i-use-aider-in-a-large-mono-repo)、[Repository map](https://aider.chat/docs/repomap.html)

源码从 [repo.py](https://github.com/Aider-AI/aider/blob/main/aider/repo.py) 的 `refresh_aider_ignore`、`ignored_file`、`ignored_file_raw` 和候选文件过滤入手，观察规则更新与缓存的关系。

建议把检索范围作为宿主可配置能力，并为搜索、代码图谱和状态摘要使用一致版本的策略。检索过滤既不是写入限制，也不是删除许可；文件未被索引不表示文件没有价值。物理文件变化和上下文压缩之后，读取缓存需要正确失效。

项目许可证为 [Apache-2.0](https://github.com/Aider-AI/aider/blob/main/LICENSE.txt)。

## 8. 设计推论

以下为工作区垃圾槽的设计建议，不能理解为上述任一项目已经完整实现。

| 设计项 | 规则 | 依据 |
|---|---|---|
| 独立能力报告 | 分别报告存储分流、检索过滤、进程写入隔离、成果交付、生命周期管理 | Deep Agents 的文件路由与 Shell 边界不同 |
| 明确搜索范围 | 无范围的搜索默认落到正式项目，不能自动汇总所有存储挂载 | CompositeBackend 的根范围汇总行为 |
| 候选成果清单 | 登记对象、版本、目标和验证结果后交付，未登记输出留在临时区 | Bazel 的已知输出收集 |
| 运行与文件分别计时 | 环境到期触发停机或续期流程，文件回收另查引用和保留期 | OpenSandbox 的生命周期接口 |
| 状态核对 | 对照应用记录、磁盘对象和进程存活；不一致时先进入待恢复状态 | Vibe Kanban 的工作区与文件系统管理 |
| 解除关联单独建模 | 移除工作区关联不自动清空仍有引用的保留对象 | Backlot 的 detach 行为 |

第一轮学习建议依次阅读 Deep Agents 的后端文档、Backlot 的 attach/detach、Bazel 的沙箱说明。第二轮进入 Rust 管理器、生命周期规范和忽略规则源码。

阅读时使用同一组检查问题：一次性脚本从哪里创建；根搜索会命中什么；Shell 能否绕过分流；失败后哪些内容保留；谁有权决定交付；程序崩溃后怎样恢复；同名文件或并发调用怎样处理。这样可以将具体实现转化为可验证的设计约束。
