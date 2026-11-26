# Windows 原生隔离集成测试

[测试总入口](../README.md) · [Windows 实现](../../docs/WINDOWS.md) · [验证记录](../../docs/VALIDATION.md) · [源码编译](../../docs/BUILD.md)

这些脚本验证 **Windows 中实际构建的 Tauri 应用、真实 WebView2、真实 Windows Terminal 和原生 UI Automation**。它们使用合成 Provider，不是生产 Claude/Codex 账号或完整会话恢复的替代验收。

## 1. 前置条件

- Windows 10/11、Windows Terminal、WebView2 Runtime。
- Windows PowerShell 5.1 或兼容版本。
- Node.js 22.5+（夹具使用内置 `node:sqlite`）。
- 已构建的 `src-tauri/target/release/csm-app.exe`。
- 当前登录且未锁屏的 Windows 桌面会话。
- 明确授权本次测试创建和清理自己的 Terminal 标签。

不要在锁屏、无人交互会话或无法证明测试对象所有权时运行。

## 2. 运行

选择一个**尚不存在**的全新夹具目录。生成器会创建随机 UUID、唯一标签标题和合成数据，不读取用户真实的 `~/.claude` 或 `~/.codex`。

```powershell
node smoke-tests/windows/setup-fixtures.mjs C:\csm-fixture-new
node smoke-tests/windows/windows-gui-smoke.mjs `
  C:\csm-fixture-new\fixture.json `
  "$PWD\src-tauri\target\release\csm-app.exe"
```

可用 `CSM_WINDOWS_CDP_PORT` 修改测试专用的本地 WebView2 CDP 端口，默认 `19337`。CDP 参数只传给本次启动的测试应用进程，不修改浏览器或 WebView2 全局设置。

加入 GitHub 更新功能后的新 Windows 构建仍需重新执行此套件；旧 Release 的通过记录不能自动继承。

## 3. 验证范围

### 列表、详情与导出

- Claude/Codex 隔离列表、Codex SQLite、收藏、固定快照和反向分页。
- 首页后真实追加一条记录，加载更早仍得到 205 条唯一消息，不混入追加内容。
- Windows 反斜杠、普通、verbatim 与 UNC 路径显示和缓存键一致。
- 2 条短会话、1005 条长会话和两个并发导出通过真实 WebView2 调用统一后端。
- 导出核对数量、时间顺序、无重复、不同最终路径和不覆盖；`cleanup_warning` 必须为空。
- 只删除返回路径中同时匹配本次 run ID、会话 ID 和固定导出目录名的文件。

### 启动、负向保护与身份

- 非空失效 cwd 在创建终端前报错，不回退 home。
- Provider 立即退出、Trust 未确认且 Provider 未启动，都返回 `0/1` 和“未执行自动清理”。
- 完整启动集合连续稳定匹配 1000 ms；任何不完整观察重新计时。
- Claude/Codex 精确恢复 argv、中文/空格/单引号/`&`/分号 cwd 和环境继承。
- 工作目录由编码 PowerShell 内的 `Set-Location -LiteralPath` 设置，不作为 `wt --startingDirectory` 传递。
- 收藏按当前 Provider 隔离；Claude 运行不阻塞首次 Codex 批次。
- 不相交批次形成新窗口；重复 Provider/session 不重复启动。
- 8 路同 UUID、不同 backing path 并发只能产生 1 个交互 Provider 和 1 个 owned 标签。

### Codex daemon 与重开

- 两个隔离 `app-server-daemon` 分别持有 `source=cli` 与 `source=vscode` rollout。
- 修复后 CLI daemon 场景可重开，VS Code 对照仍保守阻止；精确恢复进程检查不得削弱。
- 关闭带随机 run ID 的 owned Codex 批量标签后等待交互 Provider 退出。
- daemon 仍持有 rollout 时重开，必须得到新交互 Provider PID 和一个 owned 原生标签。
- 无效 cwd 负向检查在对应 Provider 尚未运行时执行，避免运行中保护先行返回。

### 原生结构、网络与证据

- UIA 核实 Claude 多标签和 Codex 标签位于预期的新 HWND。
- 初始 Claude 双标签、并发单标签、初始 Codex 单标签和 Codex 重开单标签分别保存独立 JSON/PNG，不允许后阶段覆盖。
- WebView2 的 `ipc.localhost` 与 `tauri.localhost` 是精确内部豁免；其他 HTTP(S) 请求仍计为外部。
- 请求清单在完整 GUI 流程末尾再次保存和断言。
- JavaScript 异常、意外外部请求和模型提示都必须为 0。

## 4. 所有权与清理

- 不使用窗口前后差集，不按进程名批量结束，不操作已有 Terminal 标签。
- 每次清理前重新枚举随机 run ID 标签，核实父窗口进程为 `WindowsTerminal`。
- 目标标签必须只有一个可 Invoke 的关闭子按钮。
- **只 Invoke 标签自己的关闭子按钮，不调用父窗口 `WindowPattern.Close`。**
- 混合归属窗口本身不关闭，其中其他标签保持不动。
- 清理后本次 run ID 标签必须为 0；另只读核对记录的纯测试 HWND 已消失。
- 无法证明所有权时保留对象并报告，不扩大清理范围。
- Provider 清理同时匹配日志 PID、Node 可执行文件、唯一夹具脚本路径和会话 UUID。
- daemon 还必须匹配本次唯一 daemon exe 与 `app-server` 角色。
- 区分标签关闭导致的自然退出和测试主动停止，不能混报。
- 测试 App 由驱动直接创建，只结束这个直接子进程。
- 不停止真实 Codex daemon、WindowsTerminal、当前对话或其他用户进程。
- 失败后也只尝试上述受限清理；不得改成全局关闭。

## 5. 输出

夹具目录包含：

```text
providers.jsonl
launch.log
first-batch-result.json
sequential-codex-batch-result.json
codex-daemon-guard-result.json
concurrent-alias-open-result.json
invalid-cwd-result.json
codex-reopen-result.json
network-requests.json
export-result.json
gui-smoke-result.json
windows-app.png
windows-codex-detail.png
```

原生证据分别保存：

```text
native-validation-claude-batch.json / terminal-tabs-claude-batch.png
native-validation-claude-concurrent-alias.json / terminal-tabs-claude-concurrent-alias.png
native-validation-codex-batch.json / terminal-tabs-codex-batch.png
native-validation-codex-reopen.json / terminal-tabs-codex-reopen.png
```

所有正文和截图都应是合成夹具内容。不要对混合归属窗口或含真实凭据、真实会话的屏幕截图。

## 6. 成功判据

除具体场景断言外，至少要求：

```text
WINDOWS_GUI_SMOKE_OK
OWNED_TERMINAL_TABS_REMAINING=0
OWNED_PROVIDER_PROCESSES_REMAINING=0
```

并独立确认：

- 成功启动均记录 `stable_ms=1000`。
- 负向路径没有 opened 假成功。
- 并发别名只有一个 Provider 与一个 owned 标签。
- Codex 关闭重开得到新交互 PID。
- 导出与缓存工作区按证据清理。
- 测试前已有进程身份保持不变。
- 所有截图逐张查看，且没有被后续阶段覆盖。

## 7. 历史实机结果的范围

先前 Windows 11 ARM64 构建已记录：

- 普通 Rust、Clippy、Release/NSIS 和真实大文件测试通过。
- NSIS 更新、卸载、干净重装和最终安装路径完整 GUI 通过。
- 真实 WebView2 的分页、导出、收藏、SQLite/缓存和合成会话详情通过。
- 原生单窗多标签、不相交多窗口、跨 Provider 独立批次、daemon 对照与 Codex 重开通过。
- 同 UUID 并发保护、三种负向保护和严格 owned 清理通过。
- 合成截图逐张查看，0 JavaScript 异常、0 意外外部 HTTP 请求、0 模型提示。

这些记录属于旧构建。具体 run ID 与证据范围见 [../../docs/VALIDATION.md](../../docs/VALIDATION.md)，产物身份以当次交付清单为准，不在持续维护文档中固定旧哈希。

## 8. 明确未覆盖边界

- 新 GitHub 更新功能尚未进入旧 Windows 安装版，真实公开仓库检查未完成。
- Windows x64、MSI、Explorer、自启动真实切换和原生标签聚焦未覆盖。
- 真实 `.ps1` Provider 完整恢复未覆盖。
- 模拟 CLI 不连接 Anthropic/OpenAI，不发送提示，不测试真实账号或配额。
- 合成 TrustPending 不修改或通过真实项目 Trust/权限。
- 官方 CLI 自动定位和 `--version` 不等于完整 TUI 历史与正文恢复。
- 真实官方 Codex Terminal 的关闭后重开未被自动化操作。
- 测试通过不等于“已经没有 bug”。
