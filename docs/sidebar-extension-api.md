# 侧栏扩展 API

`dsh-better-sidebar` 在客户端 Cordis 上提供 `betterSidebar` 服务。消费插件的 `package.json` 必须把 `dsh-better-sidebar` 写入 `dsh.client.inject`，客户端入口同时声明 `exports.inject = ["betterSidebar"]`。依赖由 Host 启动图排序，扩展无需读取全局变量。

文件、Git、网页预览和终端也由该服务注册为 `explorer`、`git`、`browser`、`terminal` 描述符，因此与第三方标签使用相同的打开、激活、停靠、分栏、浮窗、移动端合并和恢复路径。旧布局中的 `state.tab = "explorer" | "git" | "browser" | "terminal"` 会迁移为对应的 `builtin:*` 标签 ID，并保留文件列表、URL 和导航历史。

```json
{
  "dsh": {
    "client": {
      "platform": "web",
      "inject": ["dsh-better-sidebar"]
    }
  }
}
```

```js
const inject = ["betterSidebar"];

function apply(ctx) {
  ctx.effect(() => ctx.betterSidebar.registerTab({
    id: "example:tasks",
    title: "任务",
    order: 120,
    single: true,
    settings: {
      pluginToggles: [
        { key: "compact", title: "紧凑显示", type: "switch", defaultValue: true }
      ]
    },
    component({ tab, visible, pluginSettings }) {
      return React.createElement("section", null,
        visible ? tab.title : "已暂停",
        pluginSettings.compact ? " · 紧凑" : "");
    }
  }));
}
```

注册函数返回幂等 disposer。消费插件应把 disposer 交给 `ctx.effect`，这样插件停用或热重载会撤销描述符。重复 ID 会立即抛错；撤销描述符不会删除用户已保存的标签，重新注册同一 ID 后标签会恢复显示。

## 页面描述符

`registerTab(descriptor)` 与上游常用字段兼容：

- `id`、`title`、`component` 为必填项；ID 只允许字母、数字、冒号、点、下划线和连字符。
- `order` 控制入口顺序，`hidden` 隐藏未打开入口，`available(ctx, scope, state)` 控制入口当前是否可用。
- `single` 或 `dedupeKey(tab)` 控制复用。`createTab(state)` 可以生成自定义 ID、标签数据及状态补丁。
- `urlTarget(url)` 可声明接管对话中的 HTTP(S) 链接；按注册顺序选取第一个启用且匹配的页面。
- `badge` 生成标签徽标；`onOpen`、`onActivate`、`onClose` 接收实际标签和会话范围。回调异常会记录错误，但不会中断侧栏。
- `component` 接收 `{ ctx, store, scope, tab, visible, pluginSettings }`。已打开的扩展页面在切换标签时保持挂载，通过 `visible: false` 暂停轮询；`pluginSettings` 是该描述符已解析的本地设置，原始持久值也可从 `store.getSnapshot().prefs.pluginSettings[descriptor.id]` 读取。

服务提供 `openTab`、`activateTab`、`updateTab`、`closeTab`、`openFile`。`moveTab` 把标签停靠到右侧或底部的指定窗格，并可用 `edge` 创建分栏；`floatTab` 创建浮窗，`updateFloat` 更新经过视口约束的几何。带 `scope` 的操作写入指定会话，不会切换当前会话；异步回调或跨会话更新必须向 `updateTab(tabId, patch, scope)` 等方法显式传入原会话范围，不能依赖届时的当前会话。`getSnapshot` 与 `subscribeState` 可供 `useSyncExternalStore` 使用；快照在没有状态变更时保持引用稳定。

## 布局与拖放

每个会话包含右侧 `splits`、底部 `bottomSplits` 和 `floats`。分栏是最多三层的 leaf／row split／col split 树，每个 split 最多包含四个子节点；标签在任一时刻只属于一个 leaf 或浮窗。拖动标签到窗格中心会合并，拖到左、右、上、下边缘会创建对应方向的分栏。分隔条、底栏高度和浮窗几何都支持指针与键盘。

指针移动阶段只发布本地预览，`pointerup` 才提交到 Host。`pointercancel`、`lostpointercapture`、隐藏视图和组件卸载会恢复初值；保存失败时显示错误和“重试保存布局”。移动端不改写桌面树：右侧、底部和浮窗中的内置与第三方标签合并进同一全宽抽屉，返回桌面后恢复原停靠位置。

文件页不可整体关闭，以免绕过未保存检查。按会话和路径保存的草稿内存缓存使文件页在右侧、底部、分栏和浮窗之间移动时保持编辑内容；缓存总量限制为 8 MiB，只淘汰已保存项，不会静默淘汰脏草稿。容量不足时编辑器显示警告并阻止移动，保存或确认关闭文件后释放缓存。

## 文件查看器描述符

`registerFileViewer(descriptor)` 按 `priority` 从高到低匹配，相同优先级保持注册顺序。`detect(path, head)` 在调用方提供文件头时先于扩展名；空 `exts` 且带 `detect` 表示只通过内容探测，空 `exts` 且没有 `detect` 表示兜底查看器。

加载策略如下：

| `fetchStrategy` | 组件收到的数据 |
| --- | --- |
| `none` | 不预取内容。 |
| `fsRead` | UTF-8 文本写入 `content`。 |
| `mediaUrl` | 受工作区边界保护的地址写入 `mediaUrl`。 |
| `custom` | 调用 `load(path, scope, signal)`，结果写入 `customData`；视图卸载时中止信号。 |
| `binary-download` | 下载地址写入 `mediaUrl`，由组件决定呈现方式。 |

查看器组件接收 `{ ctx, store, scope, path, title, viewerId, content?, mediaUrl?, customData?, pluginSettings }`。查看器禁用、撤销或不匹配时继续检查下一项，最终回落到内置查看器。

## 设置与持久化

`settings.toggles` 读写 Host 已声明的侧栏字段；未知 Host 字段会显示保存错误。`settings.pluginToggles` 支持 `switch`、`text`、`number`、`select` 与多选，值写入 `pluginSettings[descriptor.id]`。`settings.render(props)` 可以代替声明式行并通过 `updatePluginSetting` 保存自定义 JSON 值。

每个页面和查看器另有独立启停开关。显式 `false` 才表示禁用，因此安装新扩展后默认可用。会话布局保存活动页面、文件与网页标签、导航历史、第三方标签的 `path` 和 JSON `meta`，以及分栏树、底栏高度和浮窗几何；临时全屏和移动抽屉不写入磁盘。

完整声明位于包内 `lib/client.d.ts`。当前服务特性通过 `betterSidebar.features` 检查，其中 `splitLayout`、`bottomPanel` 和 `floatWindows` 分别表示分栏、底栏和浮窗能力。
