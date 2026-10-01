# Flutter 桌面性能与资源基线

测量日期：2026-09-29。平台：Windows x64。资源循环使用 Release 构建，持续流式使用 Profile 构建；两个场景分别统计。

固定资源负载下，候选客户端的 **Private Bytes 观测峰值降低 30.66%**，但 **Working Set 观测峰值增加 13.05%**。两版均满足第 5 轮至第 20 轮的内存端点增长判据。持续流式场景中，候选 UI／Raster P95 分别为 **5.067／2.230 ms**，均满足 16.667 ms 阈值，但相对原版分别增加 **12.52%／28.60%**；这组结果不构成帧耗时下降的证据。

每个版本、每个场景均为一次独立进程运行。资源场景中的 20 轮重复不等于三次独立启动的重复测量。结论限定于下述夹具、环境和构建身份。

## 环境与构建范围

| 项目 | 记录 |
| --- | --- |
| CPU | Intel Core i7-10700KF，8 核／16 逻辑处理器 |
| 系统可见物理内存 | 34,294,738,944 B，约 31.94 GiB |
| GPU | NVIDIA GeForce RTX 3060；驱动 `32.0.15.9649` |
| 显示环境 | 环境清单记录 2560×1440、164 Hz；另有 GameViewer Virtual Display Adapter |
| 系统 | Windows 11 家庭版，`10.0.26200`；Dart 的系统描述字符串为 `Windows 10 Home`、Build 26200 |
| Flutter | `3.47.5`，revision `6a19cca56475dbfba1478ee68d7bd0c2ef891da1` |
| Dart | `3.13.4`，Windows x64 |
| 测试布局 | 1280×800 逻辑像素；各运行记录的原生视图为 1424×861 物理像素，DPR 1.0 |
| 原版源码 | `097bde64077a3e30e5c54edc55c1d543848e5f98` |
| 候选性能快照 | 客户端源码树 SHA-256：`ab4c7106350315e28a0df2adc5c9d64f3373cd0fb00a7998dff69f3abd50f34b` |

硬件清单由本机 CIM 读取；视图尺寸、DPR、运行时版本来自各次报告。文字缩放因子未独立采样。测试原生窗口使用独立的 QA 名称与单实例标识，避免与已安装客户端共用实例。

这些数值对应测试入口构建，包含集成测试绑定及进程内夹具。性能快照与最终安装构建采用独立身份，安装包以发行清单中的源码树和文件摘要为准；本报告不把测试可执行文件视为安装版启动测量。

## Release 资源循环

入口：[experience_resource_soak_test.dart](../../apps/desktop_flutter/integration_test/experience_resource_soak_test.dart)。两版入口 SHA-256 均为 `9cc2cfde0eb9d348148f089b6231c96cd42feaa68ea1e25087c66ec4d2827116`。

| 负载 | 执行方式 |
| --- | --- |
| 图片 | 固定 3840×2160 彩色图案，PNG 为 39,841 B；按不同文件名、独立字节副本连续打开。第 0 轮 10 次，第 1—20 轮每轮 20 次；每次仅显示当前预览 |
| PDF | 有效的文本与矢量 PDF；50 页文件 16,166 B，200 页文件 64,418 B。第 0 轮用 50 页，其余用 200 页；访问首、中、末页，不遍历渲染全部页面 |
| 终端 | 每轮回放 3000 行文本，切至 Git 标签后检查停止读取；释放视图不得提交关闭终端动作 |
| 会话作用域 | 工作台在 21 个不同会话作用域中重建，不包含真实聊天历史加载 |
| 关闭与静置 | 每轮关闭整个工作台；第 1—20 轮关闭后静置 60 秒，第 5 轮为稳态比较起点 |
| 共享图片缓存 | 两版均设置 48 MiB、128 项；1 MiB = 1,048,576 B |

客户端仍经 Host 客户端接口取得夹具字节，但接口由进程内模拟实现，没有连接真实 Rust Host、创建真实 PTY 或执行 Office 转换。PNG 和 PDF 均为固定、可重复的简单素材，不能代表大体积扫描 PDF、复杂字体或所有图像内容。

### 采样方法

外部采样器对指定 PID 调用 `Process.Refresh()`，记录 `PrivateMemorySize64`、`WorkingSet64`、句柄数和累计 CPU 时间，随后等待 1 秒。原版取得 1280 条样本，间隔中位数 1.014 秒；候选取得 1275 条样本，间隔中位数 1.014 秒。两组最大采样间隔分别为 1.042／1.045 秒。表中的“观测峰值”是这些离散样本的最大值，会漏掉短暂峰值。

Private Bytes 表示进程不可与其他进程共享的已分配内存；Working Set 表示当前驻留物理内存的页面，两者衡量对象不同。定义分别见 [PrivateMemorySize64](https://learn.microsoft.com/en-us/dotnet/api/system.diagnostics.process.privatememorysize64?view=net-10.0) 和 [WorkingSet64](https://learn.microsoft.com/en-us/dotnet/api/system.diagnostics.process.workingset64?view=netframework-4.8.1)。应用内同时记录 `ProcessInfo.currentRss/maxRss`；后者为平台相关的驻留集高水位，独立于外部 1 Hz 样本最大值，见 [Dart ProcessInfo](https://api.dart.dev/dart-io/ProcessInfo-class.html)。

每组还取得 106 个阶段快照。内置报告的 `privateBytesMeasured=false` 指内置采样器自身没有读取 Private Bytes；本节的该指标来自外部同 PID 采样。资源报告没有启动帧时间采样，其中为零的 `frame*` 字段不参与帧性能评价。

### 观测结果

| 指标 | 原版 | 候选 | 候选相对变化 |
| --- | ---: | ---: | ---: |
| Private Bytes 外采最大值 | 618,237,952 B／589.60 MiB | 428,658,688 B／408.80 MiB | −30.66% |
| Working Set 外采最大值 | 220,114,944 B／209.92 MiB | 248,844,288 B／237.32 MiB | +13.05% |
| 应用记录的 RSS 高水位 | 266,833,920 B／254.47 MiB | 259,481,600 B／247.46 MiB | −2.76% |
| 关闭后的共享图片缓存 | 46,080,000 B／43.95 MiB，8 项 | 0 B，0 项 | 缓存引用释放 |
| 阶段快照中的图片缓存最大值 | 46,080,000 B | 2,789,456 B | 仅指缓存计数 |
| PDF 查看器页图缓存上限 | 48 MiB | 24 MiB | 配置上限减半 |
| 终端缓冲行数最大值 | 3000 | 3000 | 保持预算 |

关闭阶段的 21 个快照，两版 `owners` 均为空、图片 live／pending 均为零；候选 `scopes` 也均为空。这说明挂载树中已没有这些面板所有者。它不等于全进程资源归零，也不能单独证明所有原生对象已被回收。外采句柄数在第 5／20 轮标记前分别为原版 641／637、候选 609／658；候选后段存在阶跃变化，未作句柄类型分解，不据此宣称原生句柄全部达到平台期。

以 Private Bytes 外采最大值衡量，本组下降超过 20%；物理驻留指标没有同样幅度的下降，其中 Working Set 的外采最大值反而增加。峰值统计包含初始化及预热，不能用单个百分比概括全部内存指标。两版 RSS 高水位高于对应的外采 Working Set 最大值，也说明离散采样没有捕获所有瞬时驻留峰值。

### 第 5 至第 20 轮增长判据

判据为 `第20轮值 ≤ B + max(0.10 × B, 32 MiB)`，其中 B 为第 5 轮关闭静置检查点。下面的 Private Bytes 取“不晚于关闭标记的最后一个外采样本”，避免选到下一轮已经打开预览或最终退出清理时的样本。RSS 取应用在该检查点直接记录的值。

| 指标 | 第 5 轮 B | 第 20 轮 | 增量 | 允许增量 | 判定 |
| --- | ---: | ---: | ---: | ---: | --- |
| 原版 Private Bytes | 445.63 MiB | 471.66 MiB | +26.04 MiB | 44.56 MiB | 通过 |
| 候选 Private Bytes | 379.88 MiB | 264.22 MiB | −115.66 MiB | 37.99 MiB | 通过 |
| 原版 RSS | 179.44 MiB | 181.23 MiB | +1.79 MiB | 32.00 MiB | 通过 |
| 候选 RSS | 212.18 MiB | 179.64 MiB | −32.54 MiB | 32.00 MiB | 通过 |

原始 `summary.json` 使用时间上最近的外采样本，候选 Private Bytes 为 371.95→257.66 MiB；其第 5／20 轮样本分别晚于标记 0.435／0.204 秒。按标记前取样后，候选为 398,336,000→277,057,536 B，分别早于标记 0.578／0.810 秒，结论仍通过。外采结果是静置 60 秒检查点附近的近似值，不是同步精确读数。原版这两个检查点的最近样本本就在标记之前，数值不变。

该判据约束两个端点，不要求中间每轮单调下降。候选第 19／20 轮 Private Bytes 降幅明显；没有 GC／原生分配事件关联数据，不将该降幅归因于某一种回收机制。

## Profile 持续流式

测量入口为 `experience_stream_soak_test.dart` 的固定快照，两版 SHA-256 均为 `75ce19eb212fba76cac85815b3ba81d6165f16321fa514f55dc64c561c7618c9`。

从 800 条历史开始，以真实时间发送 30 delta/s，持续 600 秒，每约 10 秒结束一个回复并开始下一轮，夹杂工具调用与完成事件。模拟 EventChannel 驱动真实 `DesktopController._onFrame`，保留原有 72 ms 合并刷新；没有直接替换正文来模拟流式。绑定采用 `fullyLive`，不按每个 delta 强制插入渲染帧。`watchPerformance` 采集 SDK 汇总及原始 FrameTiming，资源预算和最终回复完整性同时检查。

| 指标 | 原版 | 候选 |
| --- | ---: | ---: |
| 实际 delta 数／完成回复数 | 18,000／60 | 18,000／60 |
| 实际发射速率 | 29.999178 delta/s | 29.999046 delta/s |
| 合并后的正文通知数 | 6002 | 6005 |
| SDK 统计窗口帧数 | 86,901 | 84,445 |
| UI P95 | 4.503 ms | 5.067 ms |
| Raster P95 | 1.734 ms | 2.230 ms |
| UI 超过 16,667 µs | 2 帧／0.00230% | 8 帧／0.00947% |
| Raster 超过 16,667 µs | 0 帧／0% | 0 帧／0% |
| 发射调度滞后超过 16,667 µs | 0 次 | 18 次 |
| 最大发射调度滞后 | 16.469 ms | 31.601 ms |
| 结束时历史窗口 | 1160 事件／462,512 B | 1160 事件／462,512 B |
| 结束时实时缓冲 | 0 事件／0 B | 0 事件／0 B |

两版满足 UI、Raster P95 ≤16.667 ms，且各自超预算帧比例 ≤1% 的目标。候选 UI、Raster P95 均比原版高，分别增加 12.52% 和 28.60%；此结果只说明绝对阈值通过，不说明该流式负载变快。16.667 ms 是 60 Hz 的评价预算，不表示测试显示器运行在 60 Hz；发射调度滞后也不是键盘输入到界面反馈延迟。

原始时序数组在 SDK 汇总完成后又收到原版 11 条、候选 8 条记录。本表仅采用 SDK `frame_count` 对应的前 86,901／84,445 条记录；P95、超预算数均从这两个窗口重新计算，并与 `frameTargets` 核对。尾部记录完整保留，可能包含引擎延迟上报，未混入已冻结的统计窗口。SDK 自带的 missed-budget 计数不替代本表统一的 16,667 µs 阈值。

## 证据身份与复现边界

所有时间均为北京时间，2026-09-29。

| 场景 | 原版 | 候选 |
| --- | --- | --- |
| Release 资源运行 | 16:25:20—16:46:56，PID 25520 | 18:05:13—18:26:45，PID 27288 |
| Profile 流式运行 | 17:29:52—17:39:55，PID 27796 | 18:39:29—18:49:32，PID 16360 |
| Release AOT SHA-256 | `ab01b96f61e02b2c7c02752e62397c10e123cc84d92831709d6d1030eb3a5781` | `9f361d22a3335a6cbe3f8e88c7c3b3ebcf69fc6824bda09eeeafc3671baee2fd` |
| Profile AOT SHA-256 | `3fbdada6fabc32121c8345f9b89a8855689fa1d5ad798fc62d76bc8a5c9e3c4d` | `49bfab49d91732844cc0566c4c640d563a7151949a776bc5eedc32ffb628581b` |

原始证据目录：`D:\codex操作目录\flutter-experience-upgrade-20260929`。

| 证据 | 原版文件 | 候选文件 |
| --- | --- | --- |
| 资源阶段快照 | `baseline-soak-report.json` | `candidate-soak-report.json` |
| 同 PID 外部采样 | `baseline-soak-process.jsonl` | `candidate-soak-process.jsonl` |
| 原始最近样本汇总 | `baseline-soak-summary.json` | `candidate-soak-summary.json` |
| 资源进程身份 | `baseline-soak-identity.json` | `candidate-soak-identity.json` |
| 流式原始帧及汇总 | `baseline-stream-soak-report.json` | `candidate-stream-soak-report.json` |
| 流式构建身份 | `baseline-stream-soak-identity.json` | `candidate-stream-soak-identity.json` |

独立复算数据为 `resource-report-audit.json` 与 `stream-report-audit.json`；候选性能源码树清单为 `measured-candidate-source-identity.json`。原始资源报告 SHA-256 分别为 `43fe5b6ffc91ce4ae1b2496234a14bba25189c3d9228b313b2f11f7afc3e326b`、`9cd30a0da8c984d8bcd0851f94643eea1adb7aa12044292c3d243882c9819a8c`；流式报告分别为 `97d63f73dd8b0f0ed9b97215651b2f14d35207fb4f42308936ac58a1be02748d`、`3eb0e45ed0d14d83fee93f61c125abf6f7d5bf697a2b706093d042a05478e96c`。

本组没有采集真实 Host 及子进程的内存、网络或模型推理耗时、真实 PTY、Office 转换、Computer Use 连续 5 分钟、60 分钟混合使用、堆保留链或原生句柄类型。隐藏两分钟及内存压力回收有[组件生命周期回归](../../apps/desktop_flutter/test/preview_retention_test.dart)，未在本组关闭循环中单独量化其进程内存曲线。三次独立重复、跨平台实机、安装版真实业务负载仍应分别记录，不能由这里的单次受控比较替代。
