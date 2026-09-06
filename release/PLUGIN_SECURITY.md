# Web 插件安全边界

Release 随附的 Web 插件运行在 DeepSeek Harness 的同源浏览器页面中，不是 iframe 或 Worker 沙箱。安装和启用插件等同于授予其当前 Web 页面可用的 JavaScript 能力。

仅安装来源可信、内容固定且经过审计的插件。GitHub 插件安装器要求完整 40 位 commit SHA，不接受分支、tag 或默认分支。插件客户端必须声明 `dsh.client.platform = web`，并提供包内 `exports["./client"]` JavaScript 文件。

插件可以在 `dsh.client.assets` 中声明按需加载的 JavaScript。Host 只接受包目录内的规范文件、单段安全文件名和 `.js` 扩展名；主入口不超过 2 MiB，每个插件最多声明 16 个资源，单个资源不超过 8 MiB且合计不超过 32 MiB。Host 以有界读取取得入口和资源，并把所有实际资源内容纳入入口修订号与路由摘要；只改变资源也会生成新 URL。资源与入口同源，禁用或卸载插件后路由一并撤销。按需资源拥有与插件入口相同的页面权限，不能作为权限隔离边界。

纯 Web 插件不能执行 Rust Host 代码；声明 Node Host bundle 的第三方插件只加载其 Web 客户端部分，Host 部分会被明确跳过。插件不得读取凭据、调用未声明的外部服务或执行安装脚本。

禁用插件后，下一次 Host 重启不再把它加入 Web 启动清单。插件目录、依赖清单和插件库存均位于用户 Profile 运行目录。
