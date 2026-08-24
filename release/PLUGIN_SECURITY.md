# Web 插件安全边界

Release 随附的 Web 插件运行在 DeepSeek Harness 的同源浏览器页面中，不是 iframe 或 Worker 沙箱。安装和启用插件等同于授予其当前 Web 页面可用的 JavaScript 能力。

仅安装来源可信、内容固定且经过审计的插件。GitHub 插件安装器要求完整 40 位 commit SHA，不接受分支、tag 或默认分支。插件客户端必须声明 `dsh.client.platform = web`，并提供包内 `exports["./client"]` JavaScript 文件。

纯 Web 插件不能执行 Rust Host 代码；声明 Node Host bundle 的第三方插件只加载其 Web 客户端部分，Host 部分会被明确跳过。插件不得读取凭据、调用未声明的外部服务或执行安装脚本。

禁用插件后，下一次 Host 重启不再把它加入 Web 启动清单。插件目录、依赖清单和插件库存均位于用户 Profile 运行目录。