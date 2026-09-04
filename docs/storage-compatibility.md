# 存储版本兼容与恢复

## 权威数据与派生缓存

会话日志、工作区和设置是权威数据。读取失败、格式损坏或版本不兼容时，打开操作返回明确错误，保留原文件，禁止以空数据继续写入。

Session projection cache 是可重建的派生数据。只有显式声明 `BackupAndSkip` 的域允许隔离坏记录：先将原始文件保存为带唯一标识的 `.bak.*` 文件，再从会话日志重建缺失投影。备份不覆盖既有备份。

无法读取文件、无法保存备份或全局单例记录损坏时，打开操作失败，不以默认值替代。备份文件可能包含会话信息，应按用户数据保管。

## JSON 布局

| 布局 | 文件位置 | 版本规则 |
|---|---|---|
| Single | `<storage-root>/<unit>.json` | 只接受域声明的精确版本 |
| Per-record | `<storage-root>/<unit>/<table>/<key>.json` | 接受当前版本及显式声明的兼容旧版本 |

兼容版本必须是互不重复、低于当前版本的正整数，只能用于 per-record 布局。语法错误、无效 UTF-8 和不兼容版本保留为明确的读取问题，由域的恢复策略处理。

Per-record 写入将当前版本记录在每个文档中。读取未来版本不会将其解释为不存在；可重建缓存必须先保存该文档，才能生成当前版本的替代记录。

## Session projection cache

`session_projcache` 使用 per-record v5，并接受可解码的 v3、v4 记录。旧单文件布局迁移时先构建完整的新目录，再一次性发布；原单文件保留。

已经存在的新布局目录优先于旧单文件，包括记录已全部删除后的空目录，避免重启时重新导入已删除记录。

投影身份包含 `createdAt`、`cwd`、`isSeeded` 和 `inheritedEventCount`。旧记录缺少继承字段时归一为 `false` 和 `0`；种子会话或继承前缀改变时不得复用该记录。新写入始终包含完整继承信息。

## 回归验证

```bash
cargo test --locked -p dsh-storage-domain -p dsh-storage-json -p dsh-session-projection-cache
python tools/e2e_settings_model_preserves_data.py --binary <packaged-host> --repo <package-directory>
```

自动验证覆盖坏记录与未来版本的策略边界、重复备份保留、v3/v4 到 v5 的迁移、标题保留、继承身份、删除后重启，以及修改模型配置前后的会话和工作区持久化。
