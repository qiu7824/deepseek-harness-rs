# Host 启动诊断

发布流程在在线模型检查之前启动一个隔离的 Host，验证就绪地址和 `session.list` 接口。启动超过 45 秒、进程提前退出或接口不可用时，构建失败，并保留标准输出、启动跟踪与结果记录。

`DSH_TRACE_STARTUP=1` 启用 Cordis 生命周期的标准错误跟踪；默认不启用。跟踪不输出模型请求内容或配置对象。诊断使用独立 `DSH_HOME`，日志不进入安装包。

GitHub Actions 的 `Host startup diagnostics` 工作流只在 Linux 上构建 Host，分别执行默认启动与带跟踪的启动，保留 Cargo 编译耗时报告和诊断文件。该工作流不创建 Release，也不替代全平台发布验收。
