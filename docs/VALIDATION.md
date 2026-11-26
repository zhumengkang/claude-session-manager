# 验证状态与历史验收边界

[文档中心](README.md) · [测试说明](../smoke-tests/README.md) · [Windows 实机](WINDOWS.md) · [GitHub 更新](UPDATES.md) · [发布清单](RELEASE.md)

> 本文区分“当前源码普通检查”“前端模拟 IPC”“历史真实二进制验收”和“仍未覆盖项”。测试数量、run ID 和性能只代表对应日期与输入；文档不固定旧构建哈希，实际产物以当次发布清单和交付证据为准。

## 1. 当前源码：2026-11-26

### Rust 与静态检查

实际执行：

```bash
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

结果：

- debug 与 `--release` 均为 **112 项通过，0 项失败，13 项显式 ignored**。
- 严格 Clippy 退出码 0。
- ignored 项不计入通过数；真实 Terminal、真实 CLI、真实大文件等 opt-in 测试没有随普通测试自动执行。

新增更新模块覆盖：

- `owner/repository` 校验与 URL/凭据形式拒绝。
- 可选 `v` 前缀的语义化版本，同版本、旧版本和高版本。
- 草稿、预发布、外部 Release 地址拒绝。
- 忽略版本不改变版本可用性。
- 设置读写与损坏文件不被静默覆盖。
- GitHub API 地址固定到所选公开仓库。

这些普通测试没有访问真实 GitHub。

### Git 源码专用边界

2026-11-26 在本地源码专用仓库实际执行：

- `scripts/check-source-only.sh --staged`：**67 个文件通过，0 个发布附件**。
- `.githooks/pre-commit` 与无 HEAD 时的 `.githooks/pre-push`：均通过同一暂存树检查。
- 在仓库外的独立临时 Git 仓库中，正常源码的 `--staged` / `--tracked` 均通过。
- 伪造 `.dmg`、无扩展名 Mach-O，以及已经提交到临时 HEAD 的无扩展名 Mach-O 均以退出码 1 被预期拒绝。
- 临时测试仓库已清理；上述负向夹具没有进入项目源码树。

这证明本地机械检查有效，但不等于 GitHub 远端 ruleset、分支保护或 CI 已经运行/配置。hooks 也可能被显式绕过，远端保护仍需所有者配置。

### 完整前端 mock 冒烟

实际执行：

```bash
./smoke-tests/run-frontend-smoke.sh
```

结果以 `FRONTEND_SMOKE_OK` 结束、退出码 0，**0 JavaScript 异常、0 外部 HTTP 资源**。

除既有 Provider、筛选、固定快照、收藏、导出、布局和恢复 IPC 外，更新场景覆盖：

- 保存仓库与自动检查开关。
- 高版本显示横幅。
- Release 名称中的 HTML 只作为文本，不执行。
- 打开发布页 IPC 传精确 URL。
- 忽略后后台不重复弹出，手动检查可重新查看。
- 同版本不显示升级横幅。
- 后台失败不弹全局错误 toast。
- 空仓库显示配置提示。
- 更新面板和新版本横幅截图已生成，并已逐张视觉复核后加入本地文档。

### 独立前端审查回归

实际执行：

```bash
./smoke-tests/run-frontend-smoke.sh ./frontend-review-regression.mjs
```

结果：

```text
REVIEW_REGRESSION_SUMMARY total=27 passed=27 failed=0
```

新增 `update-panel-empty-startup-and-manual-failure` 验证空仓库启动不发起检查、手动失败只在自己的面板中显示。该 mock 会真实保存自己的模拟设置，不再出现“保存成功但读取永远为空”的不一致。

### 新 macOS App / DMG 构建

实际执行 `cargo tauri build`，退出码 0，生成：

```text
src-tauri/target/release/bundle/macos/Claude Session Manager.app
src-tauri/target/release/bundle/dmg/Claude Session Manager_0.1.0_aarch64.dmg
```

- `codesign --verify --deep --strict` 通过，`hdiutil verify` 报告 VALID。
- 只读挂载 DMG 后，内部 App 的 strict codesign 同样通过，且与构建目录中的完整 `.app` 逐文件一致；挂载点已卸载并删除。
- 没有覆盖 `/Applications` 中的现有安装版。
- 构建目录中的 App 使用隔离空 Claude/Codex 目录实际启动；只捕获该 PID 的 CGWindow，已查看白色主界面和左侧“更新”入口。
- 两个同 Bundle ID 管理器窗口位置重叠。第一次坐标驱动出现控件归属不一致，未据此认定点击成功；随后改为顶层窗口所有权校验，校验拒绝后不再发送点击。**真实 App 更新面板点击不记为通过**，前端 mock 点击结果不能冒充这一阶段。
- 测试 App 仅按精确 PID/路径终止，独立确认进程消失；隔离临时目录已删除。现有安装版进程保留，LaunchAgent 仍不存在，未创建或关闭 Terminal。
- 真实 GitHub 仓库、真实 App 更新面板和系统浏览器仍需后续验收。

## 2. 当前更新功能尚未完成的验收

以下不能由上述普通测试替代：

- 真实公开 GitHub 仓库的同版本与更高版本只读响应。
- 实际 GitHub 限流、代理、TLS、404 与非语义化标签的端到端表现。
- 在 macOS / Windows 新 Release 中点击更新面板与系统浏览器。
- macOS 新构建的安装、授权与完整真实 Terminal 回归。
- Windows 新 NSIS 构建、安装、更新、卸载、WebView2 与原生 UIA 验收。

**旧 `verified` 交付目录和旧二进制保持不变，不能声称其中已经包含 GitHub 更新功能。** mock 中的高版本与仓库名都是合成数据。

真实 GitHub 验收时只需读取公开 latest Release，不需要下载、安装新包，也不要编造项目仓库地址。

## 3. 既有真实 macOS 验收（历史构建）

先前 macOS Apple Silicon 安装版曾完成：

- `/Applications` strict codesign 与 DMG 校验。
- 当前构建 CDHash 对应的 Accessibility 授权核对。
- 独立安装版进程与合成会话数据。
- Claude 单开、Claude 双标签批量、Codex 双标签批量。
- 精确恢复 argv、cwd、TTY、环境和 1000 ms 稳定确认。
- 重复单开/批量没有新增 Provider 或 Terminal 对象。
- 原生 `AXTabGroup` / `AXRadioButton` 结构核验。
- 不相交批次形成多个原生窗口，不限制 Provider 窗口总数。
- 只按 owned ID + TTY 和精确 Provider 身份清理；未关闭当前对话或已有 Terminal。

同时保留这些边界：

- 桌面/iCloud 副本曾因 FinderInfo/resource fork 等 detritus 使 strict codesign 退出 1，不能写成桌面严格签名持续通过。
- 6.5 秒单开负向轮次没有稳定成功确认，但晚期驱动失败使错误 toast 未完成视觉验收。
- 随后单会话批量控制实际成功，不应误写成第二次也失败。
- macOS 后台 AppleScript 实验失败并已撤回；当前恢复仍是同步主线程 IPC。

这些记录属于旧安装版，不自动覆盖加入更新模块后的新源码或未来构建。

## 4. 既有真实 Windows 验收（历史构建）

先前 Windows 11 ARM64 构建曾完成：

- 真实 Rust 测试、Clippy、Release 与当前用户 NSIS。
- NSIS 更新、卸载、干净重装，以及最终安装路径完整 GUI。
- 真实 WebView2 的固定快照、消息分页、Markdown、收藏、导出和缓存。
- 真实 Windows Terminal 单窗多标签、跨 Provider 独立批次和不相交多窗口。
- 精确 argv/cwd/环境与完整集合 1000 ms 稳定确认。
- 同 UUID/不同 backing path 并发只启动一个 Provider 和一个 owned 标签。
- Codex CLI daemon 持有 rollout 时的关闭后重开，以及 VS Code 来源保守拦截对照。
- 缺失 CLI、失效 cwd、立即退出、Trust 未启动等负向保护。
- 只 Invoke owned 标签自己的关闭子按钮，不调用父窗口 Close；真实 Terminal 和 daemon 身份保留。

这些结果属于旧 ARM64 构建，不能扩展为：

- Windows x64 或 MSI 已支持。
- Explorer、自启动真实切换、原生标签聚焦已通过。
- 真实 `.ps1` Provider 完整恢复已通过。
- 官方账号完整 Claude/Codex TUI 历史与正文恢复已通过。
- 新 GitHub 更新功能已进入旧安装版。

## 5. 大文件与真实数据边界

历史 macOS/Windows 真实 8 GiB 合成 JSONL 验收证明：

- 冷解析、完整内容复核、追加和固定快照分页确实在大文件上运行。
- 当前一致性策略需要完整校验旧前缀，不能沿用旧版“追加只读少量字节”的性能声明。
- 大文件测试只输出大小、条数、offset、读取量和耗时等元数据。

真实 Codex 大 rollout 的只读分页只核对元数据，不输出真实正文。真实 CLI 自动发现和 `--version` 只证明定位与版本命令，不证明完整 TUI 恢复。

性能数据依赖具体机器、文件系统与构建，不应作为所有环境的固定延迟承诺。

## 6. 测试层级能证明什么

| 层级 | 能证明 | 不能证明 |
| --- | --- | --- |
| Rust 普通测试 | 解析、缓存、命令构造、身份与更新设置逻辑 | 真实 GUI、Terminal 或 GitHub 成功 |
| Clippy / fmt | 静态质量与格式 | 运行时行为 |
| 前端 CDP mock | 真实前端交互与模拟 IPC 契约 | 真实存储、CLI、TCC、GitHub、Terminal |
| 隔离 Provider GUI | 真实 App、精确 argv、原生标签与进程检测 | 官方账号完整 TUI 恢复 |
| 已安装 App GUI | 对应旧二进制的安装、授权和点击路径 | 新源码、新构建或其他平台 |
| 真实公开仓库检查 | GitHub latest Release 与版本提醒端到端响应 | 新安装包来源、签名或安装成功 |

## 7. 真实 Terminal 的强制约束

只有明确授权并能证明对象所有权时才运行 opt-in 测试：

- 永不关闭当前 Claude Code 对话窗口或用户已有 Terminal。
- 不通过“关闭全部 Terminal 窗口”清理，不重启 Terminal 或电脑。
- 不按进程名批量结束 CLI，不停止真实 Codex daemon。
- 不把前后 window ID 差集当作清理所有权。
- macOS 清理匹配当前脚本返回的对象 ID 与唯一 TTY。
- Windows 只 Invoke owned 标签自己的唯一关闭子按钮。
- 生产代码不得增加窗口关闭、失败自动清理或 AX 清理。
- 不修改真实项目 Trust/权限，不读取或打印真实会话正文作为测试日志。
- 恢复测试不发送模型提示，不绕过锁屏。
- 测试后及时清理本次拥有的对象；失败时如实保留并报告，不冒充已清理。

## 8. CI 与发布状态

2026-11-26 首轮源码导入已实际运行并通过：[source-only](https://github.com/zhumengkang/claude-session-manager/actions/runs/45785523418) 与 [windows-source-check](https://github.com/zhumengkang/claude-session-manager/actions/runs/45785521950) 均为 success。前者检查 Git 内容只允许源码及必要源码资源，后者完成 Windows 格式检查、普通 Rust 测试和严格 Clippy；二者均不上传构建附件。后续提交必须重新查看 Actions 状态，CI 通过也不能替代真实 Terminal 的隔离 GUI 验收。首轮 Windows 运行存在 `actions/checkout@v4` 的 Node.js 20 弃用提示，不影响该次 success 结论。

本地 `scripts/check-source-only.sh`、pre-commit 与 pre-push 是额外边界，不代表 GitHub 远端已经执行。详见 [GitHub 源码专用仓库与附件隔离](GITHUB_SOURCE_ONLY.md)。

项目自身 LICENSE 仍待补齐。发布前检查见 [RELEASE.md](RELEASE.md)。

**结论：当前普通回归通过，但仍存在上述未覆盖边界，不能声称“已经没有 bug”。**
