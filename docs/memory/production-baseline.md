# Rust Harness 正式内存基线

## 身份与范围

- 日期：2026-08-26
- 正式入口：`http://127.0.0.1:58080/`
- 正式二进制：`target/release/dsh.exe`
- 配置根：报告只记录路径SHA-256，不记录明文路径
- 原始报告：[production-baseline.jsonl](production-baseline.jsonl)

该基线通过正式58080、当前正式配置根和真实`dsh.exe`监听PID执行。场景只调用只读RPC，不恢复Agent、不调用模型、不执行Git写操作、不修改会话或凭据。

## 可记账结论

- `session.list`在累计20、100、200次边界没有显示线性增长。
- 有界history在累计20、100、200次边界出现可控高水位，第二组100结束后未继续按次数线性增长。
- 全程PID不变，线程、handles和直接子进程保持在机器派生证据记录的稳定范围。
- 原始报告包含13条记录：7条snapshot、6条workload；所有snapshot的binary SHA和home path hash一致。
- 原始报告不包含正式配置根明文、会话正文、凭据、命令行或Git提交内容。

## 未完成与限制

- 该结果证明的是当前正式会话目标的有界list/history，不代表68k synthetic fixture已通过Rust JSONL/SQLite后端。
- 尚未覆盖Git第二批100、PTY填满/关闭、浏览器20/100次刷新、subagent history、attachment cold scan、完成/删除/重启。
- 该矩阵在新Release运行期执行，是只读RPC基线；不得用单次baseline替代长期浏览器交互后的稳定常驻值。
- 同一SHA下正式浏览器单次刷新后：Host Working Set 32.8MB、Private 75.9MB、18线程、190 handles、直接子进程0；浏览器JS heap约29.2MB、DOM约1013节点。
- 单次刷新请求中`settings.describe`仍出现7次，是下一阶段明确RED；此外host/list/subagent/workspace/history/skill/commands/preset/models各1次，credentials 2次。
- 此单次刷新值不能推翻此前长期运行约95MB Working Set/158MB Private的高水位观测；下一阶段需要自动化20/100次刷新和静置采样。
- Working Set和Private Bytes必须继续分开记账。

## 68k合成夹具

工具`tools/memory_fixture.py`已实际流式生成并删除临时fixture：

- 事件数：68,000
- 消息组：40（80个消息边界）
- 字节数：6,651,610
- SHA-256：`b4c64a5e7f0c10fd24d8232b731c660683f2aba62fc90df063fe015b35cd9234`

fixture完全合成，不使用正式会话正文、用户路径或凭据。下一阶段需要由Rust JSONL/SQLite后端测试导入该逻辑记录流并验证首/中/尾页。

## 复现命令

```bash
python -m unittest discover -s tools/tests -p "test_memory_*.py" -v
python tools/memory_scenarios.py \
  --binary "D:/deepwork/deepseek-harness-rs/target/release/dsh.exe" \
  --home "$LOCALAPPDATA/DeepSeek Harness" \
  --history-session "<当前可读且hasMore=true的会话ID>" \
  --output "docs/memory/production-baseline.jsonl"
python tools/validate_memory_baseline.py --report docs/memory/production-baseline.jsonl --markdown docs/memory/production-baseline.md --update
python tools/validate_memory_baseline.py --report docs/memory/production-baseline.jsonl --markdown docs/memory/production-baseline.md
```

每次Release SHA变化后必须重跑并覆盖本基线；旧报告自动失效。

<!-- MEMORY-EVIDENCE:START -->
## 机器派生证据（请勿手工编辑）

- PID：`15596`
- 二进制SHA-256：`9585c2e102516dd8ead940a39e914e767708ec0d141ad26cfcdb6f73084a7d4e`
- 报告SHA-256：`5f72d1ba12497c841966c07e02c1d905a6f004af9bfbf55ba714f81a106a6d43`
- 记录：13（snapshot 7 / workload 6）

| 采样点 | Working Set MB | Private MB | 线程 | Handles |
|---|---:|---:|---:|---:|
| baseline | 33.5 | 76.3 | 18 | 191 |
| list_20 | 32.9 | 77.0 | 20 | 193 |
| list_100 | 32.9 | 76.6 | 20 | 193 |
| list_second_100 | 32.8 | 76.4 | 20 | 193 |
| history_20 | 38.4 | 78.8 | 20 | 193 |
| history_100 | 41.5 | 85.9 | 20 | 193 |
| history_second_100 | 38.5 | 82.1 | 20 | 193 |

| 工作负载 | 批次请求 | 累计请求 | 响应MB | 秒 |
|---|---:|---:|---:|---:|
| list_20 | 20 | 20 | 1.07 | 0.269 |
| list_100 | 80 | 100 | 4.26 | 1.396 |
| list_second_100 | 100 | 200 | 5.33 | 2.092 |
| history_20 | 20 | 20 | 8.56 | 3.102 |
| history_100 | 80 | 100 | 34.24 | 8.234 |
| history_second_100 | 100 | 200 | 42.80 | 10.014 |
<!-- MEMORY-EVIDENCE:END -->
